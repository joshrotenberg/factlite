//! Strict query interchange and independently constructed Rust/JSON parity.
use factlite::{
    Aggregate, AggregateFn, Comparison, Direction, ErrorCode, Negation, Order, Params, Pattern,
    Predicate, Query, QueryName, Term, Value, wire,
};
use proptest::prelude::*;
use serde_json::json;

const ENTITY: &str = "e1.00000000000000000000000000000001.0000000000000001";
fn name(text: &str) -> QueryName {
    QueryName::new(text).unwrap()
}

#[test]
fn defaults_limits_and_design_fixture_equal_rust_ast() {
    let minimal = r#"{"version":1,"query":{"match":[],"select":[]}}"#;
    assert_eq!(
        wire::decode_query(minimal).unwrap(),
        (Query::default(), Params::new())
    );
    let explicit = r#"{"version":1,"params":{},"query":{"inputs":{},"match":[],"where":[],"select":[],"order":[]}}"#;
    assert_eq!(
        wire::decode_query(minimal).unwrap(),
        wire::decode_query(explicit).unwrap()
    );
    for limit in [0, u64::MAX] {
        let q = Query {
            limit: Some(limit),
            ..Query::default()
        };
        assert_eq!(
            wire::decode_query(&wire::encode_query(&q, &Params::new()))
                .unwrap()
                .0,
            q
        );
    }
    let pattern = |e: &str, a: &str, v: Term| Pattern {
        entity: Term::Var(name(e)),
        attribute: a.parse().unwrap(),
        value: v,
    };
    let q = Query {
        inputs: [(name("wanted"), factlite::ValueType::String)].into(),
        patterns: vec![
            pattern("task", "task/status", Term::Param(name("wanted"))),
            pattern("task", "task/title", Term::Var(name("title"))),
            pattern("task", "task/owner", Term::Var(name("person"))),
            pattern("person", "person/name", Term::Var(name("name"))),
        ],
        select: vec![name("task"), name("title"), name("name")],
        order: vec![
            Order {
                field: name("title"),
                direction: Direction::Asc,
            },
            Order {
                field: name("task"),
                direction: Direction::Asc,
            },
        ],
        limit: Some(20),
        ..Query::default()
    };
    assert_eq!(
        wire::decode_query(include_str!("../../../examples/query.json")).unwrap(),
        (q, [(name("wanted"), Value::String("doing".into()))].into())
    );
}

#[test]
fn duplicate_keys_unknown_fields_and_malformed_terms_are_rejected() {
    for input in [
        "",
        "null",
        "[]",
        "{}",
        r#"{"version":1,"query":{}}"#,
        r#"{"version":1,"version":1,"query":{"match":[],"select":[]}}"#,
        r#"{"version":1,"query":{"match":[],"match":[],"select":[]}}"#,
        r#"{"version":1,"query":{"match":[],"select":[],"inputs":{"x":"i64","x":"i64"}}}"#,
        r#"{"version":1,"query":{"match":[],"select":[]},"params":{"x":{"i64":"1"},"x":{"i64":"1"}}}"#,
        r#"{"version":1,"query":{"match":[[{"var":"e","\u0076ar":"e"},"x/v",{"i64":"1"}]],"select":[]}}"#,
        r#"{"version":1,"query":{"match":[],"select":[]},"params":{"x":{"i64":"1","i64":"1"}}}"#,
    ] {
        assert_eq!(
            wire::decode_query(input).unwrap_err().code(),
            ErrorCode::InvalidRequest,
            "{input}"
        );
    }
    let base = json!({"version":1,"query":{"match":[],"select":[]}});
    let cases = [
        ("/extra", json!(true)),
        ("/version", json!(2)),
        ("/version", json!(1.0)),
        ("/params", json!(null)),
        ("/params", json!({"x":{"ref":{"temp":"e"}}})),
        ("/query/extra", json!(true)),
        ("/query/match", json!(null)),
        ("/query/select", json!(null)),
        ("/query/inputs", json!(null)),
        ("/query/inputs", json!({"x":"number"})),
        ("/query/inputs", json!({"":"i64"})),
        ("/query/match", json!([[]])),
        ("/query/match", json!([{}])),
        (
            "/query/match",
            json!([[{"var":"e","param":"e"},"x/v",{"i64":"1"}]]),
        ),
        (
            "/query/match",
            json!([[{"var":"e","extra":true},"x/v",{"i64":"1"}]]),
        ),
        ("/query/match", json!([[{"var":""},"x/v",{"i64":"1"}]])),
        (
            "/query/match",
            json!([[{"var":"e"},{"var":"attr"},{"i64":"1"}]]),
        ),
        (
            "/query/match",
            json!([[{"ref":{"temp":"e"}},"x/v",{"i64":"1"}]]),
        ),
        ("/query/where", json!(null)),
        (
            "/query/where",
            json!([{"left":{"i64":"1"},"op":"like","right":{"i64":"1"}}]),
        ),
        (
            "/query/where",
            json!([{"left":{"i64":"1"},"op":"eq","right":{"i64":"1"},"extra":true}]),
        ),
        ("/query/order", json!([{"field":"e","direction":"up"}])),
        (
            "/query/order",
            json!([{"field":"e","direction":"asc","extra":true}]),
        ),
        ("/query/select", json!(["\u{0}"])),
        ("/query/limit", json!(-1)),
        ("/query/limit", json!(1.0)),
        ("/query/limit", json!("1")),
        ("/query/limit", json!(null)),
    ];
    for (pointer, value) in cases {
        let mut input = base.clone();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        input
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(key.into(), value);
        assert_eq!(
            wire::decode_query(&input.to_string()).unwrap_err().code(),
            ErrorCode::InvalidRequest,
            "{input}"
        );
    }
    let overflow = r#"{"version":1,"query":{"match":[],"select":[],"limit":18446744073709551616}}"#;
    assert_eq!(
        wire::decode_query(overflow).unwrap_err().code(),
        ErrorCode::InvalidRequest
    );
}

fn values() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::I64),
        any::<u64>().prop_filter_map("finite", |bits| Value::f64(f64::from_bits(bits)).ok()),
        any::<String>().prop_map(Value::String),
        proptest::collection::vec(any::<u8>(), 0..64).prop_map(Value::Bytes),
        Just(Value::Ref(ENTITY.parse().unwrap())),
        "([a-z0-9.*+!_?<>=雪-]{1,6}/)?[a-z0-9.*+!_?<>=雪-]{1,6}"
            .prop_map(|text| Value::keyword(text).unwrap()),
        any::<i64>().prop_map(Value::Inst),
    ]
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn query_terms_params_predicates_order_and_limits_roundtrip(value in values(), other in values(), limit in any::<Option<u64>>(), comparison in 0_usize..6, descending in any::<bool>(), aggregate in 0_usize..7, with in any::<bool>(), negation in 0_usize..3, having in any::<bool>()) {
        let op = [Comparison::Eq,Comparison::Ne,Comparison::Lt,Comparison::Le,Comparison::Gt,Comparison::Ge][comparison];
        let q = Query { inputs: [(name("input'雪"),value.value_type())].into(), patterns: vec![Pattern { entity: Term::Const(Value::Ref(ENTITY.parse().unwrap())),attribute:"x/value".parse().unwrap(),value:Term::Param(name("input'雪")) },Pattern { entity: Term::Var(name("entity")),attribute:"x/value".parse().unwrap(),value:Term::Var(name("v")) }], predicates: vec![Predicate { left:Term::Const(other.clone()),comparison:op,right:Term::Param(name("input'雪")) }], select: vec![name("v"),name("entity")],order:vec![Order { field:name("v"),direction:if descending {Direction::Desc} else {Direction::Asc} }],limit,
            with: if with { vec![name("w"), name("w2")] } else { Vec::new() },
            // None, `not` with a predicate only, or `not-join` with a pattern
            // and a predicate on its local.
            negations: [
                Vec::new(),
                vec![Negation { join: None, patterns: Vec::new(), predicates: vec![Predicate { left: Term::Var(name("v")), comparison: op, right: Term::Const(other.clone()) }] }],
                vec![Negation { join: Some(vec![name("entity"), name("v")]), patterns: vec![Pattern { entity: Term::Var(name("entity")), attribute: "x/other".parse().unwrap(), value: Term::Var(name("local")) }], predicates: vec![Predicate { left: Term::Var(name("local")), comparison: op, right: Term::Param(name("input'雪")) }] }],
            ][negation].clone(),
            aggregates: [None, Some(AggregateFn::Count), Some(AggregateFn::CountDistinct), Some(AggregateFn::Sum), Some(AggregateFn::Min), Some(AggregateFn::Max), Some(AggregateFn::Avg)][aggregate]
                .map(|function| vec![Aggregate { function, var: name("v"), name: name("total") }, Aggregate { function, var: name("entity"), name: name("second") }])
                .unwrap_or_default(),
            having: if having { vec![Predicate { left: Term::Var(name("total")), comparison: op, right: Term::Param(name("input'雪")) }, Predicate { left: Term::Const(other.clone()), comparison: Comparison::Ne, right: Term::Var(name("second")) }] } else { Vec::new() } };
        let params = [(name("input'雪"),value)].into();
        prop_assert_eq!(wire::decode_query(&wire::encode_query(&q,&params)).unwrap(),(q,params));
    }
    #[test]
    fn arbitrary_query_json_never_panics(input in any::<String>()) { let _ = wire::decode_query(&input); }
}
