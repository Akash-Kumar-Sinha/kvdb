use value::{Value, ValueError};

use crate::Codec;

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
