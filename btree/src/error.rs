use std::io;

use thiserror::Error;
use value::ValueError;

use crate::pager::PageId;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DbError {
    #[error("database file i/o failed")]
    Io(#[from] io::Error),

    #[error(transparent)]
    Value(#[from] ValueError),

    #[error(
        "page {page} is corrupt: claims a {len}-byte payload, but a page holds at most {capacity}"
    )]
    CorruptPage {
        page: PageId,
        len: usize,
        capacity: usize,
    },

    #[error("a key could not be encoded: {message}")]
    KeyEncode { message: String },

    #[error(
        "node does not fit in one page: {len} bytes with the {codec} codec, \
         capacity is {capacity} — lower MIN_DEGREE, pick a more compact codec, \
         or store a smaller key/value type"
    )]
    PageOverflow {
        len: usize,
        codec: &'static str,
        capacity: usize,
    },
}

impl DbError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, DbError::Value(ValueError::NotFound))
    }
}
