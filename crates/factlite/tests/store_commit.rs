//! Public atomic commits and their observable failure outcomes.

use factlite::{
    AttributeDefinition, Cardinality, CommitOutcome, ErrorCode, Operation, Revision, Store, TempId,
    Tx, Uniqueness, Value, ValueType,
};

fn definition(name: &str, ty: ValueType, unique: Uniqueness) -> AttributeDefinition {
    AttributeDefinition::new(name.parse().unwrap(), ty, Cardinality::One, unique).unwrap()
}

#[test]
fn create_replace_noop_and_reopen_have_exact_reports() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("commit.sqlite");
    let store = Store::open(&path).unwrap();
    let label = TempId::new("task").unwrap();
    let a = store
        .commit(
            Tx::builder()
                .define_attribute(definition(
                    "task/status",
                    ValueType::String,
                    Uniqueness::None,
                ))
                .new_entity(label.clone())
                .assert(
                    label.clone(),
                    "task/status".parse().unwrap(),
                    Value::String("todo".into()),
                )
                .metadata("source", Value::String("integration".into()))
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(a.before.revision(), Revision::ZERO);
    assert_eq!(a.after.revision(), Revision::new(1));
    assert_eq!(a.metadata["source"], Value::String("integration".into()));
    let task = a.tempids[&label].clone();
    let update = Tx::builder()
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
        .unwrap();
    let b = store.commit(update.clone()).unwrap();
    assert_eq!(b.before, a.after);
    assert_eq!(b.effective_delta.len(), 2);
    assert_eq!(b.effective_delta[0].operation, Operation::Retract);
    assert_eq!(b.effective_delta[1].operation, Operation::Assert);
    let error = store.commit(update).unwrap_err();
    assert_eq!(error.code(), ErrorCode::PreconditionFailed);
    assert_eq!(error.commit_outcome(), Some(CommitOutcome::RolledBack));
    assert_eq!(store.head().unwrap(), b.after);
    let c = store
        .commit(
            Tx::builder()
                .assert(
                    task,
                    "task/status".parse().unwrap(),
                    Value::String("doing".into()),
                )
                .build()
                .unwrap(),
        )
        .unwrap();
    assert!(c.effective_delta.is_empty());
    assert_eq!(c.after.revision(), Revision::new(3));
    drop(store);
    assert_eq!(Store::open(&path).unwrap().head().unwrap(), c.after);
}

#[test]
fn late_constraints_roll_back_schema_allocation_metadata_and_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollback.sqlite");
    let store = Store::open(&path).unwrap();
    let a = TempId::new("a").unwrap();
    let report = store
        .commit(
            Tx::builder()
                .define_attribute(definition(
                    "person/name",
                    ValueType::String,
                    Uniqueness::UniqueOne,
                ))
                .new_entity(a.clone())
                .assert(
                    a,
                    "person/name".parse().unwrap(),
                    Value::String("Ada".into()),
                )
                .build()
                .unwrap(),
        )
        .unwrap();
    let b = TempId::new("b").unwrap();
    let failed = Tx::builder()
        .define_attribute(definition("task/new", ValueType::I64, Uniqueness::None))
        .new_entity(b.clone())
        .assert(
            b,
            "person/name".parse().unwrap(),
            Value::String("Ada".into()),
        )
        .metadata("failed", Value::Bool(true))
        .build()
        .unwrap();
    let error = store.commit(failed).unwrap_err();
    assert_eq!(error.code(), ErrorCode::UniqueViolation);
    assert_eq!(error.commit_outcome(), Some(CommitOutcome::RolledBack));
    assert_eq!(store.head().unwrap(), report.after);
    let connection = rusqlite::Connection::open(&path).unwrap();
    for table in [
        "transactions",
        "schema_events",
        "entity_events",
        "fact_events",
    ] {
        let sql = match table {
            "transactions" => "SELECT COUNT(*) FROM transactions",
            "schema_events" => "SELECT COUNT(*) FROM schema_events",
            "entity_events" => "SELECT COUNT(*) FROM entity_events",
            _ => "SELECT COUNT(*) FROM fact_events",
        };
        assert_eq!(
            connection
                .query_row(sql, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    drop(connection);
    drop(store);
    assert_eq!(Store::open(&path).unwrap().head().unwrap(), report.after);
}

#[test]
fn keyed_requests_are_rejected_and_nochange_head_fences_are_strict() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("keys.sqlite")).unwrap();
    let zero = store.head().unwrap();
    let error = store
        .commit(Tx::builder().idempotency_key("retry").build().unwrap())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidRequest);
    assert_eq!(error.commit_outcome(), Some(CommitOutcome::RolledBack));
    assert_eq!(store.head().unwrap(), zero);
    let one = store.commit(Tx::default()).unwrap();
    assert!(one.effective_delta.is_empty());
    let error = store
        .commit(Tx::builder().expect_head(zero).build().unwrap())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::HeadMismatch);
    assert_eq!(store.head().unwrap(), one.after);
}

#[test]
fn every_value_kind_survives_durable_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("values.sqlite");
    let store = Store::open(&path).unwrap();
    let target = TempId::new("target").unwrap();
    let created = store
        .commit(Tx::builder().new_entity(target.clone()).build().unwrap())
        .unwrap();
    let target = created.tempids[&target].clone();
    let values = [
        Value::Bool(false),
        Value::I64(i64::MIN),
        Value::I64(i64::MAX),
        Value::f64(-0.0).unwrap(),
        Value::f64(f64::MAX).unwrap(),
        Value::String("é\0exact".into()),
        Value::Bytes(vec![0, 255]),
        Value::Ref(target),
    ];
    for (index, value) in values.into_iter().enumerate() {
        let name = format!("x/value{index}");
        let label = TempId::new(format!("entity{index}")).unwrap();
        store
            .commit(
                Tx::builder()
                    .define_attribute(definition(&name, value.value_type(), Uniqueness::None))
                    .new_entity(label.clone())
                    .assert(label, name.parse().unwrap(), value)
                    .build()
                    .unwrap(),
            )
            .unwrap();
    }
    let head = store.head().unwrap();
    drop(store);
    assert_eq!(Store::open(&path).unwrap().head().unwrap(), head);
}
