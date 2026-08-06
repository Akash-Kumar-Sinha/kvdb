use std::fmt::Debug;

use value::{Value, ValueError};

/// Turns a [`Value`] into bytes and back — the wire format `Pager` writes to a page.
///
/// This trait is deliberately object-safe: every method takes `&self`, none
/// is generic, and none consumes `Self` by value. That is what lets a
/// [`CodecRegistry`](crate::CodecRegistry) hold `Box<dyn Codec>` and pick a
/// format at runtime from a name — for example an environment variable or a
/// config file — rather than requiring the format to be a compile-time type
/// parameter threaded through `Pager`, `BTree`, and `KvDb`.
///
/// The cost of that flexibility is `boxed_clone`: `Clone` itself is not
/// object-safe (`fn clone(&self) -> Self` returns `Self` by value), so cloning
/// a `Box<dyn Codec>` needs a hand-written, object-safe equivalent. The
/// `impl Clone for Box<dyn Codec>` below routes through it, so callers can
/// still write `.clone()` on a boxed codec without knowing this trick exists.
pub trait Codec: Debug + Send + Sync {
    /// A short, stable identifier for this format, e.g. `"bincode"` or `"json"`.
    ///
    /// Used as the key in [`CodecRegistry`](crate::CodecRegistry) and reported
    /// in [`ValueError::Decode`]'s `codec` field, so it should stay stable
    /// across versions — changing it changes what a `CodecRegistry::get` call
    /// resolves.
    fn name(&self) -> &'static str;

    /// Encodes `value` into this format's bytes.
    fn encode(&self, value: &Value) -> Vec<u8>;

    /// Decodes bytes previously produced by [`Codec::encode`] back into a `Value`.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError::Decode`] if `bytes` is not valid output of this
    /// codec — truncated data, or bytes written by a different codec entirely.
    fn decode(&self, bytes: &[u8]) -> Result<Value, ValueError>;

    /// An object-safe stand-in for `Clone::clone`, since `Clone` itself cannot
    /// be called through `dyn Codec`. Implementors typically just box a copy
    /// of `*self`.
    fn boxed_clone(&self) -> Box<dyn Codec>;
}

impl Clone for Box<dyn Codec> {
    fn clone(&self) -> Self {
        self.boxed_clone()
    }
}
