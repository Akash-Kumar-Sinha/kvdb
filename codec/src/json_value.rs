use std::fmt::{Display, Write};
use std::str;

use serde_json::{Map, Number, Value as JsonValue};
use value::{Value, ValueError};

pub(crate) const NAME: &str = "json";

const I64: &str = "i64";
const I32: &str = "i32";
const I8: &str = "i8";
const U64: &str = "u64";
const U32: &str = "u32";
const U8: &str = "u8";
const F64: &str = "f64";
const F32: &str = "f32";
const CHAR: &str = "char";
const TEXT: &str = "text";
const BYTES: &str = "bytes";
const LIST: &str = "list";
const PAIR: &str = "pair";

const NAN: &str = "nan";
const INF: &str = "inf";
const NEG_INF: &str = "-inf";

#[derive(Debug, Clone, PartialEq)]
pub struct Json(JsonValue);

impl Json {
    pub fn as_json(&self) -> &JsonValue {
        &self.0
    }

    pub fn into_json(self) -> JsonValue {
        self.0
    }
}

impl From<JsonValue> for Json {
    fn from(json: JsonValue) -> Self {
        Json(json)
    }
}

impl From<Json> for JsonValue {
    fn from(json: Json) -> Self {
        json.0
    }
}

impl From<&Value> for Json {
    fn from(value: &Value) -> Self {
        Json(encode(value))
    }
}

impl From<Value> for Json {
    fn from(value: Value) -> Self {
        Json::from(&value)
    }
}

impl TryFrom<Json> for Value {
    type Error = ValueError;

    fn try_from(json: Json) -> Result<Self, Self::Error> {
        decode(&json.0)
    }
}

fn err(message: impl Display) -> ValueError {
    ValueError::decode(NAME, message)
}

fn tagged(tag: &str, body: JsonValue) -> JsonValue {
    let mut object = Map::with_capacity(1);
    object.insert(tag.to_owned(), body);
    JsonValue::Object(object)
}

fn list(values: &[Value]) -> JsonValue {
    JsonValue::Array(values.iter().map(encode).collect())
}

fn float(number: f64) -> JsonValue {
    match Number::from_f64(number) {
        Some(finite) => JsonValue::Number(finite),
        None if number.is_nan() => JsonValue::String(NAN.to_owned()),
        None if number.is_sign_positive() => JsonValue::String(INF.to_owned()),
        None => JsonValue::String(NEG_INF.to_owned()),
    }
}

fn encode(value: &Value) -> JsonValue {
    match value {
        Value::I64(number) => tagged(I64, JsonValue::from(*number)),
        Value::I32(number) => tagged(I32, JsonValue::from(*number)),
        Value::I8(number) => tagged(I8, JsonValue::from(*number)),
        Value::UInt64(number) => tagged(U64, JsonValue::from(*number)),
        Value::UInt32(number) => tagged(U32, JsonValue::from(*number)),
        Value::UInt8(number) => tagged(U8, JsonValue::from(*number)),
        Value::F64(number) => tagged(F64, float(*number)),
        Value::F32(number) => tagged(F32, float(f64::from(*number))),
        Value::Char(character) => tagged(CHAR, JsonValue::from(character.to_string())),
        Value::Text(text) => tagged(TEXT, JsonValue::from(text.as_str())),
        Value::Bytes(bytes) => tagged(BYTES, JsonValue::from(to_hex(bytes))),
        Value::List(values) => tagged(LIST, list(values)),
        Value::Pair(left, right) => tagged(PAIR, JsonValue::Array(vec![list(left), list(right)])),
        other => panic!("json codec has no encoding for {other:?}"),
    }
}

fn decode(json: &JsonValue) -> Result<Value, ValueError> {
    let object = json
        .as_object()
        .ok_or_else(|| err("expected a single-key tag object"))?;
    let mut entries = object.iter();
    let (tag, body) = match (entries.next(), entries.next()) {
        (Some(entry), None) => entry,
        _ => {
            return Err(err(format!(
                "expected exactly one tag, got {}",
                object.len()
            )));
        }
    };

    match tag.as_str() {
        I64 => Ok(Value::I64(as_i64(body)?)),
        I32 => Ok(Value::I32(narrow(as_i64(body)?, I32)?)),
        I8 => Ok(Value::I8(narrow(as_i64(body)?, I8)?)),
        U64 => Ok(Value::UInt64(as_u64(body)?)),
        U32 => Ok(Value::UInt32(narrow(as_u64(body)?, U32)?)),
        U8 => Ok(Value::UInt8(narrow(as_u64(body)?, U8)?)),
        F64 => Ok(Value::F64(as_f64(body)?)),
        #[allow(
            clippy::cast_possible_truncation,
            reason = "the f64 was widened from an f32 by `encode`, so narrowing back is exact"
        )]
        F32 => Ok(Value::F32(as_f64(body)? as f32)),
        CHAR => Ok(Value::Char(as_char(body)?)),
        TEXT => Ok(Value::Text(as_text(body)?.to_owned())),
        BYTES => Ok(Value::Bytes(from_hex(as_text(body)?)?)),
        LIST => Ok(Value::List(as_list(body)?)),
        PAIR => {
            let (left, right) = as_two(body)?;
            Ok(Value::Pair(as_list(left)?, as_list(right)?))
        }
        unknown => Err(err(format!("unknown value tag {unknown:?}"))),
    }
}

fn as_i64(body: &JsonValue) -> Result<i64, ValueError> {
    body.as_i64()
        .ok_or_else(|| err(format!("expected a signed integer, got {body}")))
}

fn as_u64(body: &JsonValue) -> Result<u64, ValueError> {
    body.as_u64()
        .ok_or_else(|| err(format!("expected an unsigned integer, got {body}")))
}

fn narrow<W>(number: impl TryInto<W>, tag: &str) -> Result<W, ValueError> {
    number
        .try_into()
        .map_err(|_| err(format!("integer does not fit in {tag}")))
}

fn as_f64(body: &JsonValue) -> Result<f64, ValueError> {
    match body {
        JsonValue::Number(number) => number
            .as_f64()
            .ok_or_else(|| err(format!("expected a float, got {number}"))),
        JsonValue::String(text) => match text.as_str() {
            NAN => Ok(f64::NAN),
            INF => Ok(f64::INFINITY),
            NEG_INF => Ok(f64::NEG_INFINITY),
            other => Err(err(format!(
                "expected a non-finite float tag, got {other:?}"
            ))),
        },
        other => Err(err(format!("expected a float, got {other}"))),
    }
}

fn as_text(body: &JsonValue) -> Result<&str, ValueError> {
    body.as_str()
        .ok_or_else(|| err(format!("expected a string, got {body}")))
}

fn as_char(body: &JsonValue) -> Result<char, ValueError> {
    let text = as_text(body)?;
    let mut characters = text.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) => Ok(character),
        _ => Err(err(format!("expected exactly one character, got {text:?}"))),
    }
}

fn as_array(body: &JsonValue) -> Result<&[JsonValue], ValueError> {
    body.as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| err(format!("expected an array, got {body}")))
}

fn as_list(body: &JsonValue) -> Result<Vec<Value>, ValueError> {
    as_array(body)?.iter().map(decode).collect()
}

fn as_two(body: &JsonValue) -> Result<(&JsonValue, &JsonValue), ValueError> {
    match as_array(body)? {
        [left, right] => Ok((left, right)),
        other => Err(err(format!("expected two arrays, got {}", other.len()))),
    }
}

fn to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

fn from_hex(hex: &str) -> Result<Vec<u8>, ValueError> {
    if !hex.len().is_multiple_of(2) {
        return Err(err(format!("hex string has odd length {}", hex.len())));
    }
    hex.as_bytes()
        .chunks(2)
        .map(|pair| {
            let pair = str::from_utf8(pair).map_err(|_| err("hex string is not ASCII"))?;
            u8::from_str_radix(pair, 16).map_err(|_| err(format!("{pair:?} is not a hex byte")))
        })
        .collect()
}
