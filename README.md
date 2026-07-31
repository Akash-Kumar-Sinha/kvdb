# KvDB

A disk-backed, embedded key-value store built from scratch in Rust — a B-tree storage engine with a page-based on-disk format, and a compile-time-enforced usage protocol via the typestate pattern.

## Table of Contents

- [Introduction](#introduction)
- [Motivation](#motivation)
- [Design Decisions](#design-decisions)
  - [Why a B-tree, not a hashmap](#why-a-b-tree-not-a-hashmap)
  - [Page-based disk storage](#page-based-disk-storage)
  - [The typestate pattern](#the-typestate-pattern)
- [How to Use](#how-to-use)
  - [Basic usage — `KvDb`](#basic-usage--kvdb)
  - [Using `BTree` directly](#using-btree-directly)
- [Current Limitations](#current-limitations)

## Introduction

KvDB is an embedded key-value store — not a server you connect to over a socket, but a library you link directly into your program, the same relationship SQLite or `sled` have to the process using them. Keys and values are generic (`S`, `T`), stored on disk in a B-tree, and read/written a fixed-size page at a time through a custom `Pager`.

It exposes two layers:
- `KvDb<S, T, LockState>` — a minimal `get` / `put` / `delete` / `range` API that mirrors `BTree`'s own compile-time `Locked`/`Unlocked` protocol, so the guarantee holds at the public surface too, not just internally.
- `BTree<S, T, State, LockState>` — the underlying tree, with the full compile-time-checked initialization and usage protocol, for anyone who wants the lowest-level API directly.

## Motivation

This project exists to go past "I can use a database" and into "I understand what a database actually does under the hood." Three things specifically:

- **`Layout`, alignment, and raw memory concerns** that only show up once data actually has to survive a process restart, not just live in a `HashMap`.
- **Why durability is hard.** A `HashMap` makes no promise about what happens after a crash. This project's entire reason to exist is the promise a `HashMap` structurally cannot make: writes should be recoverable after the process that made them is gone.
- **Rust's type system as a design tool**, not just a compiler-appeasement exercise — specifically, whether compile-time state tracking (typestate) can catch real API-misuse bugs before they ever run.

## Design Decisions

### Why a B-tree, not a hashmap

A hash-based store can't answer range queries (`give me everything between these two keys`) at all — hashing destroys ordering. A B-tree keeps keys sorted, so range scans are a natural traversal instead of an impossibility. It's also shaped for disk specifically: each node holds many keys per page (not one key per pointer hop, like a binary tree), so a lookup costs a handful of page reads instead of a chain of random-access hops — the same reason SQLite, LMDB, and `redb` all use a B-tree (or B+tree) rather than a balanced binary tree for on-disk indexes.

### Page-based disk storage

Every node lives at a fixed-size (4KB) slot in a single file, addressed by a `PageId` (a plain `u64` offset) rather than an in-memory pointer. A `Pager` owns the file and handles serialization (via `serde` + `bincode`) and page allocation. This is a deliberate step up from an earlier in-memory version of this same B-tree (`Rc<RefCell<Node>>`) — the migration from pointer-based to page-based storage is itself part of what this project is meant to demonstrate: everything that was a `.borrow()`/`.borrow_mut()` in memory became a `read_page`/`write_page` round-trip to disk, with the tree algorithm itself (search, split, delete) staying identical underneath.

### The typestate pattern

`BTree<S, T, State, LockState>` uses two phantom-typed state parameters:

- **`Uninitialized` / `Initialized`** — `get`/`put`/`delete`/`range` are only defined in an `impl` block scoped to `BTree<S, T, Initialized, _>`. Calling them on an uninitialized tree isn't a runtime error, a panic, or an `unwrap()` on `None` — it's a method that doesn't exist for that type, caught by the compiler at the call site.
- **`Locked` / `Unlocked`** — mutating methods (`put`, `delete`) only exist on `BTree<S, T, Initialized, Unlocked>`; `unlock()`/`lock()` consume `self` and return a differently-typed tree, so the transition is enforced the same way — at compile time, not at runtime.

**Worth stating plainly:** `Locked`/`Unlocked` here is a self-imposed *sequencing discipline* — a way to force "you must explicitly opt in before you can mutate" — not a concurrency primitive. Nothing about it makes the store safe to share across threads; the underlying `Pager` isn't `Send`/`Sync`. It's the same shape as `std::sync::Mutex`'s locked/unlocked states, minus the actual runtime synchronization — a deliberate design choice to demonstrate the typestate pattern itself, not a claim about thread safety.

`KvDb<S, T, LockState>` forwards this exact same protocol rather than hiding it: `open()` returns an `Unlocked` `KvDb` by default (so ordinary usage — `open`, `put`, `get` — needs no ceremony at all), but `put`/`delete` are only defined for `KvDb<S, T, Unlocked>`, `get`/`range`/`len` work in either state, and `lock()`/`unlock()` consume `self` and return a differently-typed `Self`, exactly mirroring `BTree`. The compile-time guarantee holds at the public API surface, not just internally in `BTree`.

## How to Use

### Basic usage — `KvDb`

```rust
let mut db = KvDb::<i32, String>::open("data.db"); // Unlocked by default

db.put(1, "hello".to_string());
db.put(2, "world".to_string());

assert_eq!(db.get(&1), Some("hello".to_string()));

let (found, old_value) = db.delete(1);

let all_entries = db.range(); // sorted by key

// lock()/unlock() consume self and return a differently-typed Self —
// same shadowing pattern as BTree, and the same reason it's required:
// a method can't retroactively change its own caller's static type.
let db = db.lock();
assert_eq!(db.get(&2), Some("world".to_string())); // get still works locked
// db.put(3, "!".to_string()); // would not compile — put only exists on Unlocked

let mut db = db.unlock();
db.put(3, "!".to_string());
```

### Using `BTree` directly

```rust
let tree = BTree::<i32, String, Uninitialized, Locked>::new("data.db");
let mut tree = tree.unlock(); // put/delete only exist past this point

tree.put(1, "hello".to_string());
tree.put(2, "world".to_string());

let tree = tree.lock(); // get still works locked or unlocked
assert_eq!(tree.get(&1), Some("hello".to_string()));
```

## Current Limitations

- No `fsync` on write — pages are flushed to the OS page cache, not guaranteed durable against a power loss immediately after a write.
- The root page ID isn't persisted across restarts, so reopening an existing file doesn't yet recover previous data.
- No free list — pages freed by merges or root-shrinking become permanent dead space in the file.
- I/O and (de)serialization failures currently panic via `.expect(...)` rather than returning a `Result`.

---

We're currently working on a Write-ahead log (WAL) for durability.