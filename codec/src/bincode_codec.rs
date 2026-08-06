use value::{Value, ValueError};

use crate::Codec;

/// The default [`Codec`]: a compact binary format via [`bincode`], and the
/// codec `Pager::open` uses when none is chosen explicitly.
///
/// Delegates directly to `Value`'s derived `Serialize`/`Deserialize`, so a
/// `Value`'s Rust variant names and layout are part of the wire format —
/// renaming a variant changes bytes already on disk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct BincodeCodec;

impl Codec for BincodeCodec {
    fn name(&self) -> &'static str {
        "bincode"
    }

    fn encode(&self, value: &Value) -> Vec<u8> {
        bincode::serialize(value).expect("bincode cannot fail on a Value")
    }

    fn decode(&self, bytes: &[u8]) -> Result<Value, ValueError> {
        bincode::deserialize(bytes).map_err(|err| ValueError::decode("bincode", err))
    }

    fn boxed_clone(&self) -> Box<dyn Codec> {
        Box::new(*self)
    }
}
