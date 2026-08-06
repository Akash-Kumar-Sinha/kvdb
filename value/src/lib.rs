//! The typed value KvDB stores, and the error it fails with.
//!
//! This is the lowest crate in the workspace — every other crate (`codec`, `btree`,
//! `scan`, `kvdb`, `async_kvdb`) depends on it, and it depends on nothing internal.
//! That position is deliberate: [`Value`] has to be a foreign type from `codec`'s
//! point of view for the orphan-rule workaround in `codec::Json` to be real, and it
//! can only be foreign if it lives outside `codec` entirely.

mod error;
mod value;

pub use error::ValueError;
pub use value::Value;
