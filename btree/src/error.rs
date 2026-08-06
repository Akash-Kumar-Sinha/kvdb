use std::io;

use thiserror::Error;
use value::ValueError;

use crate::pager::PageId;

/// The storage-level error: everything that can go wrong reading or writing a page.
///
/// Wraps [`ValueError`] for value-level failures (`NotFound`, `TypeMismatch`,
/// a codec decode error) alongside I/O and page-layout failures that are
/// specific to the on-disk format.
///
/// `#[non_exhaustive]`: new variants may be added in a minor version, so a
/// `match` outside this crate needs a wildcard arm.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DbError {
    /// The database file could not be opened, sought, read, or written.
    /// Keeps the original [`std::io::Error`] as its `source()`.
    #[error("database file i/o failed")]
    Io(#[from] io::Error),

    /// A value-level failure — `NotFound`, `TypeMismatch`, or a codec's decode error.
    #[error(transparent)]
    Value(#[from] ValueError),

    /// A page's length prefix claims more bytes than a page can hold.
    /// Would previously have been an out-of-bounds slice panic.
    #[error(
        "page {page} is corrupt: claims a {len}-byte payload, but a page holds at most {capacity}"
    )]
    CorruptPage {
        /// The page that failed to read.
        page: PageId,
        /// The claimed payload length.
        len: usize,
        /// The maximum payload a page can hold.
        capacity: usize,
    },

    /// A key's `Serialize` implementation failed while writing a page.
    #[error("a key could not be encoded: {message}")]
    KeyEncode {
        /// A human-readable description of the encoding failure.
        message: String,
    },

    /// An encoded node does not fit in one fixed-size page. Would previously have been an `assert!`.
    #[error(
        "node does not fit in one page: {len} bytes with the {codec} codec, \
         capacity is {capacity} — lower MIN_DEGREE, pick a more compact codec, \
         or store a smaller key/value type"
    )]
    PageOverflow {
        /// The encoded size that did not fit.
        len: usize,
        /// The name of the codec that produced the oversized encoding.
        codec: &'static str,
        /// The maximum payload a page can hold.
        capacity: usize,
    },
}

impl DbError {
    /// Shorthand for the common case: is this specifically a missing-key error?
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, DbError::Value(ValueError::NotFound))
    }
}
