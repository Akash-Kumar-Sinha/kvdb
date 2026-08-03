use serde::{Deserialize, Serialize};

use crate::ValueError;

#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Value {
    I64(i64),
    I32(i32),
    I8(i8),
    UInt64(u64),
    UInt32(u32),
    UInt8(u8),
    F64(f64),
    F32(f32),
    Char(char),
    Text(String),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Pair(Vec<Value>, Vec<Value>),
}

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
            Value::I32(v) => Ok(v as i64),
            Value::I8(v) => Ok(v as i64),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for i32 {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::I32(v) => Ok(v),
            Value::I64(v) => i32::try_from(v).map_err(|_| ValueError::TypeMismatch),
            Value::I8(v) => i32::try_from(v).map_err(|_| ValueError::TypeMismatch),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for i8 {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::I8(v) => Ok(v),
            Value::I64(v) => i8::try_from(v).map_err(|_| ValueError::TypeMismatch),
            Value::I32(v) => i8::try_from(v).map_err(|_| ValueError::TypeMismatch),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for u64 {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::UInt64(v) => Ok(v),
            Value::UInt32(v) => u64::try_from(v).map_err(|_| ValueError::TypeMismatch),
            Value::UInt8(v) => u64::try_from(v).map_err(|_| ValueError::TypeMismatch),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for u32 {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::UInt32(v) => Ok(v),
            Value::UInt64(v) => u32::try_from(v).map_err(|_| ValueError::TypeMismatch),
            Value::UInt8(v) => u32::try_from(v).map_err(|_| ValueError::TypeMismatch),
            _ => Err(ValueError::TypeMismatch),
        }
    }
}

impl TryFrom<Value> for u8 {
    type Error = ValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::UInt8(v) => Ok(v),
            Value::UInt32(v) => u8::try_from(v).map_err(|_| ValueError::TypeMismatch),
            Value::UInt64(v) => u8::try_from(v).map_err(|_| ValueError::TypeMismatch),
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
            Value::List(v) => Ok(v),
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
