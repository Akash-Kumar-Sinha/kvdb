use serde::{Serialize, de::DeserializeOwned};
use value::{Value, ValueError};

use crate::btree::Node;
use crate::error::DbError;
use crate::pager::PageId;

const FIELDS: usize = 4;

pub(crate) fn to_value<S>(node: &Node<S>) -> Result<Value, DbError>
where
    S: Serialize,
{
    let keys = node
        .keys
        .iter()
        .map(|key| {
            bincode::serialize(key)
                .map(Value::Bytes)
                .map_err(|err| DbError::KeyEncode {
                    message: err.to_string(),
                })
        })
        .collect::<Result<Vec<Value>, _>>()?;
    let children = node.children.iter().copied().map(Value::UInt64).collect();

    Ok(Value::List(vec![
        Value::UInt8(u8::from(node.is_leaf)),
        Value::List(keys),
        Value::List(node.values.clone()),
        Value::List(children),
    ]))
}

pub(crate) fn from_value<S>(value: Value, codec: &'static str) -> Result<Node<S>, ValueError>
where
    S: DeserializeOwned,
{
    let fields = match value {
        Value::List(fields) if fields.len() == FIELDS => fields,
        other => {
            return Err(ValueError::decode(
                codec,
                format!("page layout: expected a list of {FIELDS} fields, got {other:?}"),
            ));
        }
    };
    let mut fields = fields.into_iter();
    let mut next = || fields.next().expect("length checked above");

    let is_leaf = match next() {
        Value::UInt8(flag) => flag != 0,
        other => return Err(field_error(codec, "is_leaf", &other)),
    };
    let keys = list(next(), codec, "keys")?
        .into_iter()
        .map(|key| match key {
            Value::Bytes(bytes) => bincode::deserialize(&bytes)
                .map_err(|err| ValueError::decode(codec, format!("page key: {err}"))),
            other => Err(field_error(codec, "key", &other)),
        })
        .collect::<Result<Vec<S>, _>>()?;
    let values = list(next(), codec, "values")?;
    let children = list(next(), codec, "children")?
        .into_iter()
        .map(|child| match child {
            Value::UInt64(id) => Ok(id),
            other => Err(field_error(codec, "child", &other)),
        })
        .collect::<Result<Vec<PageId>, _>>()?;

    Ok(Node {
        keys,
        values,
        children,
        is_leaf,
    })
}

fn list(value: Value, codec: &'static str, field: &str) -> Result<Vec<Value>, ValueError> {
    match value {
        Value::List(values) => Ok(values),
        other => Err(field_error(codec, field, &other)),
    }
}

fn field_error(codec: &'static str, field: &str, found: &Value) -> ValueError {
    ValueError::decode(codec, format!("page field {field}: unexpected {found:?}"))
}

#[cfg(test)]
mod tests {
    use ::codec::{BincodeCodec, Codec, CodecRegistry};

    use super::*;

    fn sample() -> Node<i32> {
        Node {
            keys: vec![1, 2],
            values: vec![Value::Text("a".into()), Value::List(vec![Value::I8(-1)])],
            children: vec![7, 8, 9],
            is_leaf: false,
        }
    }

    fn assert_same(left: &Node<i32>, right: &Node<i32>) {
        assert_eq!(left.keys, right.keys);
        assert_eq!(left.values, right.values);
        assert_eq!(left.children, right.children);
        assert_eq!(left.is_leaf, right.is_leaf);
    }

    #[test]
    fn every_codec_round_trips_a_node() -> Result<(), DbError> {
        let node = sample();
        for codec in CodecRegistry::default().codecs() {
            let bytes = codec.encode(&to_value(&node)?);
            let decoded = codec.decode(&bytes).expect("own output decodes");
            let round_tripped: Node<i32> =
                from_value(decoded, codec.name()).expect("layout survives the codec");
            assert_same(&node, &round_tripped);
        }
        Ok(())
    }

    #[test]
    fn an_empty_leaf_round_trips() -> Result<(), DbError> {
        let empty: Node<i32> = Node {
            keys: Vec::new(),
            values: Vec::new(),
            children: Vec::new(),
            is_leaf: true,
        };
        let value = to_value(&empty)?;
        assert_same(
            &empty,
            &from_value::<i32>(value, "bincode").expect("empty leaf decodes"),
        );
        Ok(())
    }

    #[test]
    fn rejects_a_page_that_is_not_the_layout() {
        let cases = [
            Value::I32(1),
            Value::List(vec![Value::UInt8(1)]),
            Value::List(vec![
                Value::Text("not a flag".into()),
                Value::List(vec![]),
                Value::List(vec![]),
                Value::List(vec![]),
            ]),
            Value::List(vec![
                Value::UInt8(1),
                Value::List(vec![Value::I32(0)]),
                Value::List(vec![]),
                Value::List(vec![]),
            ]),
        ];
        for case in cases {
            let err = from_value::<i32>(case.clone(), "bincode")
                .expect_err(&format!("{case:?} is not a page"));
            assert!(matches!(
                err,
                ValueError::Decode {
                    codec: "bincode",
                    ..
                }
            ));
        }
    }

    #[test]
    fn reports_an_unreadable_key_blob_instead_of_panicking() -> Result<(), DbError> {
        let node = Node {
            keys: vec!["a string key".to_owned()],
            values: vec![Value::I8(0)],
            children: Vec::new(),
            is_leaf: true,
        };
        let Value::List(mut fields) = to_value(&node)? else {
            panic!("a page is always a list");
        };
        fields[1] = Value::List(vec![Value::Bytes(vec![0x01])]);

        let bytes = BincodeCodec.encode(&Value::List(fields));
        let decoded = BincodeCodec.decode(&bytes).expect("own output decodes");
        let err =
            from_value::<String>(decoded, "bincode").expect_err("a one-byte blob is not a String");
        assert!(matches!(
            err,
            ValueError::Decode {
                codec: "bincode",
                ..
            }
        ));
        Ok(())
    }
}
