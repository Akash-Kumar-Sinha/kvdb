use std::fmt::Display;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValueError {
    #[error("stored value does not convert to the requested type")]
    TypeMismatch,

    #[error("key not found")]
    NotFound,

    #[error("{codec} codec could not decode stored bytes: {message}")]
    Decode {
        codec: &'static str,
        message: String,
    },
}

impl ValueError {
    #[must_use]
    pub fn decode(codec: &'static str, message: impl Display) -> Self {
        ValueError::Decode {
            codec,
            message: message.to_string(),
        }
    }
}
