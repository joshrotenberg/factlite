//! Reference scenarios before durable-store integration.

use crate::model::{
    Model,
    tests::{attr, def, empty, setup},
};
use crate::{
    Cardinality, EntityId, ErrorCode, ErrorContext, Operation, Revision, TempId, Tx, Uniqueness,
    Value, ValueType,
};
use proptest::prelude::*;
use std::collections::BTreeSet;

fn create_graph() -> (Model, crate::TxReport) {
    let person = TempId::new("person").unwrap();
    let task = TempId::new("task").unwrap();
    let request = Tx::builder()
        .define_attribute(def(
            "person/name",
            ValueType::String,
            Cardinality::One,
            Uniqueness::None,
        ))
        .define_attribute(def(
            "task/title",
            ValueType::String,
            Cardinality::One,
            Uniqueness::None,
        ))
        .define_attribute(def(
            "task/status",
            ValueType::String,
            Cardinality::One,
            Uniqueness::None,
        ))
        .define_attribute(def(
            "task/owner",
            ValueType::Ref,
            Cardinality::One,
            Uniqueness::None,
        ))
        .new_entity(person.clone())
        .new_entity(task.clone())
        .assert(
            person.clone(),
            attr("person/name"),
            Value::String("Ada".into()),
        )
        .assert(
            task.clone(),
            attr("task/title"),
            Value::String("Design Factlite".into()),
        )
        .assert(
            task.clone(),
            attr("task/status"),
            Value::String("todo".into()),
        )
        .assert(task, attr("task/owner"), crate::ValueTerm::TempRef(person))
        .metadata("source", Value::String("design-example".into()))
        .build()
        .unwrap();
    empty().evaluate(&request).unwrap()
}

#[test]
fn a_b_conditions_head_fences_and_nochange_reports() {
    let (a, report_a) = create_graph();
    let task = report_a.tempids[&TempId::new("task").unwrap()].clone();
    assert_eq!(report_a.before.revision(), Revision::ZERO);
    assert_eq!(report_a.after.revision(), Revision::new(1));
    assert_eq!(report_a.created_entities.len(), 2);
    assert_eq!(report_a.schema_added.len(), 4);
    assert_eq!(report_a.effective_delta.len(), 4);
    let update = Tx::builder()
        .expect(
            task.clone(),
            attr("task/status"),
            vec![Value::String("todo".into())],
        )
        .assert(
            task.clone(),
            attr("task/status"),
            Value::String("doing".into()),
        )
        .metadata("source", Value::String("design-example".into()))
        .build()
        .unwrap();
    let (b, report_b) = a.evaluate(&update).unwrap();
    assert_eq!(report_b.before, report_a.after);
    assert_eq!(report_b.after.revision(), Revision::new(2));
    assert_eq!(report_b.effective_delta.len(), 2);
    assert_eq!(report_b.effective_delta[0].operation, Operation::Retract);
    assert_eq!(
        report_b.effective_delta[0].fact.value,
        Value::String("todo".into())
    );
    assert_eq!(report_b.effective_delta[1].operation, Operation::Assert);
    assert_eq!(
        report_b.effective_delta[1].fact.value,
        Value::String("doing".into())
    );
    let err = b.evaluate(&update).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PreconditionFailed);
    match err.context() {
        ErrorContext::Precondition {
            entity,
            attribute,
            expected,
            actual,
        } => {
            assert_eq!(entity, &task);
            assert_eq!(attribute, &attr("task/status"));
            assert_eq!(expected, &[Value::String("todo".into())]);
            assert_eq!(actual, &[Value::String("doing".into())]);
        }
        _ => panic!("wrong error context"),
    }
    let (unrelated, _) = a
        .evaluate(
            &Tx::builder()
                .metadata("note", Value::Bool(true))
                .build()
                .unwrap(),
        )
        .unwrap();
    assert!(unrelated.evaluate(&update).is_ok());
    let fenced = Tx::builder()
        .expect_head(report_a.after.clone())
        .build()
        .unwrap();
    assert_eq!(
        unrelated.evaluate(&fenced).unwrap_err().code(),
        ErrorCode::HeadMismatch
    );
    let (c, report_c) = b
        .evaluate(
            &Tx::builder()
                .assert(task, attr("task/status"), Value::String("doing".into()))
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(c.catalog.snapshot.revision(), Revision::new(3));
    assert!(report_c.effective_delta.is_empty());
    assert!(
        a.facts()
            .iter()
            .any(|f| f.attribute == attr("task/status") && f.value == Value::String("todo".into()))
    );
}

#[test]
fn expectations_use_prehead_and_validate_sets_types_and_identity() {
    let (model, id) = setup(Cardinality::One);
    let absent = Tx::builder()
        .expect(id.clone(), attr("x/value"), vec![])
        .assert(id.clone(), attr("x/value"), Value::I64(1))
        .build()
        .unwrap();
    let (a, _) = model.evaluate(&absent).unwrap();
    let duplicate = Tx::builder()
        .expect(
            id.clone(),
            attr("x/value"),
            vec![Value::I64(1), Value::I64(1)],
        )
        .build()
        .unwrap();
    assert!(a.evaluate(&duplicate).is_ok());
    assert_eq!(
        a.evaluate(
            &Tx::builder()
                .expect(
                    id.clone(),
                    attr("x/value"),
                    vec![Value::I64(1), Value::I64(2)]
                )
                .build()
                .unwrap()
        )
        .unwrap_err()
        .code(),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        a.evaluate(
            &Tx::builder()
                .expect(id.clone(), attr("x/value"), vec![Value::Bool(true)])
                .build()
                .unwrap()
        )
        .unwrap_err()
        .code(),
        ErrorCode::TypeMismatch
    );
    let new_attr = Tx::builder()
        .define_attribute(def(
            "x/new",
            ValueType::I64,
            Cardinality::One,
            Uniqueness::None,
        ))
        .expect(id.clone(), attr("x/new"), vec![])
        .build()
        .unwrap();
    assert_eq!(
        a.evaluate(&new_attr).unwrap_err().code(),
        ErrorCode::UnknownAttribute
    );
    let self_satisfying = Tx::builder()
        .expect(id.clone(), attr("x/value"), vec![Value::I64(2)])
        .assert(id, attr("x/value"), Value::I64(2))
        .build()
        .unwrap();
    assert_eq!(
        a.evaluate(&self_satisfying).unwrap_err().code(),
        ErrorCode::PreconditionFailed
    );
    let unknown: EntityId = "e1.00000000000000000000000000000001.0000000000000002"
        .parse()
        .unwrap();
    assert_eq!(
        a.evaluate(
            &Tx::builder()
                .expect(unknown, attr("x/value"), vec![])
                .build()
                .unwrap()
        )
        .unwrap_err()
        .code(),
        ErrorCode::UnknownEntity
    );
}

fn unique_setup(ty: ValueType) -> (Model, EntityId, EntityId) {
    let a = TempId::new("a").unwrap();
    let b = TempId::new("b").unwrap();
    let (model, report) = empty()
        .evaluate(
            &Tx::builder()
                .define_attribute(def("x/unique", ty, Cardinality::One, Uniqueness::UniqueOne))
                .new_entity(a.clone())
                .new_entity(b.clone())
                .build()
                .unwrap(),
        )
        .unwrap();
    (
        model,
        report.tempids[&a].clone(),
        report.tempids[&b].clone(),
    )
}

#[test]
fn uniqueness_transfers_swaps_and_late_failure_rollback() {
    let (model, a, b) = unique_setup(ValueType::I64);
    let (owned, _) = model
        .evaluate(
            &Tx::builder()
                .assert(a.clone(), attr("x/unique"), Value::I64(1))
                .assert(b.clone(), attr("x/unique"), Value::I64(2))
                .build()
                .unwrap(),
        )
        .unwrap();
    let swap = Tx::builder()
        .assert(a.clone(), attr("x/unique"), Value::I64(2))
        .assert(b.clone(), attr("x/unique"), Value::I64(1))
        .build()
        .unwrap();
    assert!(owned.evaluate(&swap).is_ok());
    let failing = Tx::builder()
        .new_entity(TempId::new("failed").unwrap())
        .define_attribute(def(
            "x/later",
            ValueType::String,
            Cardinality::One,
            Uniqueness::None,
        ))
        .assert(b.clone(), attr("x/unique"), Value::I64(1))
        .metadata("do-not-commit", Value::Bool(true))
        .build()
        .unwrap();
    let error = owned.evaluate(&failing).unwrap_err();
    assert_eq!(error.code(), ErrorCode::UniqueViolation);
    match error.context() {
        ErrorContext::UniqueOwnership {
            attribute,
            value,
            owners,
        } => {
            assert_eq!(attribute, &attr("x/unique"));
            assert_eq!(value, &Value::I64(1));
            assert_eq!(owners, &[a, b]);
        }
        _ => panic!("wrong uniqueness context"),
    }
    assert_eq!(owned.catalog.entities.len(), 2);
    assert!(!owned.catalog.schema.contains_key(&attr("x/later")));
    assert_eq!(owned.catalog.snapshot.revision(), Revision::new(2));
}

#[test]
fn zero_normalization_is_used_for_unique_ownership() {
    let (model, a, b) = unique_setup(ValueType::F64);
    let tx = Tx::builder()
        .assert(a, attr("x/unique"), Value::f64(-0.0).unwrap())
        .assert(b, attr("x/unique"), Value::f64(0.0).unwrap())
        .build()
        .unwrap();
    assert_eq!(
        model.evaluate(&tx).unwrap_err().code(),
        ErrorCode::UniqueViolation
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn unique_transfer_validates_final_state(value in any::<i64>(), reversed in any::<bool>()) {
        let (model,a,b)=unique_setup(ValueType::I64);
        let (owned,_)=model.evaluate(&Tx::builder().assert(a.clone(),attr("x/unique"),Value::I64(value)).build().unwrap()).unwrap();
        let mut tx=Tx::builder().assert(b.clone(),attr("x/unique"),Value::I64(value)).retract(a,attr("x/unique"),Value::I64(value)).build().unwrap();
        if reversed{tx.changes.reverse();}
        let (next,report)=owned.evaluate(&tx).unwrap();
        prop_assert_eq!(next.facts().len(),1);prop_assert_eq!(&next.facts().iter().next().unwrap().entity,&b);prop_assert_eq!(report.effective_delta.len(),2);
    }
    #[test]
    fn many_expectations_are_exact_sets(actual in proptest::collection::vec(-8_i64..8,0..16), expected in proptest::collection::vec(-8_i64..8,0..16), match_actual in any::<bool>()) {
        let expected=if match_actual{actual.iter().chain(&actual).copied().collect()}else{expected};
        let (model,id)=setup(Cardinality::Many);
        let mut builder=Tx::builder();for value in &actual{builder=builder.assert(id.clone(),attr("x/value"),Value::I64(*value));}
        let (model,_)=model.evaluate(&builder.build().unwrap()).unwrap();
        let expected_set:BTreeSet<_>=expected.iter().copied().collect();let actual_set:BTreeSet<_>=actual.into_iter().collect();
        let result=model.evaluate(&Tx::builder().expect(id,attr("x/value"),expected.into_iter().map(Value::I64).collect()).build().unwrap());
        prop_assert_eq!(result.is_ok(),expected_set==actual_set);
        if let Err(err)=result{prop_assert_eq!(err.code(),ErrorCode::PreconditionFailed);}
    }
    #[test]
    fn unique_ownership_matches_distinct_assignment(values in proptest::collection::vec(-16_i64..16,0..12)) {
        let mut builder=Tx::builder().define_attribute(def("x/unique",ValueType::I64,Cardinality::One,Uniqueness::UniqueOne));
        for (i,value) in values.iter().enumerate(){let label=TempId::new(format!("e{i}")).unwrap();builder=builder.new_entity(label.clone()).assert(label,attr("x/unique"),Value::I64(*value));}
        let distinct:BTreeSet<_>=values.iter().copied().collect();let result=empty().evaluate(&builder.build().unwrap());
        prop_assert_eq!(result.is_ok(),distinct.len()==values.len());
        if let Err(err)=result{prop_assert_eq!(err.code(),ErrorCode::UniqueViolation);}
    }
}

#[test]
fn all_six_types_share_exact_expectation_transfer_and_conflict_rules() {
    let a = TempId::new("a").unwrap();
    let b = TempId::new("b").unwrap();
    let target = TempId::new("target").unwrap();
    let (base, created) = empty()
        .evaluate(
            &Tx::builder()
                .new_entity(a.clone())
                .new_entity(b.clone())
                .new_entity(target.clone())
                .build()
                .unwrap(),
        )
        .unwrap();
    let a = created.tempids[&a].clone();
    let b = created.tempids[&b].clone();
    let target = created.tempids[&target].clone();
    for value in [
        Value::Bool(false),
        Value::I64(i64::MIN),
        Value::f64(-0.0).unwrap(),
        Value::String("é\0exact".into()),
        Value::Bytes(vec![0, 255]),
        Value::Ref(target),
    ] {
        let tx = Tx::builder()
            .define_attribute(def(
                "x/typed",
                value.value_type(),
                Cardinality::One,
                Uniqueness::UniqueOne,
            ))
            .assert(a.clone(), attr("x/typed"), value.clone())
            .build()
            .unwrap();
        let (owned, _) = base.evaluate(&tx).unwrap();
        assert!(
            owned
                .evaluate(
                    &Tx::builder()
                        .expect(
                            a.clone(),
                            attr("x/typed"),
                            vec![value.clone(), value.clone()]
                        )
                        .build()
                        .unwrap()
                )
                .is_ok()
        );
        assert_eq!(
            owned
                .evaluate(
                    &Tx::builder()
                        .assert(b.clone(), attr("x/typed"), value.clone())
                        .build()
                        .unwrap()
                )
                .unwrap_err()
                .code(),
            ErrorCode::UniqueViolation
        );
        let (transferred, report) = owned
            .evaluate(
                &Tx::builder()
                    .assert(b.clone(), attr("x/typed"), value.clone())
                    .retract(a.clone(), attr("x/typed"), value.clone())
                    .build()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(transferred.facts().len(), 1);
        assert_eq!(transferred.facts().iter().next().unwrap().value, value);
        assert_eq!(report.effective_delta.len(), 2);
    }
}

#[test]
fn signed_zero_is_one_exact_fact_and_one_expected_value() {
    let (model, a, _) = unique_setup(ValueType::F64);
    let conflict = Tx::builder()
        .assert(a.clone(), attr("x/unique"), Value::f64(-0.0).unwrap())
        .retract(a.clone(), attr("x/unique"), Value::f64(0.0).unwrap())
        .build()
        .unwrap();
    assert_eq!(
        model.evaluate(&conflict).unwrap_err().code(),
        ErrorCode::ConflictingBatch
    );
    let (owned, _) = model
        .evaluate(
            &Tx::builder()
                .assert(a.clone(), attr("x/unique"), Value::f64(-0.0).unwrap())
                .build()
                .unwrap(),
        )
        .unwrap();
    assert!(
        owned
            .evaluate(
                &Tx::builder()
                    .expect(
                        a,
                        attr("x/unique"),
                        vec![Value::f64(-0.0).unwrap(), Value::f64(0.0).unwrap()]
                    )
                    .build()
                    .unwrap()
            )
            .is_ok()
    );
}

#[test]
fn exhaustive_three_value_batch_and_expectation_tables() {
    for cardinality in [Cardinality::One, Cardinality::Many] {
        for old in 0_u8..8 {
            if cardinality == Cardinality::One && old.count_ones() > 1 {
                continue;
            }
            let (mut model, id) = setup(cardinality);
            model.set_facts(
                (0..3)
                    .filter(|n| old & (1 << n) != 0)
                    .map(|n| crate::Fact {
                        entity: id.clone(),
                        attribute: attr("x/value"),
                        value: Value::I64(n),
                    })
                    .collect(),
            );
            for adds in 0_u8..8 {
                for removes in 0_u8..8 {
                    let mut builder = Tx::builder();
                    for n in 0..3 {
                        if adds & (1 << n) != 0 {
                            builder = builder.assert(id.clone(), attr("x/value"), Value::I64(n));
                        }
                        if removes & (1 << n) != 0 {
                            builder = builder.retract(id.clone(), attr("x/value"), Value::I64(n));
                        }
                    }
                    let mut tx = builder.build().unwrap();
                    let result = model.evaluate(&tx);
                    tx.changes.reverse();
                    let reversed = model.evaluate(&tx);
                    let conflicts = adds & removes != 0
                        || cardinality == Cardinality::One && adds.count_ones() > 1;
                    if conflicts {
                        assert_eq!(result.unwrap_err().code(), ErrorCode::ConflictingBatch);
                        assert_eq!(reversed.unwrap_err().code(), ErrorCode::ConflictingBatch);
                    } else {
                        let expected = if cardinality == Cardinality::One && adds != 0 {
                            adds
                        } else {
                            (old & !removes) | adds
                        };
                        let (next, report) = result.unwrap();
                        let (next_reversed, reversed_report) = reversed.unwrap();
                        let actual =
                            next.facts()
                                .iter()
                                .fold(0_u8, |mask, fact| match fact.value {
                                    Value::I64(n) => mask | (1 << n),
                                    _ => unreachable!(),
                                });
                        assert_eq!(actual, expected);
                        assert_eq!(next.facts(), next_reversed.facts());
                        assert_eq!(report, reversed_report);
                        assert_eq!(
                            report.effective_delta.len(),
                            (old ^ expected).count_ones() as usize
                        );
                    }
                }
            }
            for expected in 0_u8..8 {
                let values = (0..3)
                    .filter(|n| expected & (1 << n) != 0)
                    .map(Value::I64)
                    .collect();
                let result = model.evaluate(
                    &Tx::builder()
                        .expect(id.clone(), attr("x/value"), values)
                        .build()
                        .unwrap(),
                );
                if cardinality == Cardinality::One && expected.count_ones() > 1 {
                    assert_eq!(result.unwrap_err().code(), ErrorCode::InvalidRequest);
                } else if expected == old {
                    assert!(result.is_ok());
                } else {
                    assert_eq!(result.unwrap_err().code(), ErrorCode::PreconditionFailed);
                }
            }
        }
    }
}
