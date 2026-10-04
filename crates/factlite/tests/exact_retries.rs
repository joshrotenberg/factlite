//! Retry precedence, original outcomes, atomic key reservation, and contention.

use std::sync::{Arc, Barrier};

use factlite::{
    AttributeDefinition, Cardinality, CommitOutcome, ErrorCode, FactFilter, Revision, SnapshotId,
    Store, TempId, Tx, Uniqueness, Value, ValueTerm, ValueType, wire,
};
use proptest::prelude::*;

fn create(key: &str, head: &SnapshotId) -> Tx {
    let label = TempId::new("entity").unwrap();
    Tx::builder()
        .expect_head(head.clone())
        .idempotency_key(key)
        .define_attribute(
            AttributeDefinition::new(
                "task/status".parse().unwrap(),
                ValueType::String,
                Cardinality::One,
                Uniqueness::None,
            )
            .unwrap(),
        )
        .define_attribute(
            AttributeDefinition::new(
                "task/self".parse().unwrap(),
                ValueType::Ref,
                Cardinality::One,
                Uniqueness::None,
            )
            .unwrap(),
        )
        .new_entity(label.clone())
        .assert(
            label.clone(),
            "task/status".parse().unwrap(),
            Value::String("todo".into()),
        )
        .assert(
            label.clone(),
            "task/self".parse().unwrap(),
            ValueTerm::TempRef(label),
        )
        .metadata("source", Value::String("retry-test".into()))
        .build()
        .unwrap()
}

#[test]
fn original_create_and_conditional_reports_replay_before_stale_head_and_expectations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retry.sqlite");
    let store = Store::open(&path).unwrap();
    let request_a = create("create", &store.head().unwrap());
    let a = store.commit(request_a.clone()).unwrap();
    let id = a.created_entities[0].clone();
    let request_b = Tx::builder()
        .idempotency_key("start")
        .expect_head(a.after.clone())
        .expect(
            id.clone(),
            "task/status".parse().unwrap(),
            vec![Value::String("todo".into())],
        )
        .assert(
            id.clone(),
            "task/status".parse().unwrap(),
            Value::String("doing".into()),
        )
        .metadata("source", Value::String("retry-test".into()))
        .build()
        .unwrap();
    let b = store.commit(request_b.clone()).unwrap();
    let c = store
        .commit(
            Tx::builder()
                .assert(
                    id,
                    "task/status".parse().unwrap(),
                    Value::String("done".into()),
                )
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(store.commit(request_a.clone()).unwrap(), a);
    assert_eq!(store.commit(request_b.clone()).unwrap(), b);
    assert_eq!(store.head().unwrap(), c.after);
    let records = store.db().unwrap().transactions(Revision::ZERO).unwrap();
    assert_eq!(records.len(), 3);
    let facts = store.db().unwrap().facts(FactFilter::default()).unwrap();
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened
            .commit(wire::decode_tx(&wire::encode_tx(&request_a)).unwrap())
            .unwrap(),
        a
    );
    assert_eq!(reopened.commit(request_b).unwrap(), b);
    assert_eq!(reopened.head().unwrap(), c.after);
    assert_eq!(
        reopened.db().unwrap().facts(FactFilter::default()).unwrap(),
        facts
    );
    assert_eq!(
        reopened.db().unwrap().transactions(Revision::ZERO).unwrap(),
        records
    );
}

#[test]
fn conflicting_reuse_precedes_state_errors_and_semantically_equal_reordering_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("conflicts.sqlite")).unwrap();
    let request = create("key", &store.head().unwrap());
    let original = store.commit(request.clone()).unwrap();
    let encoded: serde_json::Value = serde_json::from_str(&wire::encode_tx(&request)).unwrap();
    for field in [
        "metadata",
        "changes",
        "new_entities",
        "expected_head",
        "schema",
    ] {
        let mut changed = encoded.clone();
        match field {
            "metadata" => changed["metadata"]["extra"] = serde_json::json!({"bool":true}),
            "changes" => changed["changes"].as_array_mut().unwrap().reverse(),
            "new_entities" => {
                changed["new_entities"][0] = serde_json::json!("renamed");
                for operation in changed["changes"].as_array_mut().unwrap() {
                    operation["entity"]["temp"] = serde_json::json!("renamed");
                    if operation["value"].get("ref").is_some() {
                        operation["value"]["ref"]["temp"] = serde_json::json!("renamed");
                    }
                }
            }
            "expected_head" => changed["expected_head"]["revision"] = serde_json::json!("99"),
            "schema" => changed["schema"][0]["type"] = serde_json::json!("i64"),
            _ => unreachable!(),
        }
        let tx = wire::decode_tx(&changed.to_string()).unwrap();
        let error = store.commit(tx).unwrap_err();
        assert_eq!(error.code(), ErrorCode::IdempotencyConflict, "{field}");
        assert_eq!(error.commit_outcome(), Some(CommitOutcome::RolledBack));
        assert_eq!(store.head().unwrap(), original.after);
    }
}

#[test]
fn failed_requests_reserve_no_key_and_new_nochange_requests_advance_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("failure.sqlite");
    let store = Store::open(&path).unwrap();
    let zero = store.head().unwrap();
    let impossible = SnapshotId::new(
        zero.store().clone(),
        zero.lineage().clone(),
        Revision::new(1),
    );
    let failed = store
        .commit(
            Tx::builder()
                .idempotency_key("reuse")
                .expect_head(impossible)
                .build()
                .unwrap(),
        )
        .unwrap_err();
    assert_eq!(failed.code(), ErrorCode::HeadMismatch);
    assert_eq!(store.head().unwrap(), zero);
    let successful = Tx::builder()
        .idempotency_key("reuse")
        .metadata("value", Value::Bool(true))
        .build()
        .unwrap();
    let a = store.commit(successful.clone()).unwrap();
    assert!(a.effective_delta.is_empty());
    let b = store.commit(Tx::default()).unwrap();
    assert_eq!(b.after.revision(), Revision::new(2));
    assert_eq!(store.commit(successful).unwrap(), a);
    assert_eq!(store.head().unwrap(), b.after);
    assert_eq!(Store::open(&path).unwrap().head().unwrap(), b.after);
}

#[test]
fn same_key_contenders_allocate_once_and_different_requests_have_one_winner() {
    for same_request in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("race.sqlite");
        let store = Store::open(&path).unwrap();
        let connections: Vec<_> = (0..8).map(|_| Store::open(&path).unwrap()).collect();
        let barrier = Arc::new(Barrier::new(connections.len()));
        let request = create("contended", &store.head().unwrap());
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = connections
                .into_iter()
                .enumerate()
                .map(|(index, connection)| {
                    let barrier = barrier.clone();
                    let request = request.clone();
                    scope.spawn(move || {
                        let request = if same_request {
                            request
                        } else {
                            let mut json: serde_json::Value =
                                serde_json::from_str(&wire::encode_tx(&request)).unwrap();
                            json["metadata"]["writer"] =
                                serde_json::json!({"i64":index.to_string()});
                            wire::decode_tx(&json.to_string()).unwrap()
                        };
                        barrier.wait();
                        connection.commit(request)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        let successes: Vec<_> = results
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .collect();
        assert_eq!(successes.len(), if same_request { 8 } else { 1 });
        for report in &successes {
            assert_eq!(report, &successes[0]);
        }
        for error in results.iter().filter_map(|result| result.as_ref().err()) {
            assert_eq!(error.code(), ErrorCode::IdempotencyConflict);
        }
        assert_eq!(store.head().unwrap().revision(), Revision::new(1));
        assert_eq!(
            store
                .db()
                .unwrap()
                .transactions(Revision::ZERO)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .db()
                .unwrap()
                .facts(FactFilter::default())
                .unwrap()
                .len(),
            2
        );
    }
}

#[test]
fn keyed_all_typed_values_forward_cycles_and_empty_entities_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("types.sqlite");
    let store = Store::open(&path).unwrap();
    let a = TempId::new("z").unwrap();
    let b = TempId::new("a").unwrap();
    let mut tx = Tx::builder()
        .idempotency_key("types")
        .new_entity(a.clone())
        .new_entity(b.clone())
        .new_entity(TempId::new("empty").unwrap());
    for (index, value) in [
        Value::Bool(false),
        Value::I64(i64::MIN),
        Value::f64(-0.0).unwrap(),
        Value::String("é\0".into()),
        Value::Bytes(vec![0, 255]),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("x/v{index}");
        tx = tx
            .define_attribute(
                AttributeDefinition::new(
                    name.parse().unwrap(),
                    value.value_type(),
                    Cardinality::Many,
                    Uniqueness::None,
                )
                .unwrap(),
            )
            .assert(a.clone(), name.parse().unwrap(), value.clone())
            .metadata(name, value);
    }
    tx = tx
        .define_attribute(
            AttributeDefinition::new(
                "x/ref".parse().unwrap(),
                ValueType::Ref,
                Cardinality::Many,
                Uniqueness::None,
            )
            .unwrap(),
        )
        .assert(
            a.clone(),
            "x/ref".parse().unwrap(),
            ValueTerm::TempRef(b.clone()),
        )
        .assert(b, "x/ref".parse().unwrap(), ValueTerm::TempRef(a));
    let tx = tx.build().unwrap();
    let report = store.commit(tx.clone()).unwrap();
    let reference = Value::Ref(report.created_entities[1].clone());
    let concrete = Tx::builder()
        .idempotency_key("concrete-ref")
        .expect(
            report.created_entities[0].clone(),
            "x/ref".parse().unwrap(),
            vec![reference.clone()],
        )
        .assert(
            report.created_entities[0].clone(),
            "x/ref".parse().unwrap(),
            reference,
        )
        .build()
        .unwrap();
    let latest = store.commit(concrete.clone()).unwrap();
    assert!(latest.effective_delta.is_empty());
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.commit(tx).unwrap(), report);
    assert_eq!(reopened.commit(concrete).unwrap(), latest);
    assert_eq!(report.created_entities.len(), 3);
    assert_eq!(reopened.head().unwrap(), latest.after);
}

#[test]
fn corrupt_or_unknown_retained_canonical_evidence_fails_reopen() {
    for mutation in [
        "UPDATE transactions SET canonical_request=X'00'",
        "UPDATE transactions SET canonical_version=2",
        "UPDATE transactions SET canonical_request=canonical_request || X'00'",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.sqlite");
        let store = Store::open(&path).unwrap();
        store
            .commit(Tx::builder().idempotency_key("key").build().unwrap())
            .unwrap();
        drop(store);
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(mutation).unwrap();
        drop(connection);
        let code = Store::open(&path).err().unwrap().code();
        assert_eq!(
            code,
            if mutation.contains("version=2") {
                ErrorCode::UnsupportedFormat
            } else {
                ErrorCode::Storage
            }
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mismatched.sqlite");
    let store = Store::open(&path).unwrap();
    store
        .commit(Tx::builder().idempotency_key("key").build().unwrap())
        .unwrap();
    drop(store);
    let changed = Tx::builder()
        .metadata("different", Value::Bool(true))
        .build()
        .unwrap()
        .canonical_request();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE transactions SET canonical_request=?1",
            [changed.bytes()],
        )
        .unwrap();
    drop(connection);
    assert_eq!(Store::open(&path).err().unwrap().code(), ErrorCode::Storage);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn retry_identity_ignores_wire_spelling_but_returns_original_after_later_commits(
        value in any::<i64>(), later in 0_usize..5, key in any::<String>(),
    ) {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("property.sqlite");
        let store = Store::open(&path).unwrap();
        let signed = if value >= 0 { format!("+00{value}") } else { format!("-00{}",value.unsigned_abs()) };
        let key = serde_json::json!(key).to_string();
        let first = format!(r#"{{"version":1,"idempotency_key":{key},"metadata":{{"i":{{"i64":"{signed}"}},"f":{{"f64":"-0.0"}},"b":{{"bytes":"AQI"}}}}}}"#);
        let retry = format!(r#"{{"version":1,"idempotency_key":{key},"metadata":{{"b":{{"bytes":"AQI="}},"f":{{"f64":"0e0"}},"i":{{"i64":"{value}"}}}}}}"#);
        let original = store.commit(wire::decode_tx(&first).unwrap()).unwrap();
        for _ in 0..later { store.commit(Tx::default()).unwrap(); }
        let head = store.head().unwrap();
        prop_assert_eq!(store.commit(wire::decode_tx(&retry).unwrap()).unwrap(),original.clone());
        prop_assert_eq!(store.head().unwrap(),head.clone());
        drop(store); let reopened = Store::open(&path).unwrap();
        prop_assert_eq!(reopened.commit(wire::decode_tx(&retry).unwrap()).unwrap(),original);
        prop_assert_eq!(reopened.head().unwrap(),head);
    }
}
