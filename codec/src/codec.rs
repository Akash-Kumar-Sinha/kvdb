use std::fmt::Debug;

use value::{Value, ValueError};

pub trait Codec: Debug + Send + Sync {
    fn name(&self) -> &'static str;

    fn encode(&self, value: &Value) -> Vec<u8>;

    fn decode(&self, bytes: &[u8]) -> Result<Value, ValueError>;

    fn boxed_clone(&self) -> Box<dyn Codec>;
}

impl Clone for Box<dyn Codec> {
    fn clone(&self) -> Self {
        self.boxed_clone()
    }
}
