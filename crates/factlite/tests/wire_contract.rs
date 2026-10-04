//! Strict interchange, typed precision, and Rust/JSON semantic parity.

use factlite::{ErrorCode, FactFilter, Revision, SnapshotId, Store, TempId, Tx, Value, wire};
use proptest::prelude::*;

const ENTITY: &str = "e1.00000000000000000000000000000001.0000000000000001";

#[test]
fn malformed_json_duplicates_unknown_fields_and_tags_are_rejected_everywhere() {
    let cases = [
        "",
        "{}",
        "[]",
        "null",
        "{\"version\":1} trailing",
        r#"{"version":1,"version":1}"#,
        r#"{"version":1,"\u0076ersion":1}"#,
        r#"{"version":1,"surprise":[]}"#,
        r#"{"version":0}"#,
        r#"{"version":2}"#,
        r#"{"version":"1"}"#,
        r#"{"version":1.0}"#,
        r#"{"version":1,"changes":null}"#,
        r#"{"version":1,"metadata":null}"#,
        r#"{"version":1,"idempotency_key":null}"#,
        r#"{"version":1,"schema":[{"name":"x/v","name":"x/v","type":"i64","cardinality":"one","unique":"none"}]}"#,
        r#"{"version":1,"schema":[{"name":"x/v","type":"i64","cardinality":"many","unique":"unique_one"}]}"#,
        r#"{"version":1,"schema":[{"name":"x/v","type":"i64","cardinality":"one","unique":"none","extra":true}]}"#,
        r#"{"version":1,"schema":[{"name":"x/v","type":"int","cardinality":"one","unique":"none"}]}"#,
        r#"{"version":1,"schema":[{"name":"x/v","type":"i64","unique":"none"}]}"#,
        r#"{"version":1,"new_entities":["e","e"]}"#,
        r#"{"version":1,"changes":[{"op":"assert","entity":{"temp":"missing"},"attr":"x/v","value":{"i64":"1"}}]}"#,
        r#"{"version":1,"new_entities":["e"],"changes":[{"op":"assert","entity":{"temp":"e","temp":"e"},"attr":"x/v","value":{"i64":"1"}}]}"#,
        r#"{"version":1,"new_entities":["e"],"changes":[{"op":"assert","entity":{"temp":"e"},"attr":"x/v","value":{"i64":"1","i64":"1"}}]}"#,
        r#"{"version":1,"new_entities":["e"],"changes":[{"op":"assert","entity":{"temp":"e","extra":true},"attr":"x/v","value":{"i64":"1"}}]}"#,
        r#"{"version":1,"new_entities":["e"],"changes":[{"op":"upsert","entity":{"temp":"e"},"attr":"x/v","value":{"i64":"1"}}]}"#,
        r#"{"version":1,"new_entities":["e"],"changes":[{"op":"assert","entity":{"temp":"e"},"attr":"x/v","value":{"ref":{"temp":"missing"}}}]}"#,
        r#"{"version":1,"metadata":{"a":{"i64":"1"},"a":{"i64":"1"}}}"#,
        r#"{"version":1,"metadata":{"a":{"i64":"1","i64":"1"}}}"#,
        r#"{"version":1,"metadata":{"a":{"ref":{"temp":"e"}}}}"#,
        r#"{"version":1,"expect":[{"entity":{"temp":"e"},"attr":"x/v","values":[]}]}"#,
    ];
    for input in cases {
        assert_eq!(
            wire::decode_tx(input).unwrap_err().code(),
            ErrorCode::InvalidRequest,
            "{input}"
        );
    }
    let mut nested = String::from(r#"{"version":1,"extra":"#);
    nested.push_str(&"[".repeat(150));
    nested.push('0');
    nested.push_str(&"]".repeat(150));
    nested.push('}');
    assert_eq!(
        wire::decode_tx(&nested).unwrap_err().code(),
        ErrorCode::InvalidRequest
    );
    let both = format!(
        r#"{{"version":1,"new_entities":["e"],"changes":[{{"op":"assert","entity":{{"id":"{ENTITY}","temp":"e"}},"attr":"x/v","value":{{"i64":"1"}}}}]}}"#
    );
    assert_eq!(
        wire::decode_tx(&both).unwrap_err().code(),
        ErrorCode::InvalidRequest
    );
    let metadata_ref =
        format!(r#"{{"version":1,"metadata":{{"x":{{"ref":{{"id":"{ENTITY}"}}}}}}}}"#);
    assert_eq!(
        wire::decode_tx(&metadata_ref).unwrap_err().code(),
        ErrorCode::InvalidRequest
    );
    let expect_missing_values =
        format!(r#"{{"version":1,"expect":[{{"entity":{{"id":"{ENTITY}"}},"attr":"x/v"}}]}}"#);
    assert_eq!(
        wire::decode_tx(&expect_missing_values).unwrap_err().code(),
        ErrorCode::InvalidRequest
    );
}

#[test]
fn exact_tags_decimal_boundaries_finite_floats_and_base64_are_enforced() {
    for input in [
        "null",
        "1",
        "{}",
        r#"{"i64":1}"#,
        r#"{"f64":1.5}"#,
        r#"{"bool":"true"}"#,
        r#"{"string":null}"#,
        r#"{"i64":"9223372036854775808"}"#,
        r#"{"i64":"-9223372036854775809"}"#,
        r#"{"i64":" 1"}"#,
        r#"{"i64":"1_0"}"#,
        r#"{"i64":"0x10"}"#,
        r#"{"f64":"NaN"}"#,
        r#"{"f64":"inf"}"#,
        r#"{"f64":"-Infinity"}"#,
        r#"{"f64":"1e400"}"#,
        r#"{"bytes":"A"}"#,
        r#"{"bytes":"AR=="}"#,
        r#"{"bytes":"_w=="}"#,
        r#"{"bytes":"AQI=\n"}"#,
        r#"{"ref":{"temp":"e"}}"#,
        r#"{"bytes":[]}"#,
        r#"{"string":"x","bool":true}"#,
        r#"{"int":"1"}"#,
    ] {
        assert_eq!(
            wire::decode_value(input).unwrap_err().code(),
            ErrorCode::InvalidRequest,
            "{input}"
        );
    }
    assert_eq!(
        wire::decode_value(r#"{"i64":"-9223372036854775808"}"#).unwrap(),
        Value::I64(i64::MIN)
    );
    assert_eq!(
        wire::decode_value(r#"{"i64":"9223372036854775807"}"#).unwrap(),
        Value::I64(i64::MAX)
    );
    for input in [r#"{"i64":"+0001"}"#, r#"{"i64":"1"}"#] {
        assert_eq!(wire::decode_value(input).unwrap(), Value::I64(1));
    }
    for input in [r#"{"f64":"1.0"}"#, r#"{"f64":"1e0"}"#, r#"{"f64":"1"}"#] {
        assert_eq!(wire::decode_value(input).unwrap(), Value::f64(1.0).unwrap());
    }
    assert_eq!(
        wire::encode_value(&wire::decode_value(r#"{"f64":"-0.0"}"#).unwrap()),
        r#"{"f64":"0"}"#
    );
    for input in [r#"{"bytes":"AQI="}"#, r#"{"bytes":"AQI"}"#] {
        let value = wire::decode_value(input).unwrap();
        assert_eq!(value, Value::Bytes(vec![1, 2]));
        assert_eq!(wire::encode_value(&value), r#"{"bytes":"AQI="}"#);
    }
    assert_ne!(
        wire::decode_value(r#"{"i64":"1"}"#).unwrap(),
        wire::decode_value(r#"{"f64":"1"}"#).unwrap()
    );
}

#[test]
fn defaults_expectation_sets_snapshot_tokens_and_array_order_roundtrip() {
    let omitted = wire::decode_tx(r#"{"version":1}"#).unwrap();
    let explicit = wire::decode_tx(
        r#"{"metadata":{},"changes":[],"expect":[],"new_entities":[],"schema":[],"version":1}"#,
    )
    .unwrap();
    assert_eq!(omitted, Tx::default());
    assert_eq!(omitted, explicit);
    assert_eq!(
        wire::decode_tx(&wire::encode_tx(&omitted)).unwrap(),
        omitted
    );
    let snapshot: SnapshotId =
        "d1.00000000000000000000000000000001.00000000000000000000000000000002.18446744073709551615"
            .parse()
            .unwrap();
    assert_eq!(
        wire::decode_snapshot(&wire::encode_snapshot(&snapshot)).unwrap(),
        snapshot
    );
    for invalid in ["-1", "18446744073709551616", "+1", "1.0"] {
        let input = format!(
            r#"{{"version":1,"store":"{}","lineage":"{}","revision":"{invalid}"}}"#,
            snapshot.store(),
            snapshot.lineage()
        );
        assert_eq!(
            wire::decode_snapshot(&input).unwrap_err().code(),
            ErrorCode::InvalidSnapshot
        );
    }
    let padded = wire::encode_snapshot(&snapshot).replace("18446744073709551615", "0001");
    assert_eq!(
        wire::decode_snapshot(&padded).unwrap().revision(),
        Revision::new(1)
    );
    let request = format!(
        r#"{{"version":1,"new_entities":["z","a"],"expected_head":{},"idempotency_key":"key","expect":[{{"entity":{{"id":"{ENTITY}"}},"attr":"x/v","values":[{{"i64":"2"}},{{"i64":"1"}},{{"i64":"2"}}]}}],"changes":[{{"op":"assert","entity":{{"temp":"z"}},"attr":"x/v","value":{{"ref":{{"temp":"a"}}}}}},{{"op":"retract","entity":{{"id":"{ENTITY}"}},"attr":"x/v","value":{{"i64":"3"}}}}]}}"#,
        wire::encode_snapshot(&snapshot)
    );
    let tx = wire::decode_tx(&request).unwrap();
    assert_eq!(
        tx.expectations()[0].values,
        vec![Value::I64(1), Value::I64(2)]
    );
    assert_eq!(tx.new_entities()[0].as_str(), "z");
    assert_eq!(tx.new_entities()[1].as_str(), "a");
    assert_eq!(tx.expected_head(), Some(&snapshot));
    assert_eq!(tx.idempotency_key(), Some("key"));
    assert_eq!(wire::decode_tx(&wire::encode_tx(&tx)).unwrap(), tx);
}

#[test]
fn json_and_rust_requests_commit_to_identical_durable_outcomes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("parity.sqlite")).unwrap();
    let request = r#"{"version":1,"schema":[{"name":"x/value","type":"f64","cardinality":"one","unique":"none"}],"new_entities":["e"],"changes":[{"op":"assert","entity":{"temp":"e"},"attr":"x/value","value":{"f64":"-0.0"}}],"metadata":{"source":{"bytes":"AQI"}}}"#;
    let decoded = wire::decode_tx(request).unwrap();
    let label = TempId::new("e").unwrap();
    let rust = Tx::builder()
        .define_attribute(
            factlite::AttributeDefinition::new(
                "x/value".parse().unwrap(),
                factlite::ValueType::F64,
                factlite::Cardinality::One,
                factlite::Uniqueness::None,
            )
            .unwrap(),
        )
        .new_entity(label.clone())
        .assert(label, "x/value".parse().unwrap(), Value::f64(0.0).unwrap())
        .metadata("source", Value::Bytes(vec![1, 2]))
        .build()
        .unwrap();
    assert_eq!(decoded, rust);
    let report = store.commit(decoded).unwrap();
    assert_eq!(report.metadata["source"], Value::Bytes(vec![1, 2]));
    assert_eq!(
        store.db().unwrap().facts(FactFilter::default()).unwrap()[0].value,
        Value::f64(0.0).unwrap()
    );
    assert_eq!(wire::decode_tx(&wire::encode_tx(&rust)).unwrap(), rust);
}

fn values() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::I64),
        any::<u64>().prop_filter_map("finite", |bits| Value::f64(f64::from_bits(bits)).ok()),
        any::<String>().prop_map(Value::String),
        proptest::collection::vec(any::<u8>(), 0..96).prop_map(Value::Bytes),
        Just(Value::Ref(ENTITY.parse().unwrap())),
    ]
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn all_typed_values_roundtrip_exactly(value in values()) {
        prop_assert_eq!(wire::decode_value(&wire::encode_value(&value)).unwrap(),value);
    }
    #[test]
    fn arbitrary_wire_input_never_panics(input in any::<String>()) {
        let _ = wire::decode_tx(&input); let _ = wire::decode_snapshot(&input); let _ = wire::decode_value(&input);
    }
    #[test]
    fn owned_requests_roundtrip_without_reordering(values in proptest::collection::vec(values(),0..16)) {
        let label = TempId::new("e").unwrap();
        let mut builder = Tx::builder().new_entity(label.clone());
        for value in values { builder = builder.assert(label.clone(),"x/value".parse().unwrap(),value); }
        let tx = builder.build().unwrap();
        prop_assert_eq!(wire::decode_tx(&wire::encode_tx(&tx)).unwrap(),tx);
    }
}
