//! Read-request parts: exact round trips, strictness, and agreement with the
//! plain-text spellings the CLI takes from options.

use factlite::{AttrName, EntityId, ErrorCode, FactFilter, Revision, Value, wire};
use proptest::prelude::*;
use serde_json::json;

const ENTITY: &str = "e1.00000000000000000000000000000001.0000000000000001";

fn invalid(result: factlite::Result<impl std::fmt::Debug>, input: &str) {
    assert_eq!(
        result.unwrap_err().code(),
        ErrorCode::InvalidRequest,
        "{input}"
    );
}

#[test]
fn an_entity_is_exactly_one_id_and_never_a_label_or_lookup() {
    let entity: EntityId = ENTITY.parse().unwrap();
    assert_eq!(
        wire::decode_entity_id(&format!(r#"{{"id":"{ENTITY}"}}"#)).unwrap(),
        entity
    );
    assert_eq!(
        wire::encode_entity_id(&entity),
        format!(r#"{{"id":"{ENTITY}"}}"#)
    );
    for input in [
        "",
        "null",
        "{}",
        "[]",
        &format!(r#""{ENTITY}""#),
        &format!(r#"{{"id":"{ENTITY}","id":"{ENTITY}"}}"#),
        &format!(r#"{{"id":"{ENTITY}","extra":1}}"#),
        &format!(r#"{{"id":"{ENTITY}"}} trailing"#),
        r#"{"id":"e1.00000000000000000000000000000001.0000000000000000"}"#,
        r#"{"id":"E1.00000000000000000000000000000001.0000000000000001"}"#,
        r#"{"id":1}"#,
        r#"{"id":null}"#,
        r#"{"temp":"e"}"#,
        r#"{"lookup":{"attr":"task/id","value":{"string":"a"}}}"#,
    ] {
        invalid(wire::decode_entity_id(input), input);
    }
}

#[test]
fn attributes_are_strings_and_a_projection_keeps_order_and_repeats() {
    let name: AttrName = "task/title".parse().unwrap();
    assert_eq!(wire::decode_attr(r#""task/title""#).unwrap(), name);
    assert_eq!(wire::encode_attr(&name), r#""task/title""#);
    for input in [
        "",
        "task/title",
        r#""title""#,
        r#""task/""#,
        r#""/title""#,
        r#""a/b/c""#,
        r#""task/ti tle""#,
        r#"{"attr":"task/title"}"#,
        r#"["task/title"]"#,
        "null",
    ] {
        invalid(wire::decode_attr(input), input);
    }

    let other: AttrName = "task/status".parse().unwrap();
    let projection = vec![other.clone(), name.clone(), other.clone()];
    let text = r#"["task/status","task/title","task/status"]"#;
    assert_eq!(wire::decode_attrs(text).unwrap(), projection);
    assert_eq!(wire::encode_attrs(&projection), text);
    assert_eq!(wire::decode_attrs("[]").unwrap(), Vec::new());
    for input in [
        "",
        "null",
        "{}",
        r#""task/title""#,
        r#"["task/title",null]"#,
        r#"["task/title",1]"#,
        r#"["title"]"#,
        r#"[["task/title"]]"#,
    ] {
        invalid(wire::decode_attrs(input), input);
    }
}

#[test]
fn a_filter_takes_any_of_three_members_and_no_null_defaults() {
    assert_eq!(wire::decode_filter("{}").unwrap(), FactFilter::default());
    assert_eq!(wire::encode_filter(&FactFilter::default()), "{}");
    let full = FactFilter {
        entity: Some(ENTITY.parse().unwrap()),
        attribute: Some("task/priority".parse().unwrap()),
        value: Some(Value::I64(3)),
    };
    let text =
        format!(r#"{{"attr":"task/priority","entity":{{"id":"{ENTITY}"}},"value":{{"i64":"3"}}}}"#);
    assert_eq!(wire::decode_filter(&text).unwrap(), full);
    assert_eq!(wire::encode_filter(&full), text);
    // A reference value names an allocated entity, as in every concrete value.
    assert_eq!(
        wire::decode_filter(&format!(r#"{{"value":{{"ref":{{"id":"{ENTITY}"}}}}}}"#))
            .unwrap()
            .value,
        Some(Value::Ref(ENTITY.parse().unwrap()))
    );
    for input in [
        "",
        "null",
        "[]",
        r#"{"entity":null}"#,
        r#"{"attr":null}"#,
        r#"{"value":null}"#,
        r#"{"attribute":"task/priority"}"#,
        r#"{"attr":"task/priority","attr":"task/priority"}"#,
        r#"{"attr":"priority"}"#,
        r#"{"entity":"e1.00000000000000000000000000000001.0000000000000001"}"#,
        r#"{"entity":{"temp":"e"}}"#,
        r#"{"entity":{"lookup":{"attr":"task/id","value":{"string":"a"}}}}"#,
        r#"{"value":3}"#,
        r#"{"value":{"i64":3}}"#,
        r#"{"value":{"i64":"3","string":"3"}}"#,
        r#"{"value":{"ref":{"temp":"e"}}}"#,
        r#"{"value":{"ref":{"lookup":{"attr":"task/id","value":{"string":"a"}}}}}"#,
    ] {
        invalid(wire::decode_filter(input), input);
    }
}

#[test]
fn a_revision_is_a_string_of_canonical_decimal_digits() {
    assert_eq!(wire::decode_revision(r#""0""#).unwrap(), Revision::ZERO);
    assert_eq!(
        wire::decode_revision(r#""18446744073709551615""#).unwrap(),
        Revision::new(u64::MAX)
    );
    assert_eq!(wire::encode_revision(Revision::new(12)), r#""12""#);
    for input in [
        "",
        "null",
        "12",
        r#""""#,
        r#""012""#,
        r#""+12""#,
        r#""-1""#,
        r#""1.0""#,
        r#"" 12""#,
        r#""0x10""#,
        r#""18446744073709551616""#,
        r#"{"revision":"12"}"#,
    ] {
        invalid(wire::decode_revision(input), input);
    }
}

fn entities() -> impl Strategy<Value = EntityId> {
    (any::<u128>(), 1..=u64::MAX)
        .prop_map(|(store, allocation)| format!("e1.{store:032x}.{allocation:016x}"))
        .prop_filter_map("a parsable store binding", |text| text.parse().ok())
}
fn attributes() -> impl Strategy<Value = AttrName> {
    "[a-z0-9.*+!_?<>=雪-]{1,6}/[a-z0-9.*+!_?<>=雪-]{1,6}".prop_map(|text| text.parse().unwrap())
}
fn values() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::I64),
        any::<u64>().prop_filter_map("finite", |bits| Value::f64(f64::from_bits(bits)).ok()),
        any::<String>().prop_map(Value::String),
        proptest::collection::vec(any::<u8>(), 0..48).prop_map(Value::Bytes),
        entities().prop_map(Value::Ref),
        "([a-z0-9.*+!_?<>=雪-]{1,6}/)?[a-z0-9.*+!_?<>=雪-]{1,6}"
            .prop_map(|text| Value::keyword(text).unwrap()),
        any::<i64>().prop_map(Value::Inst),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn entities_attributes_and_revisions_roundtrip_exactly(
        entity in entities(),
        attribute in attributes(),
        projection in proptest::collection::vec(attributes(), 0..8),
        revision in any::<u64>(),
    ) {
        prop_assert_eq!(&wire::decode_entity_id(&wire::encode_entity_id(&entity)).unwrap(), &entity);
        prop_assert_eq!(&wire::decode_attr(&wire::encode_attr(&attribute)).unwrap(), &attribute);
        prop_assert_eq!(&wire::decode_attrs(&wire::encode_attrs(&projection)).unwrap(), &projection);
        let revision = Revision::new(revision);
        prop_assert_eq!(wire::decode_revision(&wire::encode_revision(revision)).unwrap(), revision);
    }

    #[test]
    fn filters_roundtrip_exactly(
        entity in proptest::option::of(entities()),
        attribute in proptest::option::of(attributes()),
        value in proptest::option::of(values()),
    ) {
        let filter = FactFilter { entity, attribute, value };
        prop_assert_eq!(&wire::decode_filter(&wire::encode_filter(&filter)).unwrap(), &filter);
    }

    /// The JSON spellings accept exactly the text the CLI's options accept,
    /// and mean the same thing.
    #[test]
    fn json_spellings_agree_with_the_plain_text_the_cli_parses(
        text in prop_oneof![
            any::<String>(),
            "e1\\.[0-9a-f]{32}\\.[0-9a-f]{16}",
            "[a-z /]{0,8}",
            "[0-9]{1,21}",
        ],
    ) {
        let entity = wire::decode_entity_id(&json!({"id": text}).to_string());
        prop_assert_eq!(entity.ok(), text.parse::<EntityId>().ok());
        let attribute = wire::decode_attr(&json!(text).to_string());
        prop_assert_eq!(attribute.ok(), text.parse::<AttrName>().ok());
        let revision = wire::decode_revision(&json!(text).to_string());
        prop_assert_eq!(revision.ok(), text.parse::<Revision>().ok());
    }

    #[test]
    fn arbitrary_read_input_never_panics(input in any::<String>()) {
        let _ = wire::decode_entity_id(&input);
        let _ = wire::decode_attr(&input);
        let _ = wire::decode_attrs(&input);
        let _ = wire::decode_filter(&input);
        let _ = wire::decode_revision(&input);
    }
}
