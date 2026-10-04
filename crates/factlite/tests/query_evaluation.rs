//! Public joins, distinct/order/limit, and independently computed relations.

use std::collections::BTreeSet;

use factlite::{
    AttributeDefinition, Cardinality, Comparison, Direction, ErrorCode, Order, Params, Pattern,
    Predicate, Query, QueryName, Revision, Store, TempId, Term, Tx, Uniqueness, Value, ValueTerm,
    ValueType, wire,
};
use proptest::prelude::*;

fn name(value: &str) -> QueryName {
    QueryName::new(value).unwrap()
}
fn var(value: &str) -> Term {
    Term::Var(name(value))
}
fn pattern(entity: Term, attribute: &str, value: Term) -> Pattern {
    Pattern {
        entity,
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
fn order(field: &str, direction: Direction) -> Order {
    Order {
        field: name(field),
        direction,
    }
}
fn joined() -> Query {
    Query {
        inputs: [(name("wanted"), ValueType::String)].into(),
        patterns: vec![
            pattern(var("task"), "task/status", Term::Param(name("wanted"))),
            pattern(var("task"), "task/title", var("title")),
            pattern(var("task"), "task/owner", var("person")),
            pattern(var("person"), "person/name", var("name")),
        ],
        select: vec![name("task"), name("title"), name("name")],
        order: vec![
            order("title", Direction::Asc),
            order("task", Direction::Asc),
        ],
        limit: Some(20),
        ..Query::default()
    }
}

#[test]
fn design_join_is_empty_at_a_and_returns_typed_task_title_owner_at_b_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("joined.sqlite");
    let store = Store::open(&path).unwrap();
    let a = store
        .commit(wire::decode_tx(include_str!("../../../examples/create.json")).unwrap())
        .unwrap();
    let task = a.tempids[&TempId::new("task").unwrap()].clone();
    let db_a = store.db().unwrap();
    let params = [(name("wanted"), Value::String("doing".into()))].into();
    let query = joined();
    assert!(db_a.query(&query, &params).unwrap().rows.is_empty());
    let b = store
        .commit(
            Tx::builder()
                .expect(
                    task.clone(),
                    "task/status".parse().unwrap(),
                    vec![Value::String("todo".into())],
                )
                .assert(
                    task.clone(),
                    "task/status".parse().unwrap(),
                    Value::String("doing".into()),
                )
                .build()
                .unwrap(),
        )
        .unwrap();
    let db_b = store.db().unwrap();
    let expected = vec![vec![
        Value::Ref(task),
        Value::String("Design Factlite".into()),
        Value::String("Ada".into()),
    ]];
    assert_eq!(db_b.query(&query, &params).unwrap().rows, expected);
    assert!(db_a.query(&query, &params).unwrap().rows.is_empty());
    drop((db_a, db_b, store));
    let reopened = Store::open(&path).unwrap();
    assert!(
        reopened
            .db_at(&a.after)
            .unwrap()
            .query(&query, &params)
            .unwrap()
            .rows
            .is_empty()
    );
    assert_eq!(
        reopened
            .db_at(&b.after)
            .unwrap()
            .query(&query, &params)
            .unwrap()
            .rows,
        expected
    );
}

#[test]
fn many_combinations_project_distinct_before_order_limit_and_absent_facts_fail_match() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("many.sqlite")).unwrap();
    let label = TempId::new("e").unwrap();
    store
        .commit(
            Tx::builder()
                .define_attribute(definition("x/left", ValueType::I64))
                .define_attribute(definition("x/right", ValueType::I64))
                .define_attribute(definition("x/absent", ValueType::I64))
                .new_entity(label.clone())
                .assert(label.clone(), "x/left".parse().unwrap(), Value::I64(1))
                .assert(label.clone(), "x/left".parse().unwrap(), Value::I64(2))
                .assert(label.clone(), "x/right".parse().unwrap(), Value::I64(2))
                .assert(label, "x/right".parse().unwrap(), Value::I64(3))
                .build()
                .unwrap(),
        )
        .unwrap();
    let db = store.db().unwrap();
    let mut q = Query {
        patterns: vec![
            pattern(var("e"), "x/left", var("a")),
            pattern(var("e"), "x/right", var("b")),
        ],
        predicates: vec![Predicate {
            left: var("a"),
            comparison: Comparison::Lt,
            right: var("b"),
        }],
        select: vec![name("a"), name("b")],
        order: vec![order("a", Direction::Asc), order("b", Direction::Asc)],
        ..Query::default()
    };
    assert_eq!(
        db.query(&q, &Params::new()).unwrap().rows,
        vec![
            vec![Value::I64(1), Value::I64(2)],
            vec![Value::I64(1), Value::I64(3)],
            vec![Value::I64(2), Value::I64(3)]
        ]
    );
    q.select = vec![name("a")];
    q.order = vec![order("a", Direction::Desc)];
    q.limit = Some(2);
    assert_eq!(
        db.query(&q, &Params::new()).unwrap().rows,
        vec![vec![Value::I64(2)], vec![Value::I64(1)]]
    );
    q.limit = Some(1);
    assert_eq!(
        db.query(&q, &Params::new()).unwrap().rows,
        vec![vec![Value::I64(2)]]
    );
    q.limit = Some(0);
    assert!(db.query(&q, &Params::new()).unwrap().rows.is_empty());
    q.patterns.push(pattern(var("e"), "x/missing", var("c")));
    assert_eq!(
        db.query(&q, &Params::new()).unwrap_err().code(),
        ErrorCode::UnknownAttribute
    );
    q.patterns.last_mut().unwrap().attribute = "x/absent".parse().unwrap();
    q.limit = None;
    assert!(db.query(&q, &Params::new()).unwrap().rows.is_empty());
}

#[test]
fn all_types_compare_and_sort_without_numeric_coercion_or_float_zero_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("typed.sqlite")).unwrap();
    let allocated = store
        .commit(
            Tx::builder()
                .new_entity(TempId::new("a").unwrap())
                .new_entity(TempId::new("b").unwrap())
                .build()
                .unwrap(),
        )
        .unwrap();
    let id = allocated.created_entities[0].clone();
    let cases = [
        vec![Value::Bool(false), Value::Bool(true)],
        vec![Value::I64(i64::MIN), Value::I64(0), Value::I64(i64::MAX)],
        vec![
            Value::f64(-f64::MAX).unwrap(),
            Value::f64(-0.0).unwrap(),
            Value::f64(f64::MAX).unwrap(),
        ],
        vec![
            Value::String("\0".into()),
            Value::String("a".into()),
            Value::String("é".into()),
        ],
        vec![
            Value::Bytes(vec![]),
            Value::Bytes(vec![0]),
            Value::Bytes(vec![255]),
        ],
        vec![
            Value::Ref(id.clone()),
            Value::Ref(allocated.created_entities[1].clone()),
        ],
    ];
    for (index, values) in cases.iter().enumerate() {
        let attr = format!("x/type{index}");
        let mut tx = Tx::builder().define_attribute(definition(&attr, values[0].value_type()));
        for value in values.iter().rev() {
            tx = tx.assert(id.clone(), attr.parse().unwrap(), value.clone());
        }
        store.commit(tx.build().unwrap()).unwrap();
        for (op, comparison) in [
            Comparison::Eq,
            Comparison::Ne,
            Comparison::Lt,
            Comparison::Le,
            Comparison::Gt,
            Comparison::Ge,
        ]
        .into_iter()
        .enumerate()
        {
            let q = Query {
                inputs: [(name("wanted"), values[0].value_type())].into(),
                patterns: vec![pattern(
                    Term::Const(Value::Ref(id.clone())),
                    &attr,
                    var("v"),
                )],
                predicates: vec![Predicate {
                    left: var("v"),
                    comparison,
                    right: Term::Param(name("wanted")),
                }],
                select: vec![name("v")],
                order: vec![order("v", Direction::Asc)],
                ..Query::default()
            };
            let expected: Vec<_> = values
                .iter()
                .enumerate()
                .filter(|(index, _)| match op {
                    0 => *index == 1,
                    1 => *index != 1,
                    2 => *index < 1,
                    3 => *index <= 1,
                    4 => *index > 1,
                    5 => *index >= 1,
                    _ => unreachable!(),
                })
                .map(|(_, value)| vec![value.clone()])
                .collect();
            assert_eq!(
                store
                    .db()
                    .unwrap()
                    .query(&q, &[(name("wanted"), values[1].clone())].into())
                    .unwrap()
                    .rows,
                expected
            );
        }
    }
    for comparison in [Comparison::Eq, Comparison::Ne] {
        let q = Query {
            predicates: vec![Predicate {
                left: Term::Const(Value::I64(1)),
                comparison,
                right: Term::Const(Value::f64(1.0).unwrap()),
            }],
            ..Query::default()
        };
        let result = store.db().unwrap().query(&q, &Params::new()).unwrap();
        assert_eq!(
            result.rows,
            if comparison == Comparison::Eq {
                vec![]
            } else {
                vec![vec![]]
            }
        );
    }
    assert_eq!(
        store
            .db()
            .unwrap()
            .query(&Query::default(), &Params::new())
            .unwrap()
            .rows,
        vec![vec![]]
    );
}

#[test]
fn repeated_entity_ref_variable_requires_self_edge_and_empty_projection_is_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("graph.sqlite")).unwrap();
    let a = TempId::new("a").unwrap();
    let b = TempId::new("b").unwrap();
    let report = store
        .commit(
            Tx::builder()
                .define_attribute(definition("x/link", ValueType::Ref))
                .new_entity(a.clone())
                .new_entity(b.clone())
                .assert(
                    a.clone(),
                    "x/link".parse().unwrap(),
                    ValueTerm::TempRef(b.clone()),
                )
                .assert(b.clone(), "x/link".parse().unwrap(), ValueTerm::TempRef(a))
                .assert(
                    b.clone(),
                    "x/link".parse().unwrap(),
                    ValueTerm::TempRef(b.clone()),
                )
                .build()
                .unwrap(),
        )
        .unwrap();
    let mut q = Query {
        patterns: vec![pattern(var("e"), "x/link", var("e"))],
        select: vec![name("e")],
        ..Query::default()
    };
    assert_eq!(
        store.db().unwrap().query(&q, &Params::new()).unwrap().rows,
        vec![vec![Value::Ref(report.tempids[&b].clone())]]
    );
    q.select.clear();
    assert_eq!(
        store.db().unwrap().query(&q, &Params::new()).unwrap().rows,
        vec![vec![]]
    );
    let unknown = format!(
        "e1.{}.ffffffffffffffff",
        report
            .after
            .store()
            .to_string()
            .strip_prefix("s1.")
            .unwrap()
    )
    .parse()
    .unwrap();
    q.patterns[0].entity = Term::Const(Value::Ref(unknown));
    assert!(
        store
            .db()
            .unwrap()
            .query(&q, &Params::new())
            .unwrap()
            .rows
            .is_empty()
    );
    assert_eq!(store.head().unwrap().revision(), Revision::new(1));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn many_joins_filter_project_distinct_order_limit_match_cartesian_set_oracle(
        left in proptest::collection::vec(-3_i64..4,0..8), right in proptest::collection::vec(-3_i64..4,0..8),
        project_pair in any::<bool>(), descending in any::<bool>(), limit in 0_usize..12,
    ) {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("property.sqlite"); let store = Store::open(&path).unwrap();
        let label = TempId::new("e").unwrap();
        let mut tx = Tx::builder().define_attribute(definition("x/left",ValueType::I64)).define_attribute(definition("x/right",ValueType::I64)).new_entity(label.clone());
        for value in &left { tx = tx.assert(label.clone(),"x/left".parse().unwrap(),Value::I64(*value)); }
        for value in &right { tx = tx.assert(label.clone(),"x/right".parse().unwrap(),Value::I64(*value)); }
        let a = store.commit(tx.build().unwrap()).unwrap();
        let direction = if descending { Direction::Desc } else { Direction::Asc };
        let q = Query { patterns: vec![pattern(var("e"),"x/left",var("a")),pattern(var("e"),"x/right",var("b"))],predicates: vec![Predicate { left: var("a"),comparison: Comparison::Lt,right: var("b") }],select: if project_pair { vec![name("a"),name("b")] } else { vec![name("a")] },order: if project_pair { vec![order("a",direction),order("b",direction)] } else { vec![order("a",direction)] },limit: Some(limit as u64),..Query::default() };
        let mut oracle = BTreeSet::new();
        for a in left { for b in &right { if a < *b { oracle.insert(if project_pair {vec![a,*b]} else {vec![a]}); } } }
        let mut expected: Vec<_> = oracle.into_iter().collect(); if descending { expected.reverse(); } expected.truncate(limit);
        let expected: Vec<Vec<_>> = expected.into_iter().map(|row| row.into_iter().map(Value::I64).collect()).collect();
        let db_a = store.db().unwrap(); let result = db_a.query(&q,&Params::new()).unwrap();
        prop_assert_eq!(&result.rows,&expected);
        store.commit(Tx::builder().assert(a.created_entities[0].clone(),"x/left".parse().unwrap(),Value::I64(99)).build().unwrap()).unwrap();
        prop_assert_eq!(db_a.query(&q,&Params::new()).unwrap(),result.clone());
        drop((db_a,store)); let recovered = Store::open(&path).unwrap().db_at(&a.after).unwrap();
        prop_assert_eq!(recovered.query(&q,&Params::new()).unwrap(),result);
    }
}
