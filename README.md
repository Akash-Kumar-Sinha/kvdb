# KvDB

A disk-backed, embedded key-value store built from scratch in Rust — a B-tree storage engine with a page-based on-disk format, and a compile-time-enforced usage protocol via the typestate pattern.

## Table of Contents

- [Introduction](#introduction)
- [Motivation](#motivation)
- [Design Decisions](#design-decisions)
  - [Why a B-tree, not a hashmap](#why-a-b-tree-not-a-hashmap)
  - [Page-based disk storage](#page-based-disk-storage)
  - [The typestate pattern](#the-typestate-pattern)
  - [Typed values via `Value`, not a generic `T`](#typed-values-via-value-not-a-generic-t)
- [How to Use](#how-to-use)
  - [Basic usage — `KvDb`](#basic-usage--kvdb)
  - [Using `BTree` directly](#using-btree-directly)
  - [Working with `List` values](#working-with-list-values)
- [Current Limitations](#current-limitations)

## Introduction

KvDB is an embedded key-value store — not a server you connect to over a socket, but a library you link directly into your program, the same relationship SQLite or `sled` have to the process using them. Keys are generic (`S`), values are stored as a typed `Value` enum, and everything is read/written a fixed-size page at a time through a custom `Pager`.

It exposes two layers:
- `KvDb<S, LockState>` — a minimal `get` / `put` / `delete` / `range` API that mirrors `BTree`'s own compile-time `Locked`/`Unlocked` protocol, so the guarantee holds at the public surface too, not just internally.
- `BTree<S, State, LockState>` — the underlying tree, with the full compile-time-checked initialization and usage protocol, for anyone who wants the lowest-level API directly.

## Motivation

This project exists to go past "I can use a database" and into "I understand what a database actually does under the hood." A few things specifically:

- **`Layout`, alignment, and raw memory concerns** that only show up once data actually has to survive a process restart, not just live in a `HashMap`.
- **Why durability is hard.** A `HashMap` makes no promise about what happens after a crash. This project's entire reason to exist is the promise a `HashMap` structurally cannot make: writes should be recoverable after the process that made them is gone.
- **Rust's type system as a design tool**, not just a compiler-appeasement exercise — typestate for compile-time state tracking, and a closed `Value` enum for compile-time-checked, self-describing storage instead of an opaque generic.

## Design Decisions

### Why a B-tree, not a hashmap

A hash-based store can't answer range queries (`give me everything between these two keys`) at all — hashing destroys ordering. A B-tree keeps keys sorted, so range scans are a natural traversal instead of an impossibility. It's also shaped for disk specifically: each node holds many keys per page (not one key per pointer hop, like a binary tree), so a lookup costs a handful of page reads instead of a chain of random-access hops — the same reason SQLite, LMDB, and `redb` all use a B-tree (or B+tree) rather than a balanced binary tree for on-disk indexes.

### Page-based disk storage

Every node lives at a fixed-size (4KB) slot in a single file, addressed by a `PageId` (a plain `u64` offset) rather than an in-memory pointer. A `Pager` owns the file and handles serialization (via `serde` + `bincode`) and page allocation. This is a deliberate step up from an earlier in-memory version of this same B-tree (`Rc<RefCell<Node>>`) — the migration from pointer-based to page-based storage is itself part of what this project is meant to demonstrate: everything that was a `.borrow()`/`.borrow_mut()` in memory became a `read_page`/`write_page` round-trip to disk, with the tree algorithm itself (search, split, delete) staying identical underneath.

### The typestate pattern

`BTree<S, State, LockState>` uses two phantom-typed state parameters:

- **`Uninitialized` / `Initialized`** — `get`/`put`/`delete`/`range` are only defined in an `impl` block scoped to `BTree<S, Initialized, _>`. Calling them on an uninitialized tree isn't a runtime error, a panic, or an `unwrap()` on `None` — it's a method that doesn't exist for that type, caught by the compiler at the call site.
- **`Locked` / `Unlocked`** — mutating methods (`put`, `delete`) only exist on `BTree<S, Initialized, Unlocked>`; `unlock()`/`lock()` consume `self` and return a differently-typed tree, so the transition is enforced the same way — at compile time, not at runtime.

**Worth stating plainly:** `Locked`/`Unlocked` here is a self-imposed *sequencing discipline* — a way to force "you must explicitly opt in before you can mutate" — not a concurrency primitive. Nothing about it makes the store safe to share across threads; the underlying `Pager` isn't `Send`/`Sync`. It's the same shape as `std::sync::Mutex`'s locked/unlocked states, minus the actual runtime synchronization — a deliberate design choice to demonstrate the typestate pattern itself, not a claim about thread safety.

`KvDb<S, LockState>` forwards this exact same protocol rather than hiding it: `open()` returns an `Unlocked` `KvDb` by default (so ordinary usage — `open`, `put`, `get` — needs no ceremony at all), but `put`/`delete` are only defined for `KvDb<S, Unlocked>`, `get`/`range`/`len` work in either state, and `lock()`/`unlock()` consume `self` and return a differently-typed `Self`, exactly mirroring `BTree`. The compile-time guarantee holds at the public API surface, not just internally in `BTree`.

### Typed values via `Value`, not a generic `T`

Values used to be a free generic `T`, matching whatever the caller stored. That's now a closed, `#[non_exhaustive]` enum instead:

```rust
pub enum Value {
    I64(i64),
    I32(i32),
    I8(i8),
    UInt64(u64),
    UInt32(u32),
    UInt8(u8),
    F64(f64),
    F32(f32),
    Char(char),
    Text(String),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Pair(Vec<Value>, Vec<Value>),
}
```

Two reasons for the change:

- **The B-tree's own logic never inspects a value's contents** — `insert`, `split_child`, `delete` all move `Value` around as an opaque unit regardless of which variant it is, so having a concrete enum there costs nothing structurally.
- **A closed enum makes type mismatches a checked, recoverable error instead of a silent bug.** Writing an `i64` and reading it back as a `String` is now `Err(ValueError::TypeMismatch)`, not a garbage value or a panic.

The public API hides the enum at both ends rather than making callers construct or match on it directly:

- **`put`** takes `impl Into<Value>`, so `db.put(1, 100)` works without writing `Value::I32(100)` — `From<i32>`, `From<i64>`, `From<String>`, `From<&str>`, `From<Vec<u8>>`, and `From<Vec<Value>>` are all implemented.
- **`get<R>`** is generic over the return type, bounded by `TryFrom<Value, Error = ValueError>` — `let name: String = db.get(&key)?;` extracts and type-checks in one step. A `ValueError::NotFound` covers a missing key; a `ValueError::TypeMismatch` covers asking for the wrong type. `TryFrom<Value>` for `i32`/`i64` widens automatically (an `I32` value can be read back as an `i64` without a cast; the reverse checks for overflow).

`List(Vec<Value>)` makes `Value` genuinely recursive — a list can contain another list — which round-trips through the existing `serde`/`bincode` path with no special-casing needed.

## How to Use

### Basic usage — `KvDb`

```rust
let mut db = KvDb::<i32>::open("data.db"); // Unlocked by default

db.put(1, "hello".to_string());
db.put(2, "world".to_string());

let value: String = db.get(&1)?;
assert_eq!(value, "hello");

let (found, old_value) = db.delete(1);

let all_entries = db.range(); // Vec<(S, Value)>, sorted by key

// lock()/unlock() consume self and return a differently-typed Self —
// same shadowing pattern as BTree, and the same reason it's required:
// a method can't retroactively change its own caller's static type.
let db = db.lock();
let value: String = db.get(&2)?; // get still works locked
// db.put(3, "!".to_string()); // would not compile — put only exists on Unlocked

let mut db = db.unlock();
db.put(3, "!".to_string());
```

### Using `BTree` directly

```rust
let tree = BTree::<i32, Uninitialized, Locked>::new("data.db");
let mut tree = tree.unlock(); // put/delete only exist past this point

tree.put(1, "hello".to_string());
tree.put(2, "world".to_string());

let tree = tree.lock(); // get still works locked or unlocked
let value: String = tree.get(&1)?;
assert_eq!(value, "hello");
```

### Working with `List` values

```rust
let mut db = KvDb::<i32>::open("data.db");

let mixed = Value::List(vec![
    Value::I32(1),
    Value::Text("nested".to_string()),
    Value::List(vec![Value::Float(3.5)]),
]);
db.put(1, mixed);

let value: Vec<Value> = db.get(&1)?;
```

## Current Limitations

- No `fsync` on write — pages are flushed to the OS page cache, not guaranteed durable against a power loss immediately after a write.
- The root page ID isn't persisted across restarts, so reopening an existing file doesn't yet recover previous data.
- No free list — pages freed by merges or root-shrinking become permanent dead space in the file.
- I/O and page (de)serialization failures inside `Pager`/`BTree` still panic via `.expect(...)` rather than returning a `Result`. `Value` extraction (`get`) is the one path that's already fully `Result`-based (`ValueError::NotFound`/`TypeMismatch`) — propagating `Result` the rest of the way through the I/O path is the next piece of this cleanup.

---
