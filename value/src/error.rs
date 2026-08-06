use std::fmt::Display;

use thiserror::Error;

/// A failure at the value level: the wrong type was asked for, the key was
/// missing, or a codec couldn't turn bytes back into a [`Value`](crate::Value).
///
/// This is distinct from `btree::DbError`, which wraps `ValueError` alongside
/// I/O and page-layout failures. `ValueError` covers only what can go wrong
/// with a value once the bytes for it are already in hand.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValueError {
    /// The stored value exists but is not the variant the caller asked to convert it to.
    ///
    /// There is no widening: a value stored as `I32` returns this for a
    /// requested `i64`, not a converted value.
    #[error("stored value does not convert to the requested type")]
    TypeMismatch,

    /// No entry exists for the requested key.
    #[error("key not found")]
    NotFound,

    /// A codec (see the `codec` crate's `Codec` trait) could not turn its stored
    /// bytes back into a `Value` — corrupt data, a truncated write, or bytes
    /// written by a different codec entirely.
    #[error("{codec} codec could not decode stored bytes: {message}")]
    Decode {
        /// The `Codec::name()` of the codec that failed to decode.
        codec: &'static str,
        /// A human-readable description of what went wrong.
        message: String,
    },
}

impl ValueError {
    /// Builds a [`ValueError::Decode`] from a codec's name and any displayable failure.
    #[must_use]
    pub fn decode(codec: &'static str, message: impl Display) -> Self {
        ValueError::Decode {
            codec,
            message: message.to_string(),
        }
    }
}
