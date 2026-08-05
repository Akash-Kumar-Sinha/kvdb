use value::{Value, ValueError};

use crate::Codec;
use crate::json_value::{self, Json};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct JsonCodec;

impl Codec for JsonCodec {
    fn name(&self) -> &'static str {
        json_value::NAME
    }

    fn encode(&self, value: &Value) -> Vec<u8> {
        serde_json::to_vec(Json::from(value).as_json()).expect("a tagged Json tree always encodes")
    }

    fn decode(&self, bytes: &[u8]) -> Result<Value, ValueError> {
        let json: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|err| ValueError::decode(self.name(), err))?;
        Value::try_from(Json::from(json))
    }

    fn boxed_clone(&self) -> Box<dyn Codec> {
        Box::new(*self)
    }
}
