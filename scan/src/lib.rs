//! The [`LendingIterator`] trait and [`ScanIter`], a zero-copy cursor over a
//! [`BTree`](btree::BTree)'s entries.
//!
//! `LendingIterator` exists because `std::iter::Iterator` cannot express an
//! item that borrows from the iterator's own state across calls to `next` —
//! see that trait's docs for the mechanism (a Generic Associated Type) and
//! why it costs dyn-compatibility to get it.

mod scan;

pub use scan::{Cursor, LendingIterator, Scan, ScanIter, Step, step};
