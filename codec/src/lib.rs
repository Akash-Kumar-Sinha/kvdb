//! A pluggable wire format for [`Value`](value::Value), chosen at runtime instead
//! of hard-coded into the pager.
//!
//! [`Codec`] is the object-safe trait every format implements; [`BincodeCodec`]
//! and [`JsonCodec`] are the two that ship, and [`CodecRegistry`] maps a
//! codec's name to a boxed instance so a format can be picked from a config
//! string rather than a turbofish. [`Json`] is the newtype that makes
//! `JsonCodec` legal to write at all — see its docs for why.

mod bincode_codec;
mod codec;
mod json_codec;
mod json_value;
mod registry;

pub use bincode_codec::BincodeCodec;
pub use codec::Codec;
pub use json_codec::JsonCodec;
pub use json_value::Json;
pub use registry::CodecRegistry;
