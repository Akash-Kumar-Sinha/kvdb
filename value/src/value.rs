use serde::{Deserialize, Serialize};

use crate::ValueError;

/// The closed set of types KvDB can store, so a value is self-describing on
/// disk rather than an opaque blob.
///
/// Conversions to and from Rust types are exact-match, with no widening: a
/// value stored as [`Value::I32`] does not convert to `i64`. Store the width
/// you intend to read back, or convert explicitly at the call site.
///
/// This enum is `#[non_exhaustive]`: new variants may be added in a minor
/// version, so a `match` over `Value` outside this crate needs a wildcard arm.
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Value {
    /// A signed 64-bit integer.
    I64(i64),
    /// A signed 32-bit integer.
    I32(i32),
    /// A signed 8-bit integer.
    I8(i8),
    /// An unsigned 64-bit integer.
    UInt64(u64),
    /// An unsigned 32-bit integer.
    UInt32(u32),
    /// An unsigned 8-bit integer.
    UInt8(u8),
    /// A 64-bit float. Round-trips exactly through every codec, including `NaN` and infinities.
    F64(f64),
    /// A 32-bit float. Round-trips exactly through every codec, including `NaN` and infinities.
    F32(f32),
    /// A single Unicode scalar value.
    Char(char),
    /// A UTF-8 string.
    Text(String),
    /// An arbitrary byte string.
    Bytes(Vec<u8>),
    /// A list of values, itself recursive: a list can hold another list, or a [`Value::Pair`].
    List(Vec<Value>),
    /// Two lists paired together, e.g. parallel columns.
    Pair(Vec<Value>, Vec<Value>),
    /// The accumulator built by repeated `put`s of the same key.
    ///
    /// Kept as a distinct variant from [`Value::List`] rather than reusing it,
    /// so that accumulating into an existing key can never be confused with
    /// splicing into a list the caller stored themselves — the two are
    /// indistinguishable on the wire if they share a representation. Reading
    /// this back as `Vec<Value>` works the same as reading a `List`; only the
    /// write side treats them differently. See `Value::accumulate`.
    Multi(Vec<Value>),
}

impl Value {
    /// Folds `value` into `self`, building or extending a [`Value::Multi`].
    ///
    /// The first call on a non-`Multi` value wraps both the existing value and
    /// the new one into `Multi([old, new])`; subsequent calls push onto that
    /// list. This is what backs `put`'s accumulate-on-repeat-key behaviour —
    /// see the `kvdb` crate's `KvDb::put`.
    pub fn accumulate(&mut self, value: Value) {
        if let Value::Multi(values) = self {
            values.push(value);
            return;
        }
        let first = std::mem::replace(self, Value::Multi(Vec::new()));
        *self = Value::Multi(vec![first, value]);
    }

    /// Returns `true` if this value is a [`Value::Multi`] accumulator.
    #[must_use]
    pub fn is_multi(&self) -> bool {
        matches!(self, Value::Multi(_))
    }
}

// `From<T> for Value` and `TryFrom<Value> for T` below are what let `KvDb::put`
// take `impl Into<Value>` and `KvDb::get<R>` return any `R: TryFrom<Value>`,
// so callers write `db.put(1, 100i64)` instead of `db.put(1, Value::I64(100))`.
// Each `TryFrom` matches exactly one variant and returns `ValueError::TypeMismatch`
// otherwise — see the `Value` enum doc for why there is no widening.

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::I64(value)
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Value::I32(value)
    }
}

impl From<i8> for Value {
    fn from(value: i8) -> Self {
        Value::I8(value)
    }
}

impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Value::UInt64(value)
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Value::UInt32(value)
    }
}

impl From<u8> for Value {
    fn from(value: u8) -> Self {
        Value::UInt8(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::F64(value)
    }
}

impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Value::F32(value)
    }
}

impl From<char> for Value {
    fn from(value: char) -> Self {
        Value::Char(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Text(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Text(value.to_string())
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Value::Bytes(value)
    }
}

impl From<Vec<Value>> for Value {
    fn from(value: Vec<Value>) -> Self {
        Value::List(value)
    }
}

impl From<(Vec<Value>, Vec<Value>)> for Value {
    fn from(value: (Vec<Value>, Vec<Value>)) -> Self {
        Value::Pair(value.0, value.1)
    }
}

impl TryFrom<Value> for i64 {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::I64(v) => Ok(v),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for i32 {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::I32(v) => Ok(v),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for i8 {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::I8(v) => Ok(v),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for u64 {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::UInt64(v) => Ok(v),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for u32 {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::UInt32(v) => Ok(v),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for u8 {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::UInt8(v) => Ok(v),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for f64 {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::F64(v) => Ok(v),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for f32 {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::F32(v) => Ok(v),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for char {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Char(v) => Ok(v),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for String {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Text(v) => Ok(v),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for Vec<u8> {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Bytes(v) => Ok(v),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for Vec<Value> {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::List(v) | Value::Multi(v) => Ok(v),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for (Vec<Value>, Vec<Value>) {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Pair(v1, v2) => Ok((v1, v2)),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}
