use value::{Value, ValueError};

use crate::Codec;
use crate::json_value::{self, Json};

/// A tagged, human-readable [`Codec`] backed by [`serde_json`], written to
/// prove the `Codec` abstraction generalises past `bincode`.
///
/// Every value is a single-key tag object, e.g. `{"i64":42}` or
/// `{"text":"hi"}`, rather than JSON's own untagged numbers and strings —
/// see the `json_value` module for why a hand-rolled mapping was necessary
/// instead of `Value`'s derived `Serialize`. A database opened with this
/// codec is inspectable with `cat` or `jq`, at the cost of larger pages than
/// `bincode` produces for the same data.
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
