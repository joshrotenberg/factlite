//! Differential evidence for evaluated-state reuse, including invalid logs.
use crate::{
    AttributeDefinition, Cardinality, EntityId, ErrorCode, FactDelta, Operation, Revision,
    SnapshotId, Store, TempId, Tx, Uniqueness, Value, ValueTerm, ValueType, codec, log,
    store::revision_bytes,
};
use proptest::prelude::*;

fn attribute(index: usize) -> crate::AttrName {
    format!("unique/v{index}").parse().unwrap()
}
fn label(name: &str) -> TempId {
    TempId::new(name).unwrap()
}
fn value(index: usize, number: i64, reference: &EntityId) -> Value {
    if index == 5 {
        Value::Ref(reference.clone())
    } else {
        scalar(index, number)
    }
}
fn scalar(index: usize, number: i64) -> Value {
    match index {
        0 => Value::Bool(number % 2 == 0),
        1 => Value::I64(number),
        2 => Value::f64(number as f64 / 2.0).unwrap(),
        3 => Value::String(format!("雪\0{number}")),
        4 => Value::Bytes(number.to_be_bytes().to_vec()),
        _ => panic!("invalid scalar index"),
    }
}

fn create(store: &Store, keyed: bool) -> crate::TxReport {
    let mut tx = Tx::builder()
        .new_entity(label("old"))
        .new_entity(label("new"))
        .new_entity(label("target"));
    if keyed {
        tx = tx.idempotency_key("seed");
    }
    for (index, ty) in [
        ValueType::Bool,
        ValueType::I64,
        ValueType::F64,
        ValueType::String,
        ValueType::Bytes,
        ValueType::Ref,
    ]
    .into_iter()
    .enumerate()
    {
        tx = tx.define_attribute(
            AttributeDefinition::new(
                attribute(index),
                ty,
                Cardinality::One,
                Uniqueness::UniqueOne,
            )
            .unwrap(),
        );
        tx = if index == 5 {
            tx.assert(
                label("old"),
                attribute(index),
                ValueTerm::TempRef(label("target")),
            )
        } else {
            tx.assert(label("old"), attribute(index), scalar(index, 0))
        };
    }
    store.commit(tx.build().unwrap()).unwrap()
}

fn same_state(connection: &rusqlite::Connection, basis: &SnapshotId) {
    let actual = log::replay(connection, basis).unwrap();
    let original = log::replay_reference(connection, basis).unwrap();
    assert_eq!(actual.catalog.snapshot, original.catalog.snapshot);
    assert_eq!(actual.catalog.schema, original.catalog.schema);
    assert_eq!(actual.catalog.entities, original.catalog.entities);
    assert_eq!(actual.facts, original.facts);
    // Also compare future allocations/normalization, not just visible state.
    let next = Tx::builder().new_entity(label("next")).build().unwrap();
    assert_eq!(
        actual.evaluate(&next).unwrap().1,
        original.evaluate(&next).unwrap().1
    );
}

fn trace(operations: &[(usize, i64, bool)], seed_keyed: bool) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("parity.sqlite");
    let store = Store::open(&path).unwrap();
    let seed = create(&store, seed_keyed);
    let nodes = [
        seed.tempids[&label("old")].clone(),
        seed.tempids[&label("new")].clone(),
    ];
    let target = seed.tempids[&label("target")].clone();
    let mut owners = [0; 6];
    let mut values: Vec<_> = (0..6).map(|index| value(index, 0, &target)).collect();
    let mut reports = vec![seed];
    for (step, (index, number, keyed)) in operations.iter().enumerate() {
        let old = owners[*index];
        let new = 1 - old;
        let next = value(*index, *number, &target);
        let mut tx = Tx::builder()
            .expect_head(store.head().unwrap())
            .expect(
                nodes[old].clone(),
                attribute(*index),
                vec![values[*index].clone(), values[*index].clone()],
            )
            .assert(nodes[new].clone(), attribute(*index), next.clone())
            .retract(
                nodes[old].clone(),
                attribute(*index),
                values[*index].clone(),
            )
            .metadata("step", Value::I64(step as i64));
        if *keyed {
            tx = tx.idempotency_key(format!("step-{step}"));
        }
        // New schema and empty allocations occur among typed transfers.
        if step % 2 == 0 {
            tx = tx.new_entity(label("empty")).define_attribute(
                AttributeDefinition::new(
                    format!("later/step{step}").parse().unwrap(),
                    ValueType::Bool,
                    Cardinality::Many,
                    Uniqueness::None,
                )
                .unwrap(),
            );
        }
        reports.push(store.commit(tx.build().unwrap()).unwrap());
        owners[*index] = new;
        values[*index] = next;
        let connection = store.connection().unwrap();
        for report in &reports {
            same_state(&connection, &report.after);
        }
    }
    drop(store);
    let reopened = Store::open(path).unwrap();
    let connection = reopened.connection().unwrap();
    for report in &reports {
        same_state(&connection, &report.after);
    }
}

#[test]
fn both_replayers_preserve_every_type_across_keyed_and_unkeyed_transitions() {
    let operations: Vec<_> = (0..6)
        .flat_map(|ty| [(ty, -1, true), (ty, 0, false), (ty, i64::MAX, true)])
        .collect();
    trace(&operations, true);
    trace(&operations, false);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn optimized_replay_equals_original_for_typed_mixed_prefixes(
        operations in proptest::collection::vec((0_usize..6,-4_i64..5,any::<bool>()),0..8),
        seed_keyed in any::<bool>(),
    ) {trace(&operations,seed_keyed);}
}

#[test]
fn both_replayers_reject_semantic_tampering_even_when_event_rows_match_report() {
    // Modify the retained report and its corresponding event rows together.
    // The original canonical request remains valid, so simple row parity alone
    // cannot detect these mutations; full request/report equality must do so.
    for mutation in 0..5 {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path().join("bad.sqlite")).unwrap();
        let original = create(&store, true);
        let mut report = original.clone();
        match mutation {
            0 => {
                report.metadata.insert("tampered".into(), Value::Bool(true));
            }
            1 => {
                report.effective_delta.pop();
            }
            2 => {
                report.effective_delta.push(FactDelta {
                    operation: Operation::Assert,
                    fact: report.effective_delta[0].fact.clone(),
                });
            }
            3 => {
                report
                    .tempids
                    .insert(label("old"), report.tempids[&label("new")].clone());
            }
            _ => {
                report.before = SnapshotId::new(
                    report.before.store().clone(),
                    report.before.lineage().clone(),
                    Revision::new(1),
                );
            }
        }
        let connection = store.connection().unwrap();
        let revision = revision_bytes(report.after.revision());
        connection
            .execute(
                "UPDATE transactions SET report=?1 WHERE revision=?2",
                rusqlite::params![codec::encode_report(&report), revision.as_slice()],
            )
            .unwrap();
        if mutation == 1 {
            connection
                .execute(
                    "DELETE FROM fact_events WHERE revision=?1 AND ordinal=?2",
                    rusqlite::params![revision.as_slice(), report.effective_delta.len() as i64],
                )
                .unwrap();
        }
        if mutation == 2 {
            let event = report.effective_delta.last().unwrap();
            connection.execute("INSERT INTO fact_events(revision,ordinal,operation,entity,attribute,type_tag,value) VALUES(?1,?2,1,?3,?4,?5,?6)",rusqlite::params![revision.as_slice(),report.effective_delta.len() as i64-1,event.fact.entity.to_string(),event.fact.attribute.as_str(),codec::type_tag(event.fact.value.value_type()),codec::encode_value(&event.fact.value)]).unwrap();
        }
        for replay in [log::replay, log::replay_reference] {
            assert_eq!(
                replay(&connection, &original.after).unwrap_err().code(),
                ErrorCode::Storage,
                "mutation {mutation}"
            );
        }
    }
}
