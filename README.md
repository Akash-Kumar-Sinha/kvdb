# KvDB

A disk-backed, embedded key-value store built from scratch in Rust — a B-tree storage engine with a page-based on-disk format, a compile-time-enforced typestate API, real thread-safe concurrency via a hand-rolled spinlock, and a zero-copy scanning API built on a hand-rolled `LendingIterator`.

## Table of Contents

- [Introduction](#introduction)
- [Motivation](#motivation)
- [Design Decisions](#design-decisions)
  - [Why a B-tree, not a hashmap](#why-a-b-tree-not-a-hashmap)
  - [Page-based disk storage](#page-based-disk-storage)
  - [The typestate pattern](#the-typestate-pattern)
  - [Typed values via `Value`](#typed-values-via-value)
  - [Real concurrency via a hand-rolled spinlock](#real-concurrency-via-a-hand-rolled-spinlock)
  - [Zero-copy scanning via `LendingIterator`](#zero-copy-scanning-via-lendingiterator)
- [How to Use](#how-to-use)
- [Workspace Layout](#workspace-layout)
- [Current Limitations](#current-limitations)

## Introduction

KvDB is an embedded key-value store — not a server you connect to over a socket, but a library you link directly into your program, the same relationship SQLite or `sled` have to the process using them. Keys are generic (`S`), values are stored as a typed `Value` enum, and everything is read/written a fixed-size page at a time through a custom `Pager`, safely shareable across threads via a hand-built spinlock.

The public interface is `KvDb<S, LockState>` — `open` / `put` / `get` / `delete` / `range` / `scan` / `len`, with `lock()`/`unlock()` enforcing at compile time when mutation is allowed. It's built on top of `BTree` (in the `btree` crate), `SpinLock` (in the `spinlock` crate), and `LendingIterator`/`Scan` (in the `scan` crate), but those are internal building blocks — `KvDb` is the intended entry point.

## Motivation

This project exists to go past "I can use a database" and into "I understand what a database actually does under the hood." A few things specifically:

- **Layout, alignment, and raw memory concerns** that only show up once data actually has to survive a process restart, not just live in a `HashMap`.
- **Why durability is hard.** A `HashMap` makes no promise about what happens after a crash. This project's entire reason to exist is the promise a `HashMap` structurally cannot make: writes should be recoverable after the process that made them is gone.
- **Real concurrency, not just the appearance of it.** Building a lock from raw atomics, and discovering (and fixing) real deadlocks along the way, rather than reaching for `std::sync::RwLock` and taking correctness for granted.
- **Rust's type system as a design tool** — typestate for compile-time state tracking, a closed `Value` enum for compile-time-checked, self-describing storage instead of an opaque generic, and a hand-rolled `LendingIterator` for iteration that can't be expressed with `std::iter::Iterator` alone.

## Design Decisions

### Why a B-tree, not a hashmap

A hash-based store can't answer range queries (give me everything between these two keys) at all — hashing destroys ordering. A B-tree keeps keys sorted, so range scans are a natural traversal instead of an impossibility. It's also shaped for disk specifically: each node holds many keys per page (not one key per pointer hop, like a binary tree), so a lookup costs a handful of page reads instead of a chain of random-access hops — the same reason SQLite, LMDB, and `redb` all use a B-tree (or B+tree) rather than a balanced binary tree for on-disk indexes.

### Page-based disk storage

Every node lives at a fixed-size (4KB) slot in a single file, addressed by a `PageId` (a plain `u64` offset) rather than an in-memory pointer. A `Pager` owns the file and handles serialization (via `serde` + `bincode`) and page allocation. This is a deliberate step up from an earlier in-memory version of this same B-tree (`Rc<RefCell<Node>>`) — the migration from pointer-based to page-based storage is itself part of what this project is meant to demonstrate: everything that was a `.borrow()`/`.borrow_mut()` in memory became a `read_page`/`write_page` round-trip to disk, with the tree algorithm itself (search, split, delete) staying identical underneath.

### The typestate pattern

`BTree<S, State, LockState>` uses two phantom-typed state parameters:

- **`Uninitialized` / `Initialized`** — `get`/`put`/`delete`/`range` are only defined in an `impl` block scoped to `BTree<S, Initialized, _>`. Calling them on an uninitialized tree isn't a runtime error, a panic, or an `unwrap()` on `None` — it's a method that doesn't exist for that type, caught by the compiler at the call site.
- **`Locked` / `Unlocked`** — mutating methods (`put`, `delete`) only exist on `BTree<S, Initialized, Unlocked>`; `unlock()`/`lock()` consume `self` and return a differently-typed tree, so the transition is enforced the same way — at compile time, not at runtime. The intent is safety-by-default: a handle you're only using to read shouldn't be _able_ to accidentally mutate — locking isn't something you opt into to protect data, it's the default you have to deliberately opt out of (`unlock()`) before mutation becomes possible at all.

`KvDb<S, LockState>` forwards this exact same protocol rather than hiding it: `open()` returns an `Unlocked` `KvDb` by default (so ordinary usage — `open`, `put`, `get` — needs no ceremony at all), but `put`/`delete` are only defined for `KvDb<S, Unlocked>`, `get`/`range`/`scan`/`len` work in either state, and `lock()`/`unlock()` consume `self` and return a differently-typed `Self`.

### Typed values via `Value`

Values are a closed, `#[non_exhaustive]` enum:

```rust
pub enum Value {
    I8(i8), I32(i32), I64(i64),
    UInt8(u8), UInt32(u32), UInt64(u64),
    F32(f32), F64(f64),
    Char(char),
    Text(String),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Pair(Vec<Value>, Vec<Value>),
}
```

The public API hides the enum at both ends rather than making callers construct or match on it directly:

- **`put`** takes `impl Into<Value>` — `db.put(1, 100)` works without writing `Value::I64(100)`.
- **`get<R>`** is generic over the return type, bounded by `TryFrom<Value, Error = ValueError>` — `let name: String = db.get(&key)?;` extracts and type-checks in one step. `ValueError::NotFound` covers a missing key; `ValueError::TypeMismatch` covers asking for the wrong type. Numeric extraction widens safely within a signedness/float family (an `I32` value can be read back as an `i64` without a cast) but never silently crosses signed↔unsigned or loses float precision.

`List`/`Pair` make `Value` recursive — a list can contain another list, or a pair of lists — which round-trips through the existing `serde`/`bincode` path with no special-casing needed.

### Real concurrency via a hand-rolled spinlock

`Pager` and `root_id` live together in `PagerState`, wrapped in a `SpinLock<T>` (its own crate, `spinlock`) — `AtomicBool`-based, `compare_exchange` for acquire, a `store` for release, `UnsafeCell<T>` holding the guarded data, and an RAII guard whose `Drop` releases the lock automatically. `unsafe impl Send`/`Sync` is written and justified explicitly, not assumed.

`BTree`/`KvDb` hold this behind an `Arc`, so multiple handles — potentially on different threads — can safely share the same underlying storage. Every public entry point (`put`/`get`/`delete`/`range`/`scan`/`len`) acquires the lock exactly once per call and reads `root_id` fresh from the shared state rather than from a per-clone cached copy — an earlier version cached `root_id` on each `BTree`/`KvDb` clone independently, which could silently go stale after a concurrent split. A separate, earlier bug (fixed first): a version that acquired the lock once per internal helper function, rather than once per public call, deadlocked reliably on any operation that triggered a B-tree split, since a function would try to re-acquire a lock its own caller already held.

### Zero-copy scanning via `LendingIterator`

`range()` returns an owned `Vec<(S, Value)>` — simple, but every key and value gets cloned to build it, even if the caller only wants to iterate once and discard most of the data. `scan()` (in the `scan` crate) avoids that: it borrows each key/value directly out of whichever page is currently loaded, rather than cloning into a fresh collection.

This can't be expressed with `std::iter::Iterator`, because that trait's `Item` type can't borrow from the iterator itself across calls to `next()` — Rust's standard iterator protocol assumes each item is either owned or borrows from something living *outside* the iterator. `scan()`'s items borrow from the iterator's own internal page cache, which changes on every call. That requires a **lending iterator**, implemented here as a small hand-written trait using a Generic Associated Type:

```rust
pub trait LendingIterator {
    type Item<'a> where Self: 'a;
    fn next<'a>(&'a mut self) -> Option<Self::Item<'a>>;
}
```

`ScanIter<S>` implements this with `type Item<'a> = (&'a S, &'a Value)`, walking the tree with an explicit stack (rather than recursion, since `next()` calls can't recurse across separate invocations) and interleaving "descend into child" with "yield this key" in the same order as an in-order traversal.

Because `LendingIterator` isn't `std::iter::Iterator`, `for` loops, `.map()`/`.filter()`/`.collect()`, and everything else in `std::iter` don't work on it directly — a `scan()` loop is written by hand with `while let Some((k, v)) = iter.next() { ... }`. That ergonomics cost is the deliberate tradeoff for the zero-copy guarantee; `range()` stays available (and is now implemented in terms of `scan()` internally) for callers who'd rather pay for the clones and get a plain `Vec` back.

## How to Use

```rust
let mut db = KvDb::<i32>::open("data.db"); // Unlocked by default

db.put(1, "hello".to_string());
db.put(2, 42i64);
db.put(3, vec![1u8, 2, 3]);

let greeting: String = db.get(&1)?;
let number: i64 = db.get(&2)?;
let bytes: Vec<u8> = db.get(&3)?;

let (found, old_value) = db.delete(1);

let all_entries = db.range(); // Vec<(S, Value)>, sorted by key, cloned

// zero-copy iteration — requires LendingIterator in scope
use kvdb::LendingIterator;
let mut iter = db.scan();
while let Some((key, value)) = iter.next() {
    println!("{key}: {value:?}");
}

// lock()/unlock() consume self and return a differently-typed Self —
// a method can't retroactively change its own caller's static type.
let db = db.lock();
let number: i64 = db.get(&2)?; // get still works locked
// db.put(4, "!"); // would not compile — put only exists on Unlocked

let mut db = db.unlock();
db.put(4, "!".to_string());
```

Values round-trip through `List`/`Pair` too:

```rust
let mixed = Value::List(vec![
    Value::I32(1),
    Value::Text("nested".to_string()),
]);
db.put(5, mixed);
let value: Vec<Value> = db.get(&5)?;
```

## Workspace Layout

```text
kvdb/
  btree/     - the B-tree algorithm and typestate API
  spinlock/  - the hand-rolled concurrency primitive
  scan/      - the LendingIterator trait and zero-copy Scan/ScanIter
  src/       - KvDb, the public wrapper, and Value/ValueError
```

`KvDb` is the intended way to use this project. `BTree`, `SpinLock`, and `scan` are internal building blocks it's composed from, not separate public-facing interfaces — `kvdb` re-exports what's needed from them (e.g. `LendingIterator`) so consumers never need to depend on the internal crates directly.

## Current Limitations

- No `fsync` on write — pages are flushed to the OS page cache, not guaranteed durable against a power loss immediately after a write.
- The root page ID isn't persisted across restarts, so reopening an existing file doesn't yet recover previous data.
- No free list — pages freed by merges or root-shrinking become permanent dead space in the file.
- I/O and page (de)serialization failures inside `Pager`/`BTree` still panic via `.expect(...)` rather than returning a `Result`; `get`'s `ValueError`-based error handling is the one path already fully `Result`-based.
- Locking is coarse-grained (one `SpinLock` guards the whole `Pager`, not true per-page "crabbing"), and the concurrent test suite currently covers multi-reader/single-writer only.
- The wire format is hard-coded to `bincode` — a pluggable `Codec` trait (to support e.g. JSON, or a future format migration) is planned but not yet implemented.
