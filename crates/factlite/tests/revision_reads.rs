//! Historical public reads, effective-event bounds, and an independent set oracle.

use std::collections::BTreeSet;

use factlite::{
    AttrName, AttributeDefinition, Cardinality, ErrorCode, FactFilter, Operation, PulledValue,
    Revision, Store, TempId, Tx, Uniqueness, Value, ValueType,
};
use proptest::prelude::*;

fn attr(name: &str) -> AttrName {
    name.parse().unwrap()
}
fn def(name: &str, ty: ValueType, cardinality: Cardinality) -> AttributeDefinition {
    AttributeDefinition::new(attr(name), ty, cardinality, Uniqueness::None).unwrap()
}

#[test]
fn historical_catalog_distinguishes_unknown_empty_absent_and_retracted() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("catalog.sqlite")).unwrap();
    let zero = store.db().unwrap();
    let label = TempId::new("empty").unwrap();
    let a = store
        .commit(
            Tx::builder()
                .define_attribute(def("x/one", ValueType::I64, Cardinality::One))
                .new_entity(label.clone())
                .build()
                .unwrap(),
        )
        .unwrap();
    let id = a.tempids[&label].clone();
    let db_a = store.db_at(&a.after).unwrap();
    assert_eq!(zero.pull(&id, &[]).unwrap(), None);
    assert!(
        db_a.pull(&id, &[attr("x/one")])
            .unwrap()
            .unwrap()
            .attributes
            .is_empty()
    );
    let b = store
        .commit(
            Tx::builder()
                .assert(id.clone(), attr("x/one"), Value::I64(7))
                .define_attribute(def("x/later", ValueType::Bool, Cardinality::One))
                .build()
                .unwrap(),
        )
        .unwrap();
    let db_b = store.db().unwrap();
    assert_eq!(
        db_b.pull(&id, &[attr("x/one"), attr("x/one"), attr("x/later")])
            .unwrap()
            .unwrap()
            .attributes[&attr("x/one")],
        PulledValue::One(Value::I64(7))
    );
    assert!(db_b.pull(&id, &[]).unwrap().unwrap().attributes.is_empty());
    for db in [&zero, &db_a] {
        assert_eq!(
            db.facts(FactFilter {
                attribute: Some(attr("x/later")),
                ..FactFilter::default()
            })
            .unwrap_err()
            .code(),
            ErrorCode::UnknownAttribute
        );
        assert_eq!(
            db.history(
                FactFilter {
                    attribute: Some(attr("x/later")),
                    ..FactFilter::default()
                },
                Revision::ZERO
            )
            .unwrap_err()
            .code(),
            ErrorCode::UnknownAttribute
        );
        assert_eq!(
            db.pull(&id, &[attr("x/later")]).unwrap_err().code(),
            ErrorCode::UnknownAttribute
        );
    }
    let c = store
        .commit(
            Tx::builder()
                .retract(id.clone(), attr("x/one"), Value::I64(7))
                .build()
                .unwrap(),
        )
        .unwrap();
    assert!(
        store
            .db()
            .unwrap()
            .pull(&id, &[attr("x/one")])
            .unwrap()
            .unwrap()
            .attributes
            .is_empty()
    );
    assert_eq!(db_b.facts(FactFilter::default()).unwrap().len(), 1);
    assert!(db_a.facts(FactFilter::default()).unwrap().is_empty());
    let foreign_store = Store::open(dir.path().join("foreign.sqlite")).unwrap();
    let foreign = foreign_store
        .commit(Tx::builder().new_entity(label).build().unwrap())
        .unwrap()
        .created_entities[0]
        .clone();
    let invented = format!(
        "e1.{}.ffffffffffffffff",
        a.after.store().to_string().strip_prefix("s1.").unwrap()
    )
    .parse()
    .unwrap();
    for unknown in [&foreign, &invented] {
        assert_eq!(db_b.pull(unknown, &[attr("x/one")]).unwrap(), None);
        assert!(
            db_b.facts(FactFilter {
                entity: Some(unknown.clone()),
                ..FactFilter::default()
            })
            .unwrap()
            .is_empty()
        );
    }
    assert_eq!(b.after.revision(), Revision::new(2));
    assert_eq!(c.after.revision(), Revision::new(3));
}

#[test]
fn typed_filters_and_many_projections_preserve_exact_values_and_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("types.sqlite")).unwrap();
    let target = store
        .commit(
            Tx::builder()
                .new_entity(TempId::new("target").unwrap())
                .build()
                .unwrap(),
        )
        .unwrap()
        .created_entities[0]
        .clone();
    let other = store
        .commit(
            Tx::builder()
                .new_entity(TempId::new("other").unwrap())
                .build()
                .unwrap(),
        )
        .unwrap()
        .created_entities[0]
        .clone();
    let cases = [
        (ValueType::Bool, vec![Value::Bool(false), Value::Bool(true)]),
        (
            ValueType::I64,
            vec![Value::I64(i64::MIN), Value::I64(0), Value::I64(i64::MAX)],
        ),
        (
            ValueType::F64,
            vec![
                Value::f64(-f64::MAX).unwrap(),
                Value::f64(-0.0).unwrap(),
                Value::f64(f64::MAX).unwrap(),
            ],
        ),
        (
            ValueType::String,
            vec![
                Value::String("\0".into()),
                Value::String("a".into()),
                Value::String("é".into()),
            ],
        ),
        (
            ValueType::Bytes,
            vec![
                Value::Bytes(vec![]),
                Value::Bytes(vec![0]),
                Value::Bytes(vec![255]),
            ],
        ),
        (
            ValueType::Ref,
            vec![Value::Ref(target.clone()), Value::Ref(other)],
        ),
    ];
    for (index, (ty, values)) in cases.into_iter().enumerate() {
        let name = attr(&format!("x/type{index}"));
        let mut tx = Tx::builder().define_attribute(def(name.as_str(), ty, Cardinality::Many));
        for value in values.iter().rev() {
            tx = tx
                .assert(target.clone(), name.clone(), value.clone())
                .assert(target.clone(), name.clone(), value.clone());
        }
        store.commit(tx.build().unwrap()).unwrap();
        let db = store.db().unwrap();
        assert_eq!(
            db.pull(&target, std::slice::from_ref(&name))
                .unwrap()
                .unwrap()
                .attributes[&name],
            PulledValue::Many(values.clone())
        );
        for value in &values {
            assert_eq!(
                db.facts(FactFilter {
                    entity: Some(target.clone()),
                    attribute: Some(name.clone()),
                    value: Some(value.clone())
                })
                .unwrap()
                .len(),
                1
            );
            assert_eq!(
                db.history(
                    FactFilter {
                        entity: Some(target.clone()),
                        attribute: Some(name.clone()),
                        value: Some(value.clone())
                    },
                    Revision::ZERO
                )
                .unwrap()
                .len(),
                1
            );
        }
        let wrong = if ty == ValueType::Bool {
            Value::I64(0)
        } else {
            Value::Bool(false)
        };
        for history in [false, true] {
            let filter = FactFilter {
                attribute: Some(name.clone()),
                value: Some(wrong.clone()),
                ..FactFilter::default()
            };
            let error = if history {
                db.history(filter, Revision::ZERO).unwrap_err()
            } else {
                db.facts(filter).unwrap_err()
            };
            assert_eq!(error.code(), ErrorCode::TypeMismatch);
        }
    }
    let db = store.db().unwrap();
    // Value-only restrictions keep i64 zero distinct from f64 zero.
    for value in [Value::I64(0), Value::f64(0.0).unwrap()] {
        let facts = db
            .facts(FactFilter {
                value: Some(value.clone()),
                ..FactFilter::default()
            })
            .unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].value, value);
    }
    let projection = db.pull(&target, &[attr("x/type5")]).unwrap().unwrap();
    assert!(matches!(
        projection.attributes[&attr("x/type5")],
        PulledValue::Many(_)
    ));
}

#[test]
fn bounded_events_keep_ordinals_and_transactions_include_noops_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let store = Store::open(&path).unwrap();
    let zero = store.db().unwrap();
    let label = TempId::new("entity").unwrap();
    let a = store
        .commit(
            Tx::builder()
                .define_attribute(def("x/value", ValueType::I64, Cardinality::One))
                .new_entity(label.clone())
                .assert(label.clone(), attr("x/value"), Value::I64(1))
                .metadata("source", Value::String("a".into()))
                .build()
                .unwrap(),
        )
        .unwrap();
    let id = a.tempids[&label].clone();
    let db_a = store.db().unwrap();
    let b = store
        .commit(
            Tx::builder()
                .assert(id.clone(), attr("x/value"), Value::I64(2))
                .build()
                .unwrap(),
        )
        .unwrap();
    let c = store
        .commit(
            Tx::builder()
                .assert(id.clone(), attr("x/value"), Value::I64(2))
                .retract(id.clone(), attr("x/value"), Value::I64(3))
                .metadata("noop", Value::Bool(true))
                .build()
                .unwrap(),
        )
        .unwrap();
    let db_c = store.db().unwrap();
    assert!(
        zero.history(FactFilter::default(), Revision::ZERO)
            .unwrap()
            .is_empty()
    );
    assert!(zero.transactions(Revision::ZERO).unwrap().is_empty());
    assert_eq!(
        db_a.history(FactFilter::default(), Revision::ZERO)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(db_a.transactions(Revision::ZERO).unwrap()[0].report, a);
    let history = db_c.history(FactFilter::default(), Revision::ZERO).unwrap();
    assert_eq!(
        history
            .iter()
            .map(|event| (event.revision.get(), event.ordinal, event.operation))
            .collect::<Vec<_>>(),
        vec![
            (1, 0, Operation::Assert),
            (2, 0, Operation::Retract),
            (2, 1, Operation::Assert)
        ]
    );
    let selected = db_c
        .history(
            FactFilter {
                value: Some(Value::I64(2)),
                ..FactFilter::default()
            },
            Revision::new(1),
        )
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].ordinal, 1);
    assert!(
        db_c.history(FactFilter::default(), Revision::new(2))
            .unwrap()
            .is_empty()
    );
    let records = db_c.transactions(Revision::ZERO).unwrap();
    assert_eq!(
        records
            .iter()
            .map(|record| record.report.clone())
            .collect::<Vec<_>>(),
        vec![a.clone(), b, c.clone()]
    );
    assert_eq!(records[2].revision(), Revision::new(3));
    assert!(records[2].report.effective_delta.is_empty());
    assert_eq!(records[2].report.metadata["noop"], Value::Bool(true));
    assert_eq!(db_c.transactions(Revision::new(2)).unwrap().len(), 1);
    assert!(db_c.transactions(Revision::new(3)).unwrap().is_empty());
    for db in [&zero, &db_a, &db_c] {
        let future = db.snapshot().revision().next().unwrap();
        assert_eq!(
            db.history(FactFilter::default(), future)
                .unwrap_err()
                .code(),
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            db.transactions(future).unwrap_err().code(),
            ErrorCode::InvalidRequest
        );
    }
    let expected_facts = db_c.facts(FactFilter::default()).unwrap();
    drop((zero, db_a, db_c, store));
    let reopened = Store::open(&path).unwrap();
    let recovered = reopened.db_at(&c.after).unwrap();
    assert_eq!(
        recovered.facts(FactFilter::default()).unwrap(),
        expected_facts
    );
    assert_eq!(
        recovered
            .history(FactFilter::default(), Revision::ZERO)
            .unwrap(),
        history
    );
    assert_eq!(recovered.transactions(Revision::ZERO).unwrap(), records);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn random_pinned_reads_match_independent_set_and_event_oracles(
        operations in proptest::collection::vec((any::<bool>(), -3_i64..4), 0..8),
        choose in any::<usize>(), lower in any::<usize>(),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("property.sqlite");
        let store = Store::open(&path).unwrap();
        let label = TempId::new("e").unwrap();
        let initial = store.commit(Tx::builder().define_attribute(def("x/set", ValueType::I64, Cardinality::Many))
            .new_entity(label).build().unwrap()).unwrap();
        let id = initial.created_entities[0].clone();
        let mut states = vec![BTreeSet::new()];
        let mut reports = vec![initial];
        let mut expected_events = Vec::new();
        for (assertion, value) in operations {
            let mut state = states.last().unwrap().clone();
            let changed = if assertion { state.insert(value) } else { state.remove(&value) };
            let tx = Tx::builder().metadata("step", Value::I64(reports.len() as i64));
            let tx = if assertion { tx.assert(id.clone(), attr("x/set"), Value::I64(value)) } else { tx.retract(id.clone(), attr("x/set"), Value::I64(value)) };
            let report = store.commit(tx.build().unwrap()).unwrap();
            if changed { expected_events.push((report.after.revision(), if assertion { Operation::Assert } else { Operation::Retract }, value)); }
            reports.push(report); states.push(state);
        }
        let index = choose % states.len();
        let snapshot = reports[index].after.clone();
        let after = Revision::new((lower % (index + 2)) as u64);
        let db = store.db_at(&snapshot).unwrap();
        let facts = db.facts(FactFilter::default()).unwrap();
        let expected_values: Vec<_> = states[index].iter().copied().map(Value::I64).collect();
        prop_assert_eq!(facts.iter().map(|fact| fact.value.clone()).collect::<Vec<_>>(), expected_values.clone());
        let pulled = db.pull(&id, &[attr("x/set")]).unwrap().unwrap();
        if expected_values.is_empty() { prop_assert!(pulled.attributes.is_empty()); }
        else { prop_assert_eq!(&pulled.attributes[&attr("x/set")], &PulledValue::Many(expected_values)); }
        let events = db.history(FactFilter::default(), after).unwrap();
        let expected: Vec<_> = expected_events.iter().filter(|(revision,_,_)| *revision > after && *revision <= snapshot.revision()).cloned().collect();
        prop_assert_eq!(events.iter().map(|event| (event.revision,event.operation,match event.fact.value { Value::I64(value) => value, _ => unreachable!() })).collect::<Vec<_>>(), expected);
        prop_assert!(events.iter().all(|event| event.ordinal == 0));
        let records = db.transactions(after).unwrap();
        prop_assert_eq!(records.iter().map(|record| record.report.clone()).collect::<Vec<_>>(), reports.iter().take(index + 1).filter(|report| report.after.revision() > after).cloned().collect::<Vec<_>>());
        drop((db, store));
        let reopened = Store::open(&path).unwrap();
        let recovered = reopened.db_at(&snapshot).unwrap();
        prop_assert_eq!(recovered.facts(FactFilter::default()).unwrap(), facts);
        prop_assert_eq!(recovered.history(FactFilter::default(), after).unwrap(), events);
        prop_assert_eq!(recovered.transactions(after).unwrap(), records);
    }
}
