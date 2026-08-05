use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValueError {
    #[error("stored value does not convert to the requested type")]
    TypeMismatch,

    #[error("key not found")]
    NotFound,
}
