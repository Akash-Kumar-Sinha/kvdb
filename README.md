# KvDB

A disk-backed, embedded key-value store built from scratch in Rust — a B+tree storage engine with a page-based on-disk format, a compile-time-enforced typestate API, real thread-safe concurrency via a hand-rolled spinlock, a zero-copy scanning API built on a hand-rolled `LendingIterator`, a pluggable wire format behind an object-safe `Codec` trait, and an optional async layer.

> **A note on "B-tree" vs. "B+tree" in this document.** The two terms appear side by side throughout, and that is deliberate rather than sloppy — they refer to different things.
>
> - **The data structure is a B+tree.** Values live only in leaves, internal nodes hold routing keys and child pointers, and leaves are chained by sibling pointers. That is what the code actually does today.
> - **The names in the code still say "B-tree".** The crate is `btree/`, the type is `BTree<S, State, LockState>`, the file is `btree/src/btree.rs`. These are historical: the engine began as a plain B-tree, and renaming everything would have touched every import in the workspace for no behavioural gain.
> - **"B-tree" in prose usually means the earlier design.** This README documents the *reasons* behind each decision, so it frequently contrasts what the engine was against what it is. [Why a B+tree, not a B-tree](#why-a-btree-not-a-b-tree) is that comparison in full.
>
> So: read `BTree` in code as "the tree", and read "B-tree" in prose as either the tree family or the pre-refactor design, depending on which it is being contrasted with. Where a sentence describes how the engine behaves *now*, it says B+tree.

## Table of Contents

- [Introduction](#introduction)
- [Motivation](#motivation)
- [Design Decisions](#design-decisions)
  - [Why a B+tree, not a B-tree](#why-a-btree-not-a-b-tree)
  - [Page-based disk storage](#page-based-disk-storage)
  - [The typestate pattern](#the-typestate-pattern)
  - [Typed values via `Value`](#typed-values-via-value)
  - [`put` vs. `update`: accumulate vs. replace](#put-vs-update-accumulate-vs-replace)
  - [Real concurrency via a hand-rolled spinlock](#real-concurrency-via-a-hand-rolled-spinlock)
  - [Zero-copy scanning via `LendingIterator`](#zero-copy-scanning-via-lendingiterator)
  - [Pluggable wire formats via `Codec`](#pluggable-wire-formats-via-codec)
  - [Failures are values, not panics](#failures-are-values-not-panics)
  - [Multi-writer concurrency, and the bug it found](#multi-writer-concurrency-and-the-bug-it-found)
  - [Why `Scan` uses GATs and `Codec` uses `dyn`](#why-scan-uses-gats-and-codec-uses-dyn)
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
- [Example — a full session](#example--a-full-session)
  - [Synchronous](#synchronous)
  - [Asynchronous](#asynchronous)
- [Values and conversions](#values-and-conversions)
- [Codecs — choosing a wire format](#codecs--choosing-a-wire-format)
- [Workspace Layout](#workspace-layout)
- [Current Limitations](#current-limitations)

## Introduction

KvDB is an embedded key-value store — not a server you connect to over a socket, but a library you link directly into your program, the same relationship SQLite or `sled` have to the process using them. Keys are generic (`S`), values are stored as a typed `Value` enum, and everything is read/written a fixed-size page at a time through a custom `Pager`, safely shareable across threads via a hand-built spinlock.

There are exactly two types you interact with:

| Type | Crate | Use when |
|------|-------|----------|
| `KvDb<S, LockState>` | `kvdb` | Ordinary synchronous access. |
| `AsyncKvDb<S, LockState>` | `async_kvdb` | You want DB calls to not block the calling thread. |

Everything else — `BTree`, `SpinLock`, `Pager`, `Scan`, `KvdbCall` — is an internal building block. `kvdb` re-exports the handful of items you actually need (`KvDb`, `Value`, `ValueError`, `ScanIter`, `LendingIterator`, and the codec types `Codec`, `BincodeCodec`, `JsonCodec`, `CodecRegistry`, `Json`), so you never have to depend on the internal crates directly. Those crates are not *sealed*, though: an item that a sibling crate needs is plainly `pub`, and nothing marks it as off-limits to rustdoc. Treat the re-export list above as the supported surface — the rest is reachable, but it is not the API this project promises to keep stable.

## Motivation

Using a database as a black box teaches you its API. It doesn't teach you why a page is 4KB, why a write survives a crash, or why concurrent writers are the part everyone gets wrong. This project starts from a `HashMap` — the thing you'd reach for if ordering, durability, and concurrency didn't matter — and rebuilds, piece by piece, the reasons a real embedded store looks nothing like one.

Each piece exists because the one before it ran out of road. A hash table can't answer "everything between these two keys" without scanning it end to end, so an ordered tree replaces it — and then storing values in *every* node made `put` and `get` disagree about where a key lives, so [a B+tree](#why-a-btree-not-a-b-tree) pushes them all down into the leaves. An in-memory `Rc<RefCell<Node>>` tree evaporates the moment the process exits, so [page-based storage](#page-based-disk-storage) puts every node at a fixed offset in a file instead of behind a pointer. Sharing that file across threads needs a lock, and reaching for `std::sync::RwLock` would have made that a non-event, so it's [a spinlock built from raw atomics](#real-concurrency-via-a-hand-rolled-spinlock) instead — one that surfaced two real deadlock/race bugs during development rather than hiding them. "You called `put` before the tree was initialized" is a runtime panic in most designs; here the [typestate pattern](#the-typestate-pattern) makes it a method that doesn't exist, caught at compile time. A `Box<dyn Any>`-shaped value store hides what's actually on disk; a closed `Value` enum keeps it self-describing. Hard-coding `bincode` everywhere makes the wire format a fact about the source code instead of a choice, so it's [a pluggable `Codec`](#pluggable-wire-formats-via-codec) now — at the cost of exercising the orphan rule for real to make the second implementation legal. And a `.expect()` that takes the whole process down on a full disk or a truncated file is a design that never expected to meet a disk, so [every one of those is a `Result`](#failures-are-values-not-panics) instead.

None of this is novel — it's how SQLite, LMDB, and `redb` already work. The point of building it from scratch isn't to improve on them; it's that "a B+tree keeps keys sorted" and "a lock needs to be exception-safe" mean something different once you've written the code that makes each one true, bugs included.

## Design Decisions

### Why a B+tree, not a B-tree

This engine started as a plain B-tree, where every node — internal or leaf — stored keys *and* their values. It is now a B+tree: **values live only in leaves**, internal nodes hold nothing but routing keys and child pointers, and every leaf carries a `next` pointer to its right neighbour, chaining all the leaves into a sorted linked list.

```text
B-tree (before)                     B+tree (after)
        [20:v]                            [20]              <- routing key only
       /      \                          /    \
 [10:v]        [30:v]              [10:v]  -->  [20:v 30:v]  <- every value, and
                                                                a sibling chain
key 20 lives in the root only     key 20 is a separator *and* a real leaf entry
```

The switch was not about making lookups asymptotically faster — it was about four concrete things:

**It removes a real inconsistency in `put`.** In the B-tree, `search_node`, `update_node`, and `delete_node` all compared for equality at *every* level on the way down, because a key could be sitting in an internal node. But `insert_non_full` only ever compared for *ordering* — it never checked equality at all. So calling `put` twice with the same key behaved differently depending on where the first copy happened to land: a duplicate in the same leaf if it was in a leaf, or a silent walk straight past it into a subtree if it was in an internal node. In a B+tree every one of those four operations ends in the same place — the leaf — so they cannot disagree about where a key lives. That uniformity is what later made [accumulating `put`](#put-vs-update-accumulate-vs-replace) implementable at all: there is exactly one page a given key can be on.

**It makes deletion dramatically simpler.** A B-tree deleting an internal-node key has to find that key's in-order predecessor or successor, hoist it up into the hole, and then recursively delete *it* from the leaf it came from. That was `get_predecessor`, `get_successor`, and roughly forty lines of swap-then-recurse in `delete_node`. A B+tree never deletes from an internal node at all — it descends to the leaf and removes there, and separator keys above are simply allowed to go stale, because they are routing hints, not data. All of that code is gone; `delete_node` is now a descent and a `Vec::remove`.

**It decouples an internal node's size from the size of your values.** An internal page now encodes as `[is_leaf, keys, [], children, []]` — the values list is always empty. Before, a single large `Value::Bytes` promoted into an internal node inflated that node's page and crowded out routing keys. Worth being precise about the limit of this win: fan-out here is `MAX_KEYS = 2 * MIN_DEGREE - 1`, a hard-coded constant, so the tree is *not* automatically shallower today. What changed is that raising `MIN_DEGREE` is now a safe knob — internal node size depends only on the key type, not on what callers store.

**It turns scanning into a linked-list walk.** `range()`, `len()`, and `scan()` no longer do an in-order tree traversal. They descend once to the leftmost leaf and then follow `next` pointers. In `scan()` that replaced an explicit `Vec<(PageId, usize)>` stack — and a `step()` function that interleaved "descend into child" and "yield this key" by testing whether the cursor index was even or odd — with a single `(PageId, usize)` cursor. See [Zero-copy scanning](#zero-copy-scanning-via-lendingiterator).

The costs are real and worth naming. Separator keys are now duplicated: a key can exist both as a routing key upstairs and as a real entry in a leaf, so the tree stores slightly more keys than there are entries. Leaves need the extra `next` field, which is a fifth field in the page format. And `split_child`, `borrow_from_prev`, `borrow_from_next`, and `merge_children` each grew a leaf case and an internal case, because the two node kinds now genuinely differ — a leaf split *copies* its separator upward (the key stays in the right leaf, since that leaf still owns the value), while an internal split *moves* it.

One thing the switch deliberately did *not* change is the naming — the crate is still `btree` and the type is still `BTree<S, State, LockState>`, for the reasons in the [note at the top](#kvdb).

Because "values only in leaves" is an invariant no type in this codebase enforces, `btree/src/lib.rs` has an `invariants` test module that walks the tree from the root and asserts it directly: internal nodes carry no values, every leaf sits at the same depth, each subtree's keys stay inside the separator bounds its ancestors imply, and the `next` chain visits leaves in exactly the same order as a left-to-right descent. It runs after bulk inserts, after shuffled inserts, and after every single delete in a 134-delete sequence that forces the merge and borrow paths.

### Page-based disk storage

Every node lives at a fixed-size (4KB) slot in a single file, addressed by a `PageId` (a plain `u64` offset) rather than an in-memory pointer. A `Pager` owns the file, the page allocator, and the [`Codec`](#pluggable-wire-formats-via-codec) that turns a node into bytes — a page is a `u32` length prefix, then codec output, then zero padding. A node encodes as the five-field list `[is_leaf, keys, values, children, next]`, where a leaf leaves `children` empty and an internal node leaves `values` and `next` empty. This is a deliberate step up from an earlier in-memory version of this same tree (`Rc<RefCell<Node>>`) — the migration from pointer-based to page-based storage is itself part of what this project is meant to demonstrate: everything that was a `.borrow()`/`.borrow_mut()` in memory became a `read_page`/`write_page` round-trip to disk, with the tree algorithm itself (search, split, delete) staying identical underneath.

### The typestate pattern

`BTree<S, State, LockState>` uses two phantom-typed state parameters:

- **`Uninitialized` / `Initialized`** — `get`/`put`/`delete`/`range` are only defined in an `impl` block scoped to `BTree<S, Initialized, _>`. Calling them on an uninitialized tree isn't a runtime error, a panic, or an `unwrap()` on `None` — it's a method that doesn't exist for that type, caught by the compiler at the call site.
- **`Locked` / `Unlocked`** — mutating methods (`put`, `delete`) only exist on `BTree<S, Initialized, Unlocked>`; `unlock()`/`lock()` consume `self` and return a differently-typed tree, so the transition is enforced the same way — at compile time, not at runtime. The intent is safety-by-default: a handle you're only using to read shouldn't be _able_ to accidentally mutate.

`KvDb<S, LockState>` forwards this exact same protocol rather than hiding it: `open()` returns an `Unlocked` `KvDb` by default (so ordinary usage needs no ceremony), `put`/`delete` are only defined for `KvDb<S, Unlocked>`, `get`/`range`/`scan`/`len` work in either state, and `lock()`/`unlock()` consume `self` and return a differently-typed `Self`.

### Typed values via `Value`

Values are a closed, `#[non_exhaustive]` enum, so what's on disk is self-describing rather than an opaque blob. The public API hides the enum at both ends: `put` takes `impl Into<Value>` so `db.put(1, 100i64)` works without writing `Value::I64(100)`, and `get<R>` is generic over the return type so `let name: String = db.get(&key)?;` extracts and type-checks in one step. See [Values and conversions](#values-and-conversions) for the full type table and the exact-match rule.

### `put` vs. `update`: accumulate vs. replace

`put` **accumulates**; `update` **replaces**. One key always means one entry — the difference is what happens to the value already there.

```rust
db.put(5, 90)?;                       // stored as I32(90)
let n: i32 = db.get(&5)?;             // 90 — a single put stays a plain value

db.put(5, 100)?; db.put(5, 8)?;       // key 5 already exists, so these accumulate
let all: Vec<Value> = db.get(&5)?;    // [I32(90), I32(100), I32(8)]
assert_eq!(db.len()?, 1);             // still one key, one entry

db.update(5, 7)?;                     // replaces the whole accumulator
let n: i32 = db.get(&5)?;             // 7
```

The accumulator is its own variant, `Value::Multi(Vec<Value>)` — deliberately *not* `Value::List`. That distinction is the whole reason this design works. `List` is a value callers legitimately store, so if accumulation reused it, these two would be indistinguishable on disk:

```rust
db.put(1, Value::List(vec![Value::I32(1), Value::I32(2)]))?;  // a list the caller means
db.put(1, 99i64)?;                                            // ...now what?
```

With a shared representation, the second `put` would have no way to tell "append to the accumulator" from "the caller's value happens to be a list", and would splice `99` into the caller's own data. With `Multi`, the first `put` stores `List([1, 2])`, the second produces `Multi([List([1, 2]), I64(99)])`, and `get::<Vec<Value>>` returns two elements — the caller's list intact as one of them. `codec/tests/roundtrip.rs` pins this with `a_multi_is_not_a_list_on_the_wire`, which asserts every codec encodes the two differently.

Reading back follows from that. `Vec<Value>` accepts either `List` or `Multi`, so `get::<Vec<Value>>` is the "give me everything under this key" call regardless of how the key got there. Asking for a scalar after accumulation — `get::<i32>` on a key with three values — is a `ValueError::TypeMismatch`, not a silent pick of one of them.

**What this costs.** `put` used to insert blind: walk to a leaf, insert in sorted position, never compare for equality. It now checks the leaf for the key first, so bulk loads of known-fresh keys pay for a lookup they don't need. That is a real regression for the append-only-log case, and it buys the thing that was previously broken — before this change, four `put`s of key 5 stored four separate entries, `len()` reported 4, and `get` returned the *first* value written while the other three were reachable only through `scan()`. Since both methods now find an existing key the same way, `put` and `update` differ only in what they do once they've found it.

### Real concurrency via a hand-rolled spinlock

`Pager` and `root_id` live together in `PagerState`, wrapped in a `SpinLock<T>` (its own crate, `spinlock`) — `AtomicBool`-based, `compare_exchange` for acquire, a `store` for release, `UnsafeCell<T>` holding the guarded data, and an RAII guard whose `Drop` releases the lock automatically. `unsafe impl Send`/`Sync` is written and justified explicitly, not assumed.

`BTree`/`KvDb` hold this behind an `Arc`, so multiple handles — potentially on different threads — can safely share the same underlying storage. Every public entry point acquires the lock exactly once per call and reads `root_id` fresh from the shared state rather than from a per-clone cached copy — an earlier version cached `root_id` on each clone independently, which could silently go stale after a concurrent split. A separate, earlier bug (fixed first): a version that acquired the lock once per internal helper function, rather than once per public call, deadlocked reliably on any operation that triggered a split, since a function would try to re-acquire a lock its own caller already held.

"Once per public call" is load-bearing, and one method quietly broke it — see [Multi-writer concurrency, and the bug it found](#multi-writer-concurrency-and-the-bug-it-found).

### Zero-copy scanning via `LendingIterator`

`range()` returns an owned `Vec<(S, Value)>` — simple, but every key and value gets cloned to build it, even if the caller only wants to iterate once and discard most of the data. `scan()` avoids that: it borrows each key/value directly out of whichever page is currently loaded.

This can't be expressed with `std::iter::Iterator`, because that trait's `Item` type can't borrow from the iterator itself across calls to `next()` — the standard iterator protocol assumes each item is either owned or borrows from something living *outside* the iterator. `scan()`'s items borrow from the iterator's own internal page cache, which changes on every call. That requires a **lending iterator**, implemented here as a small hand-written trait using a Generic Associated Type:

```rust
pub trait LendingIterator {
    type Item<'a> where Self: 'a;
    fn next(&mut self) -> Option<Self::Item<'_>>;
}
```

`ScanIter<S>` implements this with `type Item<'a> = (&'a S, &'a Value)`. Because this is a [B+tree](#why-a-btree-not-a-b-tree), the walk is not a tree traversal at all: the cursor is a single `(PageId, usize)` pair that descends once to the leftmost leaf, yields that leaf's entries in order, then follows the leaf's `next` pointer to its right neighbour. There is no stack to maintain across `next()` calls, and no interleaving of "descend" and "yield" — a leaf is either not yet exhausted, or it hands the cursor to its sibling. That step lives in one shared function, so the sync and async walkers can't drift out of agreement about ordering.

Because `LendingIterator` isn't `std::iter::Iterator`, `for` loops, `.map()`/`.filter()`/`.collect()` don't work on it directly — a `scan()` loop is written by hand with `while let Some((k, v)) = iter.next() { ... }`. That ergonomics cost is the deliberate tradeoff for the zero-copy guarantee; `range()` stays available for callers who'd rather pay for the clones and get a plain `Vec` back.

### Pluggable wire formats via `Codec`

`Pager` used to call `bincode::serialize` and `bincode::deserialize` directly, which made the on-disk format a fact about the source code rather than a choice. It is now a `Box<dyn Codec>` handed to the pager at `open()` time:

```rust
pub trait Codec: Debug + Send + Sync {
    fn name(&self) -> &'static str;
    fn encode(&self, value: &Value) -> Vec<u8>;
    fn decode(&self, bytes: &[u8]) -> Result<Value, ValueError>;
    fn boxed_clone(&self) -> Box<dyn Codec>;
}
```

Two codecs ship: `BincodeCodec` (the previous behaviour, still the default, so existing code is untouched) and `JsonCodec`, which exists to prove the abstraction generalises rather than wrapping one format in a trait. They disagree about everything a format can disagree about — binary vs. text, compact vs. inspectable, fixed-width integers vs. decimal — and the same `KvDb` runs on either.

Three things are worth pulling out of the implementation:

**The trait is dyn-compatible on purpose, and that constrains its shape.** Every method takes `&self`, none is generic, and none mentions `Self` by value. A generic `fn encode<T: Serialize>(&self, value: &T)` would be more convenient and would make the trait unusable as `dyn Codec`, which is the entire point — a `CodecRegistry` maps `&str` to `Box<dyn Codec>`, so a format can come from a config file rather than a turbofish. `boxed_clone` is the visible scar: `Clone` is itself not dyn-compatible (`fn clone(&self) -> Self`), so cloning a codec out of the registry needs a hand-written object-safe equivalent.

**Nodes are encoded as a `Value`, not as a `Node<S>`.** Since a `dyn Codec` cannot accept an arbitrary `Serialize`, `btree::page` maps a node into the five-element list `[is_leaf, keys, values, children, next]` and hands *that* to the codec. Keys ride inside as `Value::Bytes` blobs, still produced by `bincode`, because `S` is whatever key type the caller chose and no dyn-safe trait can serialize it. So the codec owns the page format; key bytes stay a separate, deliberately unpluggable concern — the same split real engines make between a key encoder and a value codec.

**`JsonCodec` exercises the orphan rule for real.** The codec converts between `value::Value` and `serde_json::Value`, and in the `codec` crate *both* are foreign types, so `impl From<serde_json::Value> for Value` is rejected by coherence — there is a `compile_fail,E0117` doctest pinning that. The fix is the newtype: `Json(serde_json::Value)` is local, so `impl From<&Value> for Json` and `impl TryFrom<Json> for Value` are both legal, and every JSON encode/decode in the crate routes through it. It is load-bearing rather than illustrative: delete `Json` and the codec cannot be written.

The JSON mapping is written out by hand rather than delegating to `Value`'s derived `Serialize`, for two reasons that only matter because this is *storage*: `serde_json` writes non-finite floats as `null`, which does not read back as a float (a silent write loss), and derived tags are the Rust variant names, so renaming `Value::UInt64` would change bytes already on disk. The tags here are lowercase constants (`{"u64":1}`), non-finite floats are `"nan"`/`"inf"`/`"-inf"`, and byte strings are hex.

Property tests found one bug during this phase that no example-based test was going to: `serde_json`'s default float parser is a fast path that misreads `2.65582999060582e-301` on the way back in. The fix was its `float_roundtrip` feature — roughly 2× on float parsing, in exchange for floats that are actually the floats you stored.

### Failures are values, not panics

For most of this project's life, `Pager::read_page` and `write_page` ended in `.expect("read failed")`, and every tree helper that called them did the same. A full disk, a truncated file, or a page written by a different codec took the process down. Only `get`'s `ValueError` path — the *value*-level failures, "no such key" and "wrong type" — was ever a `Result`.

Every one of those paths now returns `Result<_, DbError>` and propagates with `?`, along the same call chain the spinlock work already threads a `&mut Pager` / `&mut PagerState` through: `put` → `insert` → `insert_locked` → `split_child` → `insert_non_full`, `delete` → `delete_node` → `fill_child` → `borrow_from_prev`/`borrow_from_next`/`merge_children`, plus `search_node`, `descend_to_leaf`, `leftmost_leaf`, `walk_leaves`, `find_len`, and `update_node`. The guard threading and the error threading follow exactly the same shape — one passes state down, the other passes failure back up.

`DbError` is the storage-level error; `ValueError` stays the value-level one and arrives wrapped in `DbError::Value`:

| Variant | Means |
|---|---|
| `Io(std::io::Error)` | The file could not be opened, sought, read, or written. Keeps the original error as its `source()`. |
| `Value(ValueError)` | `NotFound`, `TypeMismatch`, or a codec's decode failure. `err.is_not_found()` is the shortcut for the common one. |
| `CorruptPage { page, len, capacity }` | A page's length prefix claims more bytes than a page holds. This used to be an out-of-bounds slice panic. |
| `PageOverflow { len, codec, capacity }` | An encoded node does not fit in one page. This used to be an `assert!`. |
| `KeyEncode { message }` | A key type's `Serialize` impl failed. This was `write_page`'s `.expect("serialize failed")`. |

Two things deliberately did **not** become errors:

- **`allocate_page`** — despite being grouped with the other two in the original limitation, it contains no `.expect` and does no I/O. It increments a counter. Making it fallible would be inventing a failure that cannot happen.
- **Algorithmic invariants** — `left.keys.pop().expect("left sibling has spare keys")`, `children.last().expect("internal node always has children")`. Those hold because the caller just checked them; if one ever fails, this code is wrong, and a `Result` would only ask the caller to handle a bug they cannot fix. They stay `expect`, with messages that say what is being assumed.

The visible cost is that `scan()` items became `Result<(&S, &Value), DbError>`. A lending iterator has nowhere else to put a failure — the item is the only channel — and swallowing it by ending iteration early would silently truncate a scan over a broken file.

### Multi-writer concurrency, and the bug it found

The concurrency suite used to prove one thing: many readers against a single writer's data. That says nothing about two threads *writing* at once, which is where a B+tree is actually dangerous — a split rewrites three nodes, and a reader or a second writer that sees one of them mid-flight sees a tree that does not exist.

The suite now puts eight writers on one database, with interleaved keys (`i * WRITERS + writer`, not `writer * PER_WRITER + i`) so the writers collide *inside the same nodes* rather than each owning a subtree, and enough keys to force repeated splits. It checks four things: no write is lost, no value comes back wrong, `range()` stays sorted and complete, and `scan()` agrees with `range()`. There are companion tests for readers running during concurrent splits, and for concurrent deletes, which exercise the merge/borrow rebalancing paths the insert tests never touch.

**That test found a real bug on its first run**, roughly one run in fifteen. `update` is check-then-act — look the key up, then overwrite it or insert it — and it was releasing the lock between the two steps:

```rust
let updated = {
    let mut guard = self.pager_state.acquire();
    Self::update_node(&mut guard.pager, root_id, &key, &value)?
};                                  // <- lock released here
if updated { Ok(()) } else { self.insert(key, value) }   // <- and re-acquired here
```

Two threads updating the same absent key could both miss and both insert, leaving two entries under one key — a corrupted tree, from a method whose entire purpose is to avoid duplicates. The fix is not to acquire the lock twice more carefully but to acquire it once: `insert` was split into a thin `insert` that acquires and an `insert_locked` that takes the already-held `&mut PagerState`, so `update` does its lookup and its fallback insert under a single acquisition. Re-acquiring inside the helper would have deadlocked — the spinlock is not reentrant, which is the same trap the [earlier per-helper locking bug](#real-concurrency-via-a-hand-rolled-spinlock) fell into.

Because the race window is only the first update of any given key, the regression test now races on a fresh key 25 times per run rather than once. With the bug reintroduced it fails every run instead of one in fifteen — a test that catches a race 7% of the time is not a regression test.

### Why `Scan` uses GATs and `Codec` uses `dyn`

These two traits are deliberately built on opposite mechanisms, and the contrast is the useful part:

| | `LendingIterator` / `Scan` (Phase 4) | `Codec` (Phase 5) |
|---|---|---|
| Mechanism | Generic Associated Type | plain object-safe trait |
| Dispatch | static, monomorphised | dynamic, one vtable hop |
| Dyn-compatible | no | yes, by design |
| Chosen | at compile time, by type | at runtime, by name |
| Implementations | exactly one (`ScanIter<S>`) | open-ended |

**`Scan` is GAT-based because its whole purpose is a borrow the caller can't otherwise express.** `type Item<'a> = (&'a S, &'a Value)` lets each yielded item borrow from the iterator's own page buffer, which is what makes `scan()` zero-copy. A GAT is precisely a type that depends on a lifetime — and a trait with one is not dyn-compatible, because a vtable would have to store a method whose return type isn't known until the caller's lifetime is. That exclusion costs nothing here: there is one implementation, the caller always knows its concrete type, and the abstraction exists to describe a *borrow shape*, not to let implementations be swapped. Paying a vtable hop per item on the hot iteration path to gain a substitutability nobody wants would be a bad trade.

**`Codec` is object-safe because swapping implementations is the entire feature.** Nothing about a codec needs to borrow from itself: it takes `&Value`, returns `Vec<u8>`, and every method is a plain `&self` call with no generics. Making it a type parameter — `Pager<C: Codec>` — would push the format into the type of `Pager`, and from there into `BTree<S, State, LockState>`, `KvDb<S, LockState>`, and every signature that mentions them, so that choosing JSON at runtime from a config string would be impossible: the choice would have to be a compile-time literal. One vtable hop per *page* (not per item, and dwarfed by the disk I/O it precedes) buys a format that can be named at runtime.

So neither choice is a default and neither is a preference. Each trait took the tool that fits its job: `Scan` needs to express a lifetime relationship and gives up substitutability to get it; `Codec` needs substitutability and has no lifetime relationship to express. The asymmetry is the design working, not an inconsistency in it.

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
| `open` | `fn open(path: &str) -> Result<KvDb<S, Unlocked>, DbError>` | Creates the file if absent, opens it if present. Returns an **`Unlocked`** handle, so you can write immediately. Uses the default `BincodeCodec`. |
| `open_with_codec` | `fn open_with_codec(path: &str, codec: Box<dyn Codec>) -> Result<KvDb<S, Unlocked>, DbError>` | The same, with an explicitly chosen wire format — see [Codecs](#codecs--choosing-a-wire-format). |
| `clone` | `fn clone(&self) -> Self` | Clones the *handle*, not the data. All clones share one `Arc<SpinLock<PagerState>>` — hand a clone to another thread to share the same database. |

```rust
let mut db = KvDb::<i32>::open("data.db")?;
```

`S` is the key type and must be `Ord + Clone + Serialize + DeserializeOwned`. Turbofish it on `open` (as above) or let inference pick it up from your first `put`.

### Writing

Only available on `KvDb<S, Unlocked>` — on a `Locked` handle these methods do not exist, and the call fails to compile.

| Method | Signature | Returns |
|--------|-----------|---------|
| `put` | `fn put(&mut self, key: S, value: impl Into<Value>) -> Result<(), DbError>` | **Accumulates** — a second `put` on the same key folds both values into a `Value::Multi`. See [`put` vs. `update`](#put-vs-update-accumulate-vs-replace). |
| `update` | `fn update(&mut self, key: S, value: impl Into<Value>) -> Result<(), DbError>` | Overwrites the value if the key exists anywhere in the tree, otherwise inserts it. Atomic: the lookup and the fallback insert happen under one lock acquisition. |
| `delete` | `fn delete(&mut self, key: S) -> Result<(bool, Option<Value>), DbError>` | `(found, previous_value)`. Missing key gives `(false, None)`. |

```rust
db.put(1, "hello".to_string())?;
db.put(2, 42i64)?;
db.put(3, vec![1u8, 2, 3])?;

db.update(2, 43i64)?;             // overwrites key 2 in place
db.update(4, "new".to_string())?; // key 4 doesn't exist yet, so this inserts it

let (found, old) = db.delete(1)?;
assert!(found);
```

`impl Into<Value>` is what lets you pass `42i64` instead of `Value::I64(42)`. Any type with a `From<T> for Value` impl works — see the [conversion table](#values-and-conversions).

Use `put` when repeat writes to a key should pile up (an event log keyed by subject, tags on a record); use `update` when the newest write should win. Both descend to the same leaf and both cost the same lookup — they differ only in what they do with a value that is already there.

### Reading

Available on **both** `Locked` and `Unlocked` handles.

| Method | Signature | Returns |
|--------|-----------|---------|
| `get<R>` | `fn get<R>(&mut self, key: &S) -> Result<R, DbError>` | The value converted to `R`. Borrows the key. |
| `range` | `fn range(&mut self) -> Result<Vec<(S, Value)>, DbError>` | Every entry, sorted by key, cloned into a fresh `Vec`. |
| `len` | `fn len(&mut self) -> Result<usize, DbError>` | Entry count. Walks the whole leaf chain — it is **not** O(1). |
| `is_empty` | `fn is_empty(&mut self) -> Result<bool, DbError>` | `len() == 0`, with the same full-walk cost. |

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
| `scan` | `fn scan(&self) -> ScanIter<S>` | A zero-copy in-order cursor. Takes `&self`, not `&mut self`. Yields `Result` items. |

`ScanIter` implements `LendingIterator`, **not** `Iterator` — so the trait must be in scope, and you drive it with `while let`:

```rust
use kvdb::LendingIterator;

let mut iter = db.scan();
while let Some(item) = iter.next() {
    let (key, value) = item?;
    println!("{key}: {value:?}");
}
```

Each item is a `Result<(&S, &Value), DbError>`: reading the next page can fail, and a lending iterator has nowhere to report that except in the item. After an error the walk is over — the traversal stack is dropped, so the following `next()` returns `None` instead of retrying the page that just failed.

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
| `open` | `fn open(path: &str, num_workers: usize) -> Result<AsyncKvDb<S, Unlocked>, DbError>` | Opens the file **and** spawns `num_workers` worker threads that live as long as the handle. Opening runs on the calling thread, so it is a plain `Result`, not a future. |
| `open_with_codec` | `fn open_with_codec(path: &str, num_workers: usize, codec: Box<dyn Codec>) -> Result<AsyncKvDb<S, Unlocked>, DbError>` | The same, with an explicitly chosen wire format. |

```rust
let db = AsyncKvDb::<i32>::open("data.db", 4); // 4 worker threads
```

`S` additionally requires `Send + 'static` here, because keys cross a thread boundary to reach the pool.

### Awaiting operations

Every data method returns a `KvdbCall<R>` — a future that does nothing until awaited. Forgetting the `.await` means the operation never runs.

| Method | Signature | Awaits to |
|--------|-----------|-----------|
| `put` | `fn put(&self, key: S, value: impl Into<Value>) -> KvdbCall<Result<(), DbError>>` | Accumulates into a `Value::Multi` on a repeat key — see [`put` vs. `update`](#put-vs-update-accumulate-vs-replace). |
| `update` | `fn update(&self, key: S, value: impl Into<Value>) -> KvdbCall<Result<(), DbError>>` | Overwrites if the key exists, inserts otherwise. |
| `delete` | `fn delete(&self, key: S) -> KvdbCall<Result<(bool, Option<Value>), DbError>>` | `(found, previous_value)` |
| `get<R>` | `fn get<R>(&self, key: S) -> KvdbCall<Result<R, DbError>>` | The value converted to `R` |
| `range` | `fn range(&self) -> KvdbCall<Result<Vec<(S, Value)>, DbError>>` | Every entry, sorted, cloned |
| `len` | `fn len(&self) -> KvdbCall<Result<usize, DbError>>` | Entry count |
| `is_empty` | `fn is_empty(&self) -> KvdbCall<Result<bool, DbError>>` | `len() == 0`, same full-walk cost |

`put`/`update`/`delete` exist only on `AsyncKvDb<S, Unlocked>`; `get`/`range`/`len`/`scan` work in either lock state — the same typestate split as the sync API.

```rust
db.put(1, "hello".to_string()).await?;
db.update(1, "hello, updated".to_string()).await?; // overwrites key 1

let value: String = db.get(1).await?;
let (found, old) = db.delete(1).await?;
let all = db.range().await?;
let count = db.len().await?;
```

### Async iteration

| Method | Signature | Notes |
|--------|-----------|-------|
| `scan` | `fn scan(&self) -> AsyncScanIter<S>` | Cursor whose `next()` returns a future. |
| `AsyncScanIter::next` | `fn next(&mut self) -> NextCall<'_, S>` | Awaits to `Option<Result<(S, Value), DbError>>` — **owned**, not borrowed. |

```rust
let mut iter = db.scan();
while let Some(item) = iter.next().await {
    let (key, value) = item?;
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

## Example — a full session

Both listings below are real files in the repo, and the output is what they actually print:

```console
$ cargo run --example quickstart              # examples/quickstart.rs
$ cargo run -p async_kvdb --example quickstart  # async_kvdb/examples/quickstart.rs
```

### Synchronous

```rust
use kvdb::{CodecRegistry, DbError, KvDb, LendingIterator, Value, ValueError};

fn main() -> Result<(), DbError> {
    let path = "/tmp/kvdb_quickstart.db";
    let mut db = KvDb::<u32>::open(path)?;          // Unlocked, so writes work now

    // A first put stores the value as-is.
    db.put(1, "draft".to_string())?;
    let status: String = db.get(&1)?;               // "draft"

    // The key already exists, so these accumulate instead of overwriting.
    db.put(1, "reviewed".to_string())?;
    db.put(1, "published".to_string())?;
    let history: Vec<Value> = db.get(&1)?;          // all three, in arrival order
    db.len()?;                                      // still 1 — one key, one entry

    // Reading an accumulated key as a scalar is an error, not a silent pick.
    match db.get::<String>(&1) {
        Err(DbError::Value(ValueError::TypeMismatch)) => { /* read it as Vec<Value> */ }
        other => panic!("expected a type mismatch, got {other:?}"),
    }

    // update replaces whatever was there — accumulator included.
    db.update(1, "archived".to_string())?;
    let status: String = db.get(&1)?;               // "archived"

    db.put(3, "draft".to_string())?;
    db.put(2, "review".to_string())?;

    // range() clones into a Vec; scan() borrows and needs LendingIterator in scope.
    for (key, value) in db.range()? { println!("{key}: {value:?}"); }

    let mut iter = db.scan();
    while let Some(item) = iter.next() {
        let (key, value) = item?;
        println!("{key}: {value:?}");
    }

    let (found, previous) = db.delete(2)?;          // (true, Some(Text("review")))
    db.get::<String>(&99).is_err_and(|err| err.is_not_found());   // true

    // Typestate: a Locked handle has no `put` at all.
    let mut db = db.lock();
    let status: String = db.get(&3)?;               // reads still fine
    // db.put(4, "nope".to_string())?;              // error[E0599]: no method named `put`
    let mut db = db.unlock();
    db.put(4, "draft".to_string())?;

    // A different wire format, chosen at runtime by name.
    let codec = CodecRegistry::default().create("json").expect("json ships built in");
    let mut readable = KvDb::<u32>::open_with_codec("/tmp/kvdb_quickstart_json.db", codec)?;
    readable.put(7, "on disk as text".to_string())?;   // the page literally contains
                                                       // {"text":"on disk as text"}
    Ok(())
}
```

```text
doc 1                 -> draft
doc 1 after 3 puts    -> [Text("draft"), Text("reviewed"), Text("published")]
entries in the db     -> 1
doc 1 as String       -> TypeMismatch (read it as Vec<Value>)
doc 1 after update    -> archived
range                 -> 1: Text("archived")
range                 -> 2: Text("review")
range                 -> 3: Text("draft")
scan                  -> 1: Text("archived")
scan                  -> 2: Text("review")
scan                  -> 3: Text("draft")
delete(2)             -> found=true, previous=Some(Text("review"))
get(99)               -> not found: true
doc 3 while locked    -> draft
wrote doc 4 after unlock
json page holds text  -> true
```

The commented-out `put` is not decoration — uncommenting it fails to build with `error[E0599]: no method named `put` found for struct `KvDb<u32, Locked>``. That is the [typestate pattern](#the-typestate-pattern) doing its job at the call site.

### Asynchronous

Same operations, except every data method returns a future. `kvdb_rt` deliberately ships no executor (see [Async access](#async-access-via-a-hand-rolled-future)), so the example brings its own ~20-line `block_on`; in real code that would be `tokio` or `async-std`.

```rust
use async_kvdb::{AsyncKvDb, DbError, Value};

let db = AsyncKvDb::<u32>::open(path, 4)?;      // 4 worker threads

block_on(async {
    db.put(1, "draft".to_string()).await?;
    db.put(1, "published".to_string()).await?;
    let history: Vec<Value> = db.get(1).await?; // note: owned key, not &key

    db.update(2, 42i32).await?;
    db.len().await?;

    let mut iter = db.scan();                   // inherent next(), no trait import
    while let Some(item) = iter.next().await {
        let (key, value) = item?;               // owned (S, Value), not borrowed
        println!("{key}: {value:?}");
    }

    db.delete(2).await?;
    Ok::<(), DbError>(())
})?;

// A KvdbCall does nothing until awaited, so these four run on the pool concurrently.
let pending: Vec<_> = (10..14).map(|key| db.put(key, key as i32)).collect();
block_on(async {
    for call in pending { call.await?; }
    Ok::<(), DbError>(())
})?;
```

```text
doc 1 history   -> [Text("draft"), Text("published")]
doc 2           -> 42
len             -> 2
scan            -> 1: Multi([Text("draft"), Text("published")])
scan            -> 2: I32(42)
delete(2)       -> found=true, previous=Some(I32(42))
after 4 dispatched puts -> len 5
```

Two differences worth noticing in that output. `db.get(1)` takes the key **by value**, because the job closure must be `Send + 'static` and a borrow cannot satisfy that. And `scan` shows key 1 as `Multi([...])` — `scan`/`range` hand back the stored `Value` as-is, so an accumulated key surfaces as its `Multi`, whereas `get::<Vec<Value>>` unwraps it for you.

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
    Multi(Vec<Value>),   // built by repeated `put`, never by the caller
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
| *(not constructible directly)* | `Multi` | `Vec<Value>` — see [`put` vs. `update`](#put-vs-update-accumulate-vs-replace) |

**Conversions are exact-match.** Reading a value back as any type other than the one it was stored as returns `ValueError::TypeMismatch` — there is no widening, so an `i32` you stored is not readable as an `i64`:

```rust
db.put(1, 42i32)?;
let ok:  i32 = db.get(&1)?;               // fine
let bad: Result<i64, _> = db.get(&1);     // Err(DbError::Value(ValueError::TypeMismatch))
```

Store the width you intend to read, or convert at the call site. `List`/`Pair` make `Value` recursive — a list can contain another list, or a pair of lists — which round-trips through whichever codec is in use with no special-casing:

```rust
let mixed = Value::List(vec![
    Value::I32(1),
    Value::Text("nested".to_string()),
]);
db.put(5, mixed)?;
let value: Vec<Value> = db.get(&5)?;
```

## Codecs — choosing a wire format

```rust
use kvdb::{BincodeCodec, Codec, CodecRegistry, JsonCodec, KvDb};
```

A `Codec` decides what a `Value` looks like in bytes. `open()` uses `BincodeCodec`; `open_with_codec()` takes any `Box<dyn Codec>`:

```rust
let mut db = KvDb::<i32>::open_with_codec("data.db", Box::new(JsonCodec))?;
db.put(1, 42i32)?;  // the page now literally contains {"i32":42}
```

| Codec | `name()` | Format | Use when |
|-------|----------|--------|----------|
| `BincodeCodec` | `"bincode"` | compact binary | The default. Smallest pages, fastest encode. |
| `JsonCodec` | `"json"` | tagged JSON text | You want to read the file with `cat`, `jq`, or a non-Rust tool. |

`CodecRegistry` is the runtime-dispatch half — name in, codec out:

| Method | Signature | Notes |
|--------|-----------|-------|
| `default` | `fn default() -> CodecRegistry` | Holds every built-in codec, keyed by name. |
| `new` | `fn new() -> CodecRegistry` | Empty, for when you don't want the built-ins. |
| `register` | `fn register(&mut self, codec: impl Codec + 'static) -> Option<Box<dyn Codec>>` | Keyed by the codec's own `name()`. Returns whatever it replaced. |
| `get` | `fn get(&self, name: &str) -> Option<&dyn Codec>` | Borrows. |
| `create` | `fn create(&self, name: &str) -> Option<Box<dyn Codec>>` | Owns — this is what `open_with_codec` needs. |
| `names` / `codecs` | iterators | Sorted by name. |

```rust
let format = std::env::var("KVDB_FORMAT").unwrap_or_else(|_| "bincode".into());
let codec = CodecRegistry::default()
    .create(&format)
    .expect("unknown codec");
let mut db = KvDb::<i32>::open_with_codec("data.db", codec)?;
```

Writing another codec means implementing four methods and registering it — nothing else in the engine changes. The one obligation is the round-trip law, `decode(encode(v)) == Ok(v)` for every `Value`, which `codec/tests/roundtrip.rs` property-tests across *every* codec in the registry (plus a check that the generator still reaches all 14 variants, so adding a `Value` variant fails the suite until it is covered).

Two caveats, both deliberate:

- **A file does not record which codec wrote it.** Opening a bincode database as JSON succeeds, then fails at the first page read with a `DbError::Value` carrying the codec's decode error. A format tag in a file header would let `open()` catch it instead.
- **Codecs are not interchangeable mid-life.** The choice is fixed for the handle and all its clones; there is no re-encode-in-place migration.

## Workspace Layout

```text
kvdb/
  value/       - Value and ValueError, the types every other crate shares
  codec/       - the Codec trait, BincodeCodec, JsonCodec, and CodecRegistry
  btree/       - the B+tree algorithm, typestate API, Pager, and page layout
  spinlock/    - the hand-rolled concurrency primitive
  scan/        - the LendingIterator trait and the shared in-order traversal
  kvdb_rt/     - the KvdbCall future and thread-pool handle
  async_kvdb/  - AsyncKvDb, wrapping KvDb with kvdb_rt
  src/         - KvDb, the public sync entry point
```

`value` and `codec` sit *below* `btree` rather than inside it, and that layering is forced by the design rather than aesthetic: `btree` depends on `codec` (the pager holds one), so `codec` cannot depend on `btree` — which is where `Value` used to live. Moving `Value`/`ValueError` down into their own leaf crate breaks the cycle, and it is also what makes the orphan-rule problem in `JsonCodec` real: inside `codec`, `Value` is a foreign type. `btree` re-exports both, so `btree::Value` still resolves and no existing import changed.

`KvDb` is the intended entry point for synchronous use, `AsyncKvDb` for async. The other four crates are internal by convention rather than by enforcement: items that must be `pub` for a sibling crate to compile — `Pager`, `PagerState`, `BTree::pager_state()`, and `scan`'s `Cursor`/`Step`/`step`/`ScanIter::new`/`ScanIter::into_parts` — are ordinary public items. What *is* enforced is encapsulation of state: struct fields that were previously public (`KvDb::inner`, `BTree::pager_state`, `ScanIter`'s fields) are private behind accessors, so no caller can corrupt a tree by reaching into it.

## Current Limitations

> **🚧 Open for contributions.** Five ranked by how much they actually matter, then two that are contribution-sized starter issues. Pick one and open a PR.

**Priority**

1. **The root page ID isn't persisted across restarts.** `BTree::new_with_codec` (`btree/src/btree.rs:119-131`) always allocates a fresh root leaf and points `root_id` at it, whether or not the file already holds data — reopening an existing database starts a new, empty tree over bytes it can no longer reach. This is the sharpest gap against the project's own premise: surviving a restart is the entire reason a page-based tree exists instead of a `HashMap` (see [Motivation](#motivation)), and today that guarantee doesn't hold.
2. **Locking is one `SpinLock` for the whole `Pager`.** `PagerState` is guarded by a single `Arc<SpinLock<PagerState>>` (`btree/src/btree.rs:24`), so every operation on every key serializes behind the same lock. The [concurrency suite](#multi-writer-concurrency-and-the-bug-it-found) proves correctness under concurrent writers, not throughput — real write concurrency needs per-page locking ("crabbing" down the tree) instead of one global lock.
3. **`scan()` is not snapshot-isolated.** It re-acquires the lock on every `next()` (`scan/src/scan.rs:73`) instead of holding it for the whole walk, so a concurrent writer can split a node between two steps of the same scan and the walk may then skip or repeat an entry. `range()`, which holds the lock for the entire traversal, is the consistent alternative today.
4. **There is no free list.** `Pager::allocate_page` (`btree/src/pager.rs:51`) only ever grows a counter — nothing deallocates — so pages orphaned by a merge or a root-shrink stay in the file forever as dead space. A long-running database only grows.
5. **`put` accumulates without bound, into a page that cannot grow.** `Value::accumulate` (`value/src/value.rs:26-33`) caps nothing, but every node still has to fit in one fixed 4KB page (`PAGE_SIZE`, `btree/src/pager.rs:13`) — and a verbose codec like JSON hits that ceiling sooner than bincode does. A key `put` often enough eventually fails with `DbError::PageOverflow`, and the error lands on whichever call happened to tip the page over, not on the one that actually caused the growth.

**Easy**

6. **Nothing in a database file identifies the codec that wrote it.** `Pager::open_with` (`btree/src/pager.rs:31`) never writes or checks a format tag, so opening a bincode file with `JsonCodec` reports the mismatch at the first page read instead of at `open()`. A one-byte magic/version prefix written on file creation and checked in `open_with` would surface this immediately, and the fix never has to touch the tree algorithm.
7. **Value conversions are exact-match, with no widening.** Every `TryFrom<Value>` impl in `value/src/value.rs` matches exactly one variant — `i32` is never readable as `i64` — so changing a stored field's width is a breaking change for existing data. Adding specific widening conversions (`i32 -> i64`, `f32 -> f64`) is additive and self-contained to that one file.
