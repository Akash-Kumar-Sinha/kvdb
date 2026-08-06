use std::collections::BTreeSet;

use codec::CodecRegistry;
use proptest::prelude::*;
use value::Value;

const VARIANTS: usize = 14;

fn variant_name(value: &Value) -> &'static str {
    match value {
        Value::I64(_) => "I64",
        Value::I32(_) => "I32",
        Value::I8(_) => "I8",
        Value::UInt64(_) => "UInt64",
        Value::UInt32(_) => "UInt32",
        Value::UInt8(_) => "UInt8",
        Value::F64(_) => "F64",
        Value::F32(_) => "F32",
        Value::Char(_) => "Char",
        Value::Text(_) => "Text",
        Value::Bytes(_) => "Bytes",
        Value::List(_) => "List",
        Value::Pair(..) => "Pair",
        Value::Multi(_) => "Multi",
        other => panic!("a new Value variant is not covered by this test: {other:?}"),
    }
}

fn leaf_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<i64>().prop_map(Value::I64),
        any::<i32>().prop_map(Value::I32),
        any::<i8>().prop_map(Value::I8),
        any::<u64>().prop_map(Value::UInt64),
        any::<u32>().prop_map(Value::UInt32),
        any::<u8>().prop_map(Value::UInt8),
        any::<f64>().prop_map(Value::F64),
        any::<f32>().prop_map(Value::F32),
        any::<char>().prop_map(Value::Char),
        any::<String>().prop_map(Value::Text),
        any::<Vec<u8>>().prop_map(Value::Bytes),
    ]
}

fn any_value() -> impl Strategy<Value = Value> {
    leaf_value().prop_recursive(4, 32, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::List),
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Multi),
            (
                prop::collection::vec(inner.clone(), 0..3),
                prop::collection::vec(inner, 0..3)
            )
                .prop_map(|(left, right)| Value::Pair(left, right)),
        ]
    })
}

fn same(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::F64(a), Value::F64(b)) => a == b || (a.is_nan() && b.is_nan()),
        (Value::F32(a), Value::F32(b)) => a == b || (a.is_nan() && b.is_nan()),
        (Value::List(a), Value::List(b)) | (Value::Multi(a), Value::Multi(b)) => {
            a.len() == b.len() && std::iter::zip(a, b).all(|(a, b)| same(a, b))
        }
        (Value::Pair(a1, a2), Value::Pair(b1, b2)) => {
            a1.len() == b1.len()
                && a2.len() == b2.len()
                && std::iter::zip(a1, b1).all(|(a, b)| same(a, b))
                && std::iter::zip(a2, b2).all(|(a, b)| same(a, b))
        }
        (a, b) => a == b,
    }
}

fn assert_round_trips(value: &Value) {
    for codec in CodecRegistry::default().codecs() {
        let bytes = codec.encode(value);
        let decoded = codec.decode(&bytes).unwrap_or_else(|err| {
            panic!("{} failed to decode its own output: {err}", codec.name())
        });
        assert!(
            same(value, &decoded),
            "{} did not round-trip {value:?}, got {decoded:?}",
            codec.name()
        );
    }
}

fn collect_variants(value: &Value, seen: &mut BTreeSet<&'static str>) {
    seen.insert(variant_name(value));
    match value {
        Value::List(values) | Value::Multi(values) => {
            for value in values {
                collect_variants(value, seen);
            }
        }
        Value::Pair(left, right) => {
            for value in left.iter().chain(right) {
                collect_variants(value, seen);
            }
        }
        _ => {}
    }
}

#[test]
fn every_variant_is_generated() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = any_value();
    let mut seen = BTreeSet::new();

    for _ in 0..4096 {
        let tree = strategy
            .new_tree(&mut runner)
            .expect("the strategy always produces a value");
        collect_variants(&tree.current(), &mut seen);
        if seen.len() == VARIANTS {
            break;
        }
    }

    assert_eq!(
        seen.len(),
        VARIANTS,
        "the generator never produced every Value variant — saw {seen:?}"
    );
}

#[test]
fn a_multi_round_trips_through_every_codec() {
    assert_round_trips(&Value::Multi(vec![
        Value::I32(90),
        Value::I32(100),
        Value::I32(8),
        Value::I32(60),
    ]));
}

#[test]
fn a_multi_is_not_a_list_on_the_wire() {
    let list = Value::List(vec![Value::I32(1), Value::I32(2)]);
    let multi = Value::Multi(vec![Value::I32(1), Value::I32(2)]);

    for codec in CodecRegistry::default().codecs() {
        assert_ne!(
            codec.encode(&list),
            codec.encode(&multi),
            "{} encodes a List and a Multi identically — accumulated values would be \
             indistinguishable from a value the caller stored",
            codec.name()
        );
        assert_eq!(codec.decode(&codec.encode(&list)).expect("decodes"), list);
        assert_eq!(codec.decode(&codec.encode(&multi)).expect("decodes"), multi);
    }
}

proptest! {
    #[test]
    fn every_codec_round_trips_every_value(value in any_value()) {
        assert_round_trips(&value);
    }

    #[test]
    fn nested_lists_and_pairs_survive(value in any_value()) {
        assert_round_trips(&Value::List(vec![value.clone(), Value::Multi(vec![value])]));
    }

    #[test]
    fn extreme_floats_survive(number in any::<f64>()) {
        assert_round_trips(&Value::F64(number));
    }
}
