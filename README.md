# KvDB

A disk-backed, embedded key-value store built from scratch in Rust — a B-tree storage engine with a page-based on-disk format, a compile-time-enforced typestate API, real thread-safe concurrency via a hand-rolled spinlock, a zero-copy scanning API built on a hand-rolled `LendingIterator`, and an optional async layer.

## Table of Contents

- [Introduction](#introduction)
- [Motivation](#motivation)
- [Design Decisions](#design-decisions)
  - [Why a B-tree, not a hashmap](#why-a-b-tree-not-a-hashmap)
  - [Page-based disk storage](#page-based-disk-storage)
  - [The typestate pattern](#the-typestate-pattern)
  - [Typed values via `Value`](#typed-values-via-value)
  - [`put` vs. `update`: insert-only by default](#put-vs-update-insert-only-by-default)
  - [Real concurrency via a hand-rolled spinlock](#real-concurrency-via-a-hand-rolled-spinlock)
  - [Zero-copy scanning via `LendingIterator`](#zero-copy-scanning-via-lendingiterator)
  - [Async access via a hand-rolled `Future`](#async-access-via-a-hand-rolled-future)
- [`KvDb` — the synchronous API](#kvdb--the-synchronous-api)
  - [Constructing](#constructing)
  - [Writing](#writing)
  - [Reading](#reading)
  - [Iterating](#iterating)
  - [Lock state transitions](#lock-state-transitions)
- [`AsyncKvDb` — the async API](#asynckvdb--the-async-api)
  - [Constructing an async handle](#constructing-an-async-handle)
  - [Awaiting operations](#awaiting-operations)
  - [Async iteration](#async-iteration)
  - [How it differs from `KvDb`](#how-it-differs-from-kvdb)
- [Values and conversions](#values-and-conversions)
- [Workspace Layout](#workspace-layout)
- [Current Limitations](#current-limitations)

## Introduction

KvDB is an embedded key-value store — not a server you connect to over a socket, but a library you link directly into your program, the same relationship SQLite or `sled` have to the process using them. Keys are generic (`S`), values are stored as a typed `Value` enum, and everything is read/written a fixed-size page at a time through a custom `Pager`, safely shareable across threads via a hand-built spinlock.

There are exactly two types you interact with:

| Type | Crate | Use when |
|------|-------|----------|
| `KvDb<S, LockState>` | `kvdb` | Ordinary synchronous access. |
| `AsyncKvDb<S, LockState>` | `async_kvdb` | You want DB calls to not block the calling thread. |

Everything else — `BTree`, `SpinLock`, `Pager`, `Scan`, `KvdbCall` — is an internal building block. `kvdb` re-exports the handful of items you actually need (`KvDb`, `Value`, `ValueError`, `ScanIter`, `LendingIterator`), and internal types are either private or marked `#[doc(hidden)]`, so you never depend on the internal crates directly.

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
- **`Locked` / `Unlocked`** — mutating methods (`put`, `delete`) only exist on `BTree<S, Initialized, Unlocked>`; `unlock()`/`lock()` consume `self` and return a differently-typed tree, so the transition is enforced the same way — at compile time, not at runtime. The intent is safety-by-default: a handle you're only using to read shouldn't be _able_ to accidentally mutate.

`KvDb<S, LockState>` forwards this exact same protocol rather than hiding it: `open()` returns an `Unlocked` `KvDb` by default (so ordinary usage needs no ceremony), `put`/`delete` are only defined for `KvDb<S, Unlocked>`, `get`/`range`/`scan`/`len` work in either state, and `lock()`/`unlock()` consume `self` and return a differently-typed `Self`.

### Typed values via `Value`

Values are a closed, `#[non_exhaustive]` enum, so what's on disk is self-describing rather than an opaque blob. The public API hides the enum at both ends: `put` takes `impl Into<Value>` so `db.put(1, 100i64)` works without writing `Value::I64(100)`, and `get<R>` is generic over the return type so `let name: String = db.get(&key)?;` extracts and type-checks in one step. See [Values and conversions](#values-and-conversions) for the full type table and the exact-match rule.

### `put` vs. `update`: insert-only by default

`put` always inserts — calling it twice with the same key stores two entries, not one. That's a deliberate default, not an oversight: `put` only ever walks down to a leaf and appends, so it never pays the cost of checking whether the key already exists. Anything that already knows its keys are fresh (bulk loads, append-only logs) gets the cheapest possible write path.

`update` is the separate, explicit opt-in for upsert semantics: it walks the tree once to check whether the key exists (recursing into children, not just the root), overwrites in place if so, and falls back to `put`'s insert path if not. Splitting these into two methods, rather than making `put` always check first, means the common "I know this key is new" case never pays for a lookup it doesn't need.

### Real concurrency via a hand-rolled spinlock

`Pager` and `root_id` live together in `PagerState`, wrapped in a `SpinLock<T>` (its own crate, `spinlock`) — `AtomicBool`-based, `compare_exchange` for acquire, a `store` for release, `UnsafeCell<T>` holding the guarded data, and an RAII guard whose `Drop` releases the lock automatically. `unsafe impl Send`/`Sync` is written and justified explicitly, not assumed.

`BTree`/`KvDb` hold this behind an `Arc`, so multiple handles — potentially on different threads — can safely share the same underlying storage. Every public entry point acquires the lock exactly once per call and reads `root_id` fresh from the shared state rather than from a per-clone cached copy — an earlier version cached `root_id` on each clone independently, which could silently go stale after a concurrent split. A separate, earlier bug (fixed first): a version that acquired the lock once per internal helper function, rather than once per public call, deadlocked reliably on any operation that triggered a B-tree split, since a function would try to re-acquire a lock its own caller already held.

### Zero-copy scanning via `LendingIterator`

`range()` returns an owned `Vec<(S, Value)>` — simple, but every key and value gets cloned to build it, even if the caller only wants to iterate once and discard most of the data. `scan()` avoids that: it borrows each key/value directly out of whichever page is currently loaded.

This can't be expressed with `std::iter::Iterator`, because that trait's `Item` type can't borrow from the iterator itself across calls to `next()` — the standard iterator protocol assumes each item is either owned or borrows from something living *outside* the iterator. `scan()`'s items borrow from the iterator's own internal page cache, which changes on every call. That requires a **lending iterator**, implemented here as a small hand-written trait using a Generic Associated Type:

```rust
pub trait LendingIterator {
    type Item<'a> where Self: 'a;
    fn next(&mut self) -> Option<Self::Item<'_>>;
}
```

`ScanIter<S>` implements this with `type Item<'a> = (&'a S, &'a Value)`, walking the tree with an explicit stack (rather than recursion, since `next()` calls can't recurse across separate invocations) and interleaving "descend into child" with "yield this key" in in-order sequence. That traversal step lives in one shared function, so the sync and async walkers can't drift out of agreement about ordering.

Because `LendingIterator` isn't `std::iter::Iterator`, `for` loops, `.map()`/`.filter()`/`.collect()` don't work on it directly — a `scan()` loop is written by hand with `while let Some((k, v)) = iter.next() { ... }`. That ergonomics cost is the deliberate tradeoff for the zero-copy guarantee; `range()` stays available for callers who'd rather pay for the clones and get a plain `Vec` back.

### Async access via a hand-rolled `Future`

`kvdb_rt` is deliberately small — a `KvdbCall<R>` future and a `ThreadPoolHandle` channel alias. It is **not** an executor: it provides no run loop and no `Waker` of its own, so it composes with whatever executor you already have.

The mechanism is a manual `Future` implementation. `KvdbCall::poll` dispatches the blocking `kvdb` call to a worker thread on first poll, clones the `Waker` out of the `Context`, and returns `Pending`. The worker runs the closure, writes the result into an `Arc<Mutex<Option<R>>>` slot, and calls `.wake()`. The thread that wakes the task is therefore never the thread that polled it — which is the whole point, and the part that's genuinely easy to get wrong.

`AsyncKvDb` wraps `KvDb` rather than reimplementing it. Each method clones the handle (cheap — it's an `Arc` bump), moves the clone into a `Send + 'static` closure, and hands that to the pool.

`scan()` is the one place this costs something real. The sync `ScanIter` borrows `&S`/`&Value` straight out of the page currently loaded — but a `KvdbCall` job closure must be `Send + 'static` to cross into the pool, and a borrow tied to `&mut self` can't satisfy that. So `AsyncScanIter::next()` clones the key and value once per item and hands back `(S, Value)` instead of `(&S, &Value)`, trading the zero-copy guarantee for the ability to dispatch each traversal step off the calling thread at all.

`lock()`/`unlock()` stay synchronous, deliberately: they're pure type-level relabeling with no I/O, so dispatching them to the pool would add overhead for no parallel work.

## `KvDb` — the synchronous API

```rust
use kvdb::{KvDb, LendingIterator, Value, ValueError};
```

### Constructing

| Method | Signature | Notes |
|--------|-----------|-------|
| `open` | `fn open(path: &str) -> KvDb<S, Unlocked>` | Creates the file if absent, opens it if present. Returns an **`Unlocked`** handle, so you can write immediately. |
| `clone` | `fn clone(&self) -> Self` | Clones the *handle*, not the data. All clones share one `Arc<SpinLock<PagerState>>` — hand a clone to another thread to share the same database. |

```rust
let mut db = KvDb::<i32>::open("data.db");
```

`S` is the key type and must be `Ord + Clone + Serialize + DeserializeOwned`. Turbofish it on `open` (as above) or let inference pick it up from your first `put`.

### Writing

Only available on `KvDb<S, Unlocked>` — on a `Locked` handle these methods do not exist, and the call fails to compile.

| Method | Signature | Returns |
|--------|-----------|---------|
| `put` | `fn put(&mut self, key: S, value: impl Into<Value>)` | Nothing. **Always inserts**, by design — see [`put` vs. `update`](#put-vs-update-insert-only-by-default). |
| `update` | `fn update(&mut self, key: S, value: impl Into<Value>)` | Nothing. Overwrites the value if the key exists anywhere in the tree, otherwise inserts it. |
| `delete` | `fn delete(&mut self, key: S) -> (bool, Option<Value>)` | `(found, previous_value)`. Missing key gives `(false, None)`. |

```rust
db.put(1, "hello".to_string());
db.put(2, 42i64);
db.put(3, vec![1u8, 2, 3]);

db.update(2, 43i64);           // overwrites key 2 in place
db.update(4, "new".to_string()); // key 4 doesn't exist yet, so this inserts it

let (found, old) = db.delete(1);
assert!(found);
```

`impl Into<Value>` is what lets you pass `42i64` instead of `Value::I64(42)`. Any type with a `From<T> for Value` impl works — see the [conversion table](#values-and-conversions).

Use `put` when you know the key is new (or duplicates are fine — e.g. an append-only log); use `update` for upsert semantics. `update` costs one extra tree walk to check for the key before deciding whether to overwrite or insert, so it is strictly more expensive than `put`.

### Reading

Available on **both** `Locked` and `Unlocked` handles.

| Method | Signature | Returns |
|--------|-----------|---------|
| `get<R>` | `fn get<R>(&mut self, key: &S) -> Result<R, ValueError>` | The value converted to `R`. Borrows the key. |
| `range` | `fn range(&mut self) -> Vec<(S, Value)>` | Every entry, sorted by key, cloned into a fresh `Vec`. |
| `len` | `fn len(&mut self) -> usize` | Entry count. Walks the whole tree — it is **not** O(1). |
| `is_empty` | `fn is_empty(&mut self) -> bool` | `len() == 0`, with the same full-walk cost. |

```rust
let greeting: String = db.get(&1)?;
let number: i64 = db.get(&2)?;
let bytes: Vec<u8> = db.get(&3)?;
```

The return type drives the conversion: annotate the binding (or turbofish `get::<String>(&1)`) and `R` is inferred. Two failure modes, both `ValueError`:

- `ValueError::NotFound` — no entry for that key.
- `ValueError::TypeMismatch` — the entry exists but isn't the type you asked for.

`ValueError` implements `std::error::Error` and `Display`, so it works with `?` into a `Box<dyn Error>` or `anyhow`.

These take `&mut self` because acquiring the spinlock needs mutable access to the guarded `Pager` — not because they mutate the logical contents.

### Iterating

| Method | Signature | Returns |
|--------|-----------|---------|
| `scan` | `fn scan(&self) -> ScanIter<S>` | A zero-copy in-order cursor. Takes `&self`, not `&mut self`. |

`ScanIter` implements `LendingIterator`, **not** `Iterator` — so the trait must be in scope, and you drive it with `while let`:

```rust
use kvdb::LendingIterator;

let mut iter = db.scan();
while let Some((key, value)) = iter.next() {
    println!("{key}: {value:?}");
}
```

`key` and `value` are `&S` and `&Value`, borrowed from the page the iterator currently holds. They're valid until the next `next()` call — which is exactly why this can't be an `Iterator`, and why `.map()`/`.collect()` aren't available. Use `range()` when you want an owned `Vec` and don't mind the clones.

### Lock state transitions

| Method | Signature | Available on |
|--------|-----------|--------------|
| `lock` | `fn lock(self) -> KvDb<S, Locked>` | `Unlocked` |
| `unlock` | `fn unlock(self) -> KvDb<S, Unlocked>` | `Locked` |

Both **consume** `self` and return a differently-typed handle — a method can't retroactively change its own caller's static type, so the new binding is how the state change is recorded.

```rust
let db = db.lock();
let number: i64 = db.get(&2)?;   // reads still work
// db.put(4, "!".to_string());   // compile error: no method `put` on KvDb<i32, Locked>

let mut db = db.unlock();
db.put(4, "!".to_string());      // fine again
```

## `AsyncKvDb` — the async API

```rust
use async_kvdb::{AsyncKvDb, Value, ValueError};
```

A separate crate, so synchronous-only users never pull in the thread-pool machinery. `async_kvdb` depends on `kvdb`; `kvdb` does not depend on `async_kvdb`.

### Constructing an async handle

| Method | Signature | Notes |
|--------|-----------|-------|
| `open` | `fn open(path: &str, num_workers: usize) -> AsyncKvDb<S, Unlocked>` | Opens the file **and** spawns `num_workers` worker threads that live as long as the handle. |

```rust
let db = AsyncKvDb::<i32>::open("data.db", 4); // 4 worker threads
```

`S` additionally requires `Send + 'static` here, because keys cross a thread boundary to reach the pool.

### Awaiting operations

Every data method returns a `KvdbCall<R>` — a future that does nothing until awaited. Forgetting the `.await` means the operation never runs.

| Method | Signature | Awaits to |
|--------|-----------|-----------|
| `put` | `fn put(&self, key: S, value: impl Into<Value>) -> KvdbCall<()>` | `()`. Always inserts, by design — see [`put` vs. `update`](#put-vs-update-insert-only-by-default). |
| `update` | `fn update(&self, key: S, value: impl Into<Value>) -> KvdbCall<()>` | `()`. Overwrites if the key exists, inserts otherwise. |
| `delete` | `fn delete(&self, key: S) -> KvdbCall<(bool, Option<Value>)>` | `(found, previous_value)` |
| `get<R>` | `fn get<R>(&self, key: S) -> KvdbCall<Result<R, ValueError>>` | `Result<R, ValueError>` |
| `range` | `fn range(&self) -> KvdbCall<Vec<(S, Value)>>` | Every entry, sorted, cloned |
| `len` | `fn len(&self) -> KvdbCall<usize>` | Entry count |
| `is_empty` | `fn is_empty(&self) -> KvdbCall<bool>` | `len() == 0`, same full-walk cost |

`put`/`update`/`delete` exist only on `AsyncKvDb<S, Unlocked>`; `get`/`range`/`len`/`scan` work in either lock state — the same typestate split as the sync API.

```rust
db.put(1, "hello".to_string()).await;
db.update(1, "hello, updated".to_string()).await; // overwrites key 1

let value: String = db.get(1).await?;
let (found, old) = db.delete(1).await;
let all = db.range().await;
let count = db.len().await;
```

### Async iteration

| Method | Signature | Notes |
|--------|-----------|-------|
| `scan` | `fn scan(&self) -> AsyncScanIter<S>` | Cursor whose `next()` returns a future. |
| `AsyncScanIter::next` | `fn next(&mut self) -> NextCall<'_, S>` | Awaits to `Option<(S, Value)>` — **owned**, not borrowed. |

```rust
let mut iter = db.scan();
while let Some((key, value)) = iter.next().await {
    println!("{key}: {value:?}");
}
```

No trait import needed here — `next` is an inherent method, unlike the sync side's `LendingIterator`.

### How it differs from `KvDb`

Every method on `KvDb` has a counterpart on `AsyncKvDb`. Where they differ, it falls out of the thread-boundary crossing:

| | `KvDb` | `AsyncKvDb` | Why |
|---|---|---|---|
| Receiver | `&mut self` | `&self` | Async methods clone the handle internally before moving it into the job closure. |
| `get` key | `&S` (borrowed) | `S` (owned) | The job closure must be `Send + 'static`; a borrow can't satisfy that. |
| `scan` item | `(&S, &Value)` | `(S, Value)` | Same reason — so the async walker clones one key/value per item, giving up the zero-copy guarantee. |
| Iteration trait | `LendingIterator` | inherent `next` | The async item is owned, so no lending machinery is needed. |
| `lock`/`unlock` | sync | sync | Pure type-level relabeling; no I/O to offload. |
| `put`/`update`/`is_empty` | sync | async | Fully symmetric with the sync API — each just dispatches the same tree walk to the pool instead of running it on the calling thread. |

## Values and conversions

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

| Rust type you `put` | Stored as | Read back as |
|---------------------|-----------|--------------|
| `i8` / `i32` / `i64` | `I8` / `I32` / `I64` | same type |
| `u8` / `u32` / `u64` | `UInt8` / `UInt32` / `UInt64` | same type |
| `f32` / `f64` | `F32` / `F64` | same type |
| `char` | `Char` | `char` |
| `String` | `Text` | `String` |
| `Vec<u8>` | `Bytes` | `Vec<u8>` |
| `Vec<Value>` | `List` | `Vec<Value>` |
| `(Vec<Value>, Vec<Value>)` | `Pair` | `(Vec<Value>, Vec<Value>)` |

**Conversions are exact-match.** Reading a value back as any type other than the one it was stored as returns `ValueError::TypeMismatch` — there is no widening, so an `i32` you stored is not readable as an `i64`:

```rust
db.put(1, 42i32);
let ok:  i32 = db.get(&1)?;               // fine
let bad: Result<i64, _> = db.get(&1);     // Err(ValueError::TypeMismatch)
```

Store the width you intend to read, or convert at the call site. `List`/`Pair` make `Value` recursive — a list can contain another list, or a pair of lists — which round-trips through the existing `serde`/`bincode` path with no special-casing:

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
  btree/       - the B-tree algorithm, typestate API, Value, and ValueError
  spinlock/    - the hand-rolled concurrency primitive
  scan/        - the LendingIterator trait and the shared in-order traversal
  kvdb_rt/     - the KvdbCall future and thread-pool handle
  async_kvdb/  - AsyncKvDb, wrapping KvDb with kvdb_rt
  src/         - KvDb, the public sync entry point
```

`KvDb` is the intended entry point for synchronous use, `AsyncKvDb` for async. The other four crates are internal: items that must be `pub` for a sibling crate to compile are marked `#[doc(hidden)]`, and struct fields that were previously public (`KvDb::inner`, `BTree::pager_state`, `ScanIter`'s fields) are now private behind accessors.

## Current Limitations

- The root page ID isn't persisted across restarts, so reopening an existing file doesn't yet recover previous data.
- No free list — pages freed by merges or root-shrinking become permanent dead space in the file.
- I/O and page (de)serialization failures inside `Pager`/`BTree` still panic via `.expect(...)` rather than returning a `Result`; `get`'s `ValueError` path is the one already fully `Result`-based.
- Value conversions are exact-match with no widening, so changing a field's width is a breaking change for existing data.
- Locking is coarse-grained (one `SpinLock` guards the whole `Pager`, not true per-page "crabbing"), and the concurrent test suite currently covers multi-reader/single-writer only.
- The wire format is hard-coded to `bincode` — a pluggable `Codec` trait is planned but not yet implemented.
- The test suite drives futures with its own minimal, busy-polling `block_on` and a no-op `Waker` (see [Async access via a hand-rolled `Future`](#async-access-via-a-hand-rolled-future) for why `kvdb_rt` itself has no executor) — fine for tests, but not a substitute for a real runtime like `tokio` in production.
