//! Exact submitted identity, independent of semantic final-state equivalence.

use factlite::{
    AttributeDefinition, Cardinality, EntityId, SnapshotId, TempId, Tx, Uniqueness, Value,
    ValueType, wire,
};
use proptest::prelude::*;

const ENTITY: &str = "e1.00000000000000000000000000000001.0000000000000001";
fn entity() -> EntityId {
    ENTITY.parse().unwrap()
}
fn assertion(value: Value) -> Tx {
    Tx::builder()
        .assert(entity(), "x/value".parse().unwrap(), value)
        .build()
        .unwrap()
}

#[test]
fn canonical_version_one_has_fixed_golden_bytes_and_raw_float_bits() {
    let empty = Tx::default().canonical_request();
    assert_eq!(empty.version(), 1);
    let mut golden = b"FLC1\0\0\0\x01".to_vec();
    golden.extend_from_slice(&[0; 41]);
    assert_eq!(empty.bytes(), golden);
    // The final metadata value is typed F64 followed by raw normalized binary64.
    let tx = Tx::builder()
        .metadata("f", Value::f64(-1.5).unwrap())
        .build()
        .unwrap()
        .canonical_request();
    assert_eq!(
        &tx.bytes()[tx.bytes().len() - 9..],
        &[2, 0xbf, 0xf8, 0, 0, 0, 0, 0, 0]
    );
    let tx = Tx::builder()
        .metadata("i", Value::I64(i64::MIN))
        .build()
        .unwrap()
        .canonical_request();
    assert_eq!(
        &tx.bytes()[tx.bytes().len() - 9..],
        &[1, 0x80, 0, 0, 0, 0, 0, 0, 0]
    );
}

#[test]
fn defaults_key_object_order_set_order_and_wire_spelling_do_not_change_identity() {
    let omitted = wire::decode_tx(r#"{"version":1,"idempotency_key":"one"}"#).unwrap();
    let explicit = wire::decode_tx(r#"{"version":1,"metadata":{},"schema":[],"changes":[],"new_entities":[],"expect":[],"idempotency_key":"two"}"#).unwrap();
    assert_eq!(omitted.canonical_request(), explicit.canonical_request());
    assert_eq!(
        omitted.canonical_request(),
        Tx::default().canonical_request()
    );
    let request = |integer: &str, float: &str, bytes: &str| {
        format!(
            r#"{{"version":1,"metadata":{{"int":{{"i64":"{integer}"}},"float":{{"f64":"{float}"}},"bytes":{{"bytes":"{bytes}"}}}}}}"#
        )
    };
    for alternative in [
        request("+0001", "1e0", "AQI"),
        request("1", "1.000", "AQI="),
    ] {
        assert_eq!(
            wire::decode_tx(&alternative).unwrap().canonical_request(),
            wire::decode_tx(&request("1", "1", "AQI="))
                .unwrap()
                .canonical_request()
        );
    }
    assert_eq!(
        assertion(Value::f64(-0.0).unwrap()).canonical_request(),
        assertion(Value::f64(0.0).unwrap()).canonical_request()
    );
    let a = Tx::builder()
        .expect(
            entity(),
            "x/value".parse().unwrap(),
            vec![Value::I64(2), Value::I64(1), Value::I64(2)],
        )
        .metadata("z", Value::Bool(true))
        .metadata("a", Value::I64(3))
        .build()
        .unwrap();
    let b = Tx::builder()
        .expect(
            entity(),
            "x/value".parse().unwrap(),
            vec![Value::I64(1), Value::I64(2)],
        )
        .metadata("a", Value::I64(3))
        .metadata("z", Value::Bool(true))
        .build()
        .unwrap();
    assert_eq!(a.canonical_request(), b.canonical_request());
}

#[test]
fn every_submitted_dimension_affects_identity_and_arrays_keep_order() {
    let base = Tx::default().canonical_request();
    let head: SnapshotId = "d1.00000000000000000000000000000001.00000000000000000000000000000002.0"
        .parse()
        .unwrap();
    let definition = AttributeDefinition::new(
        "x/value".parse().unwrap(),
        ValueType::I64,
        Cardinality::One,
        Uniqueness::None,
    )
    .unwrap();
    for tx in [
        Tx::builder().expect_head(head).build().unwrap(),
        Tx::builder()
            .metadata("source", Value::I64(1))
            .build()
            .unwrap(),
        Tx::builder()
            .new_entity(TempId::new("a").unwrap())
            .build()
            .unwrap(),
        Tx::builder().define_attribute(definition).build().unwrap(),
        Tx::builder()
            .expect(entity(), "x/value".parse().unwrap(), vec![])
            .build()
            .unwrap(),
        assertion(Value::I64(1)),
    ] {
        assert_ne!(base, tx.canonical_request());
    }
    assert_ne!(
        assertion(Value::I64(1)).canonical_request(),
        assertion(Value::f64(1.0).unwrap()).canonical_request()
    );
    assert_ne!(
        assertion(Value::String("a\0b".into())).canonical_request(),
        assertion(Value::Bytes(b"a\0b".to_vec())).canonical_request()
    );
    let a = Tx::builder()
        .assert(entity(), "x/value".parse().unwrap(), Value::I64(1))
        .assert(entity(), "x/value".parse().unwrap(), Value::I64(2))
        .build()
        .unwrap();
    let b = Tx::builder()
        .assert(entity(), "x/value".parse().unwrap(), Value::I64(2))
        .assert(entity(), "x/value".parse().unwrap(), Value::I64(1))
        .build()
        .unwrap();
    assert_ne!(a.canonical_request(), b.canonical_request());
    let duplicate = Tx::builder()
        .assert(entity(), "x/value".parse().unwrap(), Value::I64(1))
        .assert(entity(), "x/value".parse().unwrap(), Value::I64(1))
        .build()
        .unwrap();
    assert_ne!(
        assertion(Value::I64(1)).canonical_request(),
        duplicate.canonical_request()
    );
    let named = |name| {
        let label = TempId::new(name).unwrap();
        Tx::builder()
            .new_entity(label.clone())
            .assert(label, "x/value".parse().unwrap(), Value::I64(1))
            .build()
            .unwrap()
    };
    assert_ne!(
        named("a").canonical_request(),
        named("b").canonical_request()
    );
    let labels = |a, b| {
        Tx::builder()
            .new_entity(TempId::new(a).unwrap())
            .new_entity(TempId::new(b).unwrap())
            .build()
            .unwrap()
    };
    assert_ne!(
        labels("a", "b").canonical_request(),
        labels("b", "a").canonical_request()
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn expected_set_permutations_and_duplicates_preserve_identity(values in proptest::collection::vec(any::<i64>(),0..32)) {
        let mut reverse = values.clone(); reverse.reverse(); reverse.extend_from_slice(&values);
        let tx = |values: Vec<i64>| Tx::builder().expect(entity(),"x/value".parse().unwrap(),values.into_iter().map(Value::I64).collect()).build().unwrap();
        prop_assert_eq!(tx(values).canonical_request(),tx(reverse).canonical_request());
    }
    #[test]
    fn canonical_identity_survives_wire_roundtrip(value in any::<i64>(), bytes in proptest::collection::vec(any::<u8>(),0..96), key in any::<String>()) {
        let tx = Tx::builder().assert(entity(),"x/value".parse().unwrap(),Value::I64(value)).metadata("bytes",Value::Bytes(bytes)).idempotency_key(key).build().unwrap();
        prop_assert_eq!(tx.canonical_request(),wire::decode_tx(&wire::encode_tx(&tx)).unwrap().canonical_request());
    }
    #[test]
    fn distinct_scalar_payloads_do_not_collide(a in any::<i64>(), b in any::<i64>()) {
        prop_assert_eq!(assertion(Value::I64(a)).canonical_request() == assertion(Value::I64(b)).canonical_request(),a == b);
    }
}
