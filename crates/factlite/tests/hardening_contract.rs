//! Durable acceptance gaps found while mapping the original design matrix.
use factlite::{
    AttributeDefinition, Cardinality, CommitOutcome, ErrorCode, FactFilter, Revision, Store,
    TempId, Tx, Uniqueness, Value,
};

#[test]
fn every_unique_value_type_transfers_atomically_and_failed_ownership_reserves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unique.sqlite");
    let store = Store::open(&path).unwrap();
    let allocated = store
        .commit(
            Tx::builder()
                .new_entity(TempId::new("old").unwrap())
                .new_entity(TempId::new("new").unwrap())
                .new_entity(TempId::new("empty-target").unwrap())
                .build()
                .unwrap(),
        )
        .unwrap();
    let old = allocated.tempids[&TempId::new("old").unwrap()].clone();
    let new = allocated.tempids[&TempId::new("new").unwrap()].clone();
    let values = [
        Value::Bool(true),
        Value::I64(i64::MIN),
        Value::f64(-0.0).unwrap(),
        Value::String("\0e\u{301}雪".into()),
        Value::Bytes(vec![0, 255]),
        Value::Ref(allocated.tempids[&TempId::new("empty-target").unwrap()].clone()),
    ];
    let mut outcomes = Vec::new();
    for (index, value) in values.into_iter().enumerate() {
        let attr: factlite::AttrName = format!("unique/type{index}").parse().unwrap();
        store
            .commit(
                Tx::builder()
                    .define_attribute(
                        AttributeDefinition::new(
                            attr.clone(),
                            value.value_type(),
                            Cardinality::One,
                            Uniqueness::UniqueOne,
                        )
                        .unwrap(),
                    )
                    .assert(old.clone(), attr.clone(), value.clone())
                    .build()
                    .unwrap(),
            )
            .unwrap();
        let before = store.db().unwrap();
        let facts_before = before.facts(FactFilter::default()).unwrap();
        let key = format!("transfer-{index}");
        let error = store
            .commit(
                Tx::builder()
                    .new_entity(TempId::new("must-not-exist").unwrap())
                    .assert(new.clone(), attr.clone(), value.clone())
                    .metadata("failed", Value::Bool(true))
                    .idempotency_key(&key)
                    .build()
                    .unwrap(),
            )
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::UniqueViolation);
        assert_eq!(error.commit_outcome(), Some(CommitOutcome::RolledBack));
        assert_eq!(store.head().unwrap(), *before.snapshot());
        assert_eq!(
            store.db().unwrap().facts(FactFilter::default()).unwrap(),
            facts_before
        );
        // Reverse operation order deliberately: ownership validates final state.
        let transfer = Tx::builder()
            .assert(new.clone(), attr.clone(), value.clone())
            .retract(old.clone(), attr.clone(), value.clone())
            .metadata("success", Value::Bool(true))
            .idempotency_key(key)
            .build()
            .unwrap();
        let report = store.commit(transfer.clone()).unwrap();
        assert_eq!(report.effective_delta.len(), 2);
        assert!(report.created_entities.is_empty());
        assert_eq!(store.commit(transfer.clone()).unwrap(), report);
        assert_eq!(before.facts(FactFilter::default()).unwrap(), facts_before);
        let after = store.db().unwrap();
        assert_eq!(
            after
                .facts(FactFilter {
                    attribute: Some(attr.clone()),
                    ..FactFilter::default()
                })
                .unwrap()[0]
                .entity,
            new
        );
        assert_eq!(
            after.pull(&new, &[attr]).unwrap().unwrap().attributes.len(),
            1
        );
        outcomes.push((transfer, report));
    }
    let head = store.head().unwrap();
    assert_eq!(head.revision(), Revision::new(13));
    drop(store);
    let recovered = Store::open(&path).unwrap();
    for (request, report) in outcomes {
        assert_eq!(recovered.commit(request).unwrap(), report);
    }
    assert_eq!(recovered.head().unwrap(), head);
    let records = recovered
        .db()
        .unwrap()
        .transactions(Revision::ZERO)
        .unwrap();
    assert_eq!(records.len(), 13);
    assert_eq!(
        records
            .iter()
            .map(|record| record.report.created_entities.len())
            .sum::<usize>(),
        3
    );
    assert!(
        records
            .iter()
            .all(|record| !record.report.metadata.contains_key("failed"))
    );
}
