//! Historical schema, exact type domains, and query structure validation.

use std::sync::OnceLock;

use factlite::{
    AttributeDefinition, Cardinality, Comparison, Direction, ErrorCode, Order, Params, Pattern,
    Predicate, Query, QueryName, Schema, Store, Term, Tx, Uniqueness, Value, ValueType,
};
use proptest::prelude::*;

fn name(value: &str) -> QueryName {
    QueryName::new(value).unwrap()
}
fn var(value: &str) -> Term {
    Term::Var(name(value))
}
fn pattern(attribute: &str, value: Term) -> Pattern {
    Pattern {
        entity: var("e"),
        attribute: attribute.parse().unwrap(),
        value,
    }
}
fn definition(attribute: &str, ty: ValueType) -> AttributeDefinition {
    AttributeDefinition::new(
        attribute.parse().unwrap(),
        ty,
        Cardinality::Many,
        Uniqueness::None,
    )
    .unwrap()
}
const TYPES: [ValueType; 8] = [
    ValueType::Bool,
    ValueType::I64,
    ValueType::F64,
    ValueType::String,
    ValueType::Bytes,
    ValueType::Ref,
    ValueType::Keyword,
    ValueType::Inst,
];
fn schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("schema.sqlite")).unwrap();
        let mut tx = Tx::builder();
        for (index, ty) in TYPES.into_iter().enumerate() {
            tx = tx.define_attribute(definition(&format!("x/type{index}"), ty));
        }
        store.commit(tx.build().unwrap()).unwrap();
        store.db().unwrap().schema().unwrap()
    })
}
fn query() -> Query {
    Query {
        patterns: vec![pattern("x/type1", var("value"))],
        select: vec![name("e"), name("value")],
        ..Query::default()
    }
}

#[test]
fn ref_value_variables_join_entity_domain_and_validation_uses_historical_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let store = Store::open(&path).unwrap();
    let zero = store.db().unwrap();
    let a = store
        .commit(
            Tx::builder()
                .define_attribute(definition("task/owner", ValueType::Ref))
                .define_attribute(definition("person/name", ValueType::String))
                .build()
                .unwrap(),
        )
        .unwrap();
    let db_a = store.db().unwrap();
    let joined = Query {
        patterns: vec![
            Pattern {
                entity: var("task"),
                attribute: "task/owner".parse().unwrap(),
                value: var("person"),
            },
            Pattern {
                entity: var("person"),
                attribute: "person/name".parse().unwrap(),
                value: var("name"),
            },
        ],
        select: vec![name("task"), name("name")],
        order: vec![Order {
            field: name("task"),
            direction: Direction::Asc,
        }],
        limit: Some(20),
        ..Query::default()
    };
    assert_eq!(
        zero.validate_query(&joined, &Params::new())
            .unwrap_err()
            .code(),
        ErrorCode::UnknownAttribute
    );
    db_a.validate_query(&joined, &Params::new()).unwrap();
    store
        .commit(
            Tx::builder()
                .define_attribute(definition("task/number", ValueType::I64))
                .build()
                .unwrap(),
        )
        .unwrap();
    let later = Query {
        patterns: vec![pattern("task/number", var("value"))],
        select: vec![name("value")],
        ..Query::default()
    };
    assert_eq!(
        db_a.validate_query(&later, &Params::new())
            .unwrap_err()
            .code(),
        ErrorCode::UnknownAttribute
    );
    store
        .db()
        .unwrap()
        .validate_query(&later, &Params::new())
        .unwrap();
    let invalid_join = Query {
        patterns: vec![
            Pattern {
                entity: var("task"),
                attribute: "task/number".parse().unwrap(),
                value: var("person"),
            },
            Pattern {
                entity: var("person"),
                attribute: "person/name".parse().unwrap(),
                value: var("name"),
            },
        ],
        ..Query::default()
    };
    assert_eq!(
        store
            .db()
            .unwrap()
            .validate_query(&invalid_join, &Params::new())
            .unwrap_err()
            .code(),
        ErrorCode::TypeMismatch
    );
    db_a.validate_query(&joined, &Params::new()).unwrap();
    drop((zero, db_a, store));
    let reopened = Store::open(&path).unwrap().db_at(&a.after).unwrap();
    reopened.validate_query(&joined, &Params::new()).unwrap();
    assert_eq!(
        reopened
            .validate_query(&later, &Params::new())
            .unwrap_err()
            .code(),
        ErrorCode::UnknownAttribute
    );
}

#[test]
fn parameters_and_constants_keep_types_without_entity_or_numeric_coercion() {
    let mut q = query();
    q.inputs.insert(name("wanted"), ValueType::I64);
    q.patterns[0].value = Term::Param(name("wanted"));
    q.select = vec![name("e")];
    let mut params = Params::from([(name("wanted"), Value::I64(1))]);
    q.validate(schema(), &params).unwrap();
    params.insert(name("wanted"), Value::f64(1.0).unwrap());
    assert_eq!(
        q.validate(schema(), &params).unwrap_err().code(),
        ErrorCode::TypeMismatch
    );
    assert_eq!(
        q.validate(schema(), &Params::new()).unwrap_err().code(),
        ErrorCode::InvalidRequest
    );
    params.insert(name("wanted"), Value::I64(1));
    params.insert(name("extra"), Value::I64(1));
    assert_eq!(
        q.validate(schema(), &params).unwrap_err().code(),
        ErrorCode::InvalidRequest
    );
    q.inputs.clear();
    assert_eq!(
        q.validate(schema(), &Params::new()).unwrap_err().code(),
        ErrorCode::InvalidRequest
    );
    q.patterns[0].value = Term::Const(Value::f64(1.0).unwrap());
    assert_eq!(
        q.validate(schema(), &Params::new()).unwrap_err().code(),
        ErrorCode::TypeMismatch
    );
    q.patterns[0].value = Term::Const(Value::I64(1));
    q.select.clear();
    q.patterns[0].entity = Term::Const(Value::String("e1".into()));
    assert_eq!(
        q.validate(schema(), &Params::new()).unwrap_err().code(),
        ErrorCode::TypeMismatch
    );
    q.patterns[0].entity = Term::Const(Value::Ref(
        "e1.00000000000000000000000000000001.ffffffffffffffff"
            .parse()
            .unwrap(),
    ));
    // Query constants need the correct type, not durable allocation membership.
    q.validate(schema(), &Params::new()).unwrap();
    q.inputs.insert(name("unused"), ValueType::Bool);
    assert_eq!(
        q.validate(schema(), &Params::new()).unwrap_err().code(),
        ErrorCode::InvalidRequest
    );
    q.validate(
        schema(),
        &Params::from([(name("unused"), Value::Bool(false))]),
    )
    .unwrap();
}

#[test]
fn unbound_comparisons_duplicate_columns_and_unselected_order_fail_even_with_zero_limit() {
    let original = query();
    for mut q in [
        Query {
            select: vec![name("missing")],
            ..original.clone()
        },
        Query {
            select: vec![name("value"), name("value")],
            ..original.clone()
        },
        Query {
            select: vec![name("value")],
            order: vec![Order {
                field: name("e"),
                direction: Direction::Asc,
            }],
            ..original.clone()
        },
        Query {
            predicates: vec![Predicate {
                left: var("missing"),
                comparison: Comparison::Eq,
                right: Term::Const(Value::I64(1)),
            }],
            ..original.clone()
        },
        Query {
            predicates: vec![Predicate {
                left: var("value"),
                comparison: Comparison::Eq,
                right: Term::Param(name("missing")),
            }],
            ..original.clone()
        },
    ] {
        q.limit = Some(0);
        assert_eq!(
            q.validate(schema(), &Params::new()).unwrap_err().code(),
            ErrorCode::InvalidRequest
        );
    }
    let mut q = original;
    q.limit = Some(u64::MAX);
    q.validate(schema(), &Params::new()).unwrap();
    q.limit = Some(0);
    q.validate(schema(), &Params::new()).unwrap();
    Query::default().validate(schema(), &Params::new()).unwrap();
    assert!(QueryName::new("").is_err());
    assert!(QueryName::new("a\0b").is_err());
    assert_ne!(name("é"), name("e\u{301}"));
    assert_eq!(
        name("x'; DROP TABLE facts;--").as_str(),
        "x'; DROP TABLE facts;--"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn shared_variable_types_agree_iff_historical_domains_agree(a in 0_usize..8,b in 0_usize..8,reverse in any::<bool>()) {
        let mut patterns = vec![pattern(&format!("x/type{a}"),var("value")),pattern(&format!("x/type{b}"),var("value"))];
        if reverse { patterns.reverse(); }
        let q = Query { patterns,select: vec![name("e"),name("value")],..Query::default() };
        let result = q.validate(schema(),&Params::new());
        prop_assert_eq!(result.is_ok(),a == b);
        if a != b { prop_assert_eq!(result.unwrap_err().code(),ErrorCode::TypeMismatch); }
    }
    #[test]
    fn typed_ranges_require_equal_domains_while_equality_preserves_distinctions(a in 0_usize..8,b in 0_usize..8,operator in 0_usize..6) {
        let comparison = [Comparison::Eq,Comparison::Ne,Comparison::Lt,Comparison::Le,Comparison::Gt,Comparison::Ge][operator];
        let q = Query { patterns: vec![pattern(&format!("x/type{a}"),var("a")),pattern(&format!("x/type{b}"),var("b"))],predicates: vec![Predicate { left: var("a"),comparison,right: var("b") }],..Query::default() };
        let result = q.validate(schema(),&Params::new());
        prop_assert_eq!(result.is_ok(),operator < 2 || a == b);
        if operator >= 2 && a != b { prop_assert_eq!(result.unwrap_err().code(),ErrorCode::TypeMismatch); }
    }
}
