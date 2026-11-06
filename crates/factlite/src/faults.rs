//! SQLite fault injection through temporary schema objects and commit hooks.

use crate::{
    AttributeDefinition, Cardinality, CommitOutcome, ErrorCode, Store, TempId, Tx, Uniqueness,
    Value, ValueType,
};

fn request() -> Tx {
    let a = TempId::new("a").unwrap();
    let b = TempId::new("b").unwrap();
    Tx::builder()
        .define_attribute(
            AttributeDefinition::new(
                "x/one".parse().unwrap(),
                ValueType::I64,
                Cardinality::One,
                Uniqueness::None,
            )
            .unwrap(),
        )
        .define_attribute(
            AttributeDefinition::new(
                "x/two".parse().unwrap(),
                ValueType::I64,
                Cardinality::One,
                Uniqueness::None,
            )
            .unwrap(),
        )
        .new_entity(a.clone())
        .new_entity(b.clone())
        .assert(a.clone(), "x/one".parse().unwrap(), Value::I64(1))
        .assert(a, "x/two".parse().unwrap(), Value::I64(2))
        .assert(b, "x/one".parse().unwrap(), Value::I64(3))
        .metadata("source", Value::String("fault-must-not-persist".into()))
        .idempotency_key("fault-retry")
        .build()
        .unwrap()
}

#[test]
fn every_write_stage_rolls_back_reports_metadata_schema_allocations_facts_and_head() {
    let injections = [
        "CREATE TEMP TRIGGER injected BEFORE INSERT ON main.transactions BEGIN SELECT RAISE(ABORT,'report fault'); END;",
        "CREATE TEMP TRIGGER injected BEFORE INSERT ON main.schema_events BEGIN SELECT RAISE(ABORT,'schema fault'); END;",
        "CREATE TEMP TRIGGER injected BEFORE INSERT ON main.schema_events WHEN NEW.ordinal=1 BEGIN SELECT RAISE(ABORT,'late schema fault'); END;",
        "CREATE TEMP TRIGGER injected BEFORE INSERT ON main.entity_events BEGIN SELECT RAISE(ABORT,'allocation fault'); END;",
        "CREATE TEMP TRIGGER injected BEFORE INSERT ON main.entity_events WHEN NEW.ordinal=1 BEGIN SELECT RAISE(ABORT,'late allocation fault'); END;",
        "CREATE TEMP TRIGGER injected BEFORE INSERT ON main.fact_events BEGIN SELECT RAISE(ABORT,'fact fault'); END;",
        "CREATE TEMP TRIGGER injected BEFORE INSERT ON main.fact_events WHEN NEW.ordinal=1 BEGIN SELECT RAISE(ABORT,'late fact fault'); END;",
        "CREATE TEMP TRIGGER injected BEFORE INSERT ON main.datoms BEGIN SELECT RAISE(ABORT,'interval fault'); END;",
        "CREATE TEMP TRIGGER injected BEFORE INSERT ON main.datoms WHEN (SELECT COUNT(*) FROM main.datoms)>=2 BEGIN SELECT RAISE(ABORT,'late interval fault'); END;",
        "CREATE TEMP TRIGGER injected BEFORE UPDATE ON main.store_info BEGIN SELECT RAISE(ABORT,'head fault'); END;",
    ];
    for injection in injections {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fault.sqlite");
        let store = Store::open(&path).unwrap();
        let old = store
            .commit(
                Tx::builder()
                    .metadata("original", Value::Bool(true))
                    .build()
                    .unwrap(),
            )
            .unwrap();
        store
            .connection()
            .unwrap()
            .execute_batch(injection)
            .unwrap();
        let error = store.commit(request()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Storage);
        assert_eq!(error.commit_outcome(), Some(CommitOutcome::RolledBack));
        assert_eq!(store.head().unwrap(), old.after);
        {
            let connection = store.connection().unwrap();
            assert_eq!(
                connection
                    .query_row("SELECT COUNT(*) FROM transactions", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            for sql in [
                "SELECT COUNT(*) FROM schema_events",
                "SELECT COUNT(*) FROM entity_events",
                "SELECT COUNT(*) FROM fact_events",
                "SELECT COUNT(*) FROM datoms",
            ] {
                assert_eq!(
                    connection
                        .query_row(sql, [], |row| row.get::<_, i64>(0))
                        .unwrap(),
                    0
                );
            }
            connection.execute_batch("DROP TRIGGER injected").unwrap();
        }
        assert_eq!(Store::open(&path).unwrap().head().unwrap(), old.after);
        let valid = store.commit(request()).unwrap();
        assert_eq!(store.commit(request()).unwrap(), valid);
        assert_eq!(
            Store::open(&path).unwrap().commit(request()).unwrap(),
            valid
        );
        assert_eq!(valid.after.revision(), old.after.revision().next().unwrap());
        assert_eq!(valid.created_entities.len(), 2);
        assert_eq!(valid.effective_delta.len(), 3);
        let bytes: Vec<u8> = store
            .connection()
            .unwrap()
            .query_row(
                "SELECT report FROM transactions WHERE revision=?1",
                [crate::store::revision_bytes(old.after.revision()).as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(crate::codec::decode_report(&bytes).unwrap(), old);
    }
}

#[test]
fn deferred_commit_failure_explicitly_rolls_back_and_connection_remains_usable() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("deferred.sqlite")).unwrap();
    let zero = store.head().unwrap();
    store.connection().unwrap().execute_batch("CREATE TEMP TABLE parent(id INTEGER PRIMARY KEY); CREATE TEMP TABLE child(id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TEMP TRIGGER injected AFTER UPDATE ON main.store_info BEGIN INSERT INTO child VALUES(1); END;").unwrap();
    let error = store.commit(request()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::Storage);
    assert_eq!(error.commit_outcome(), Some(CommitOutcome::RolledBack));
    assert_eq!(store.head().unwrap(), zero);
    store
        .connection()
        .unwrap()
        .execute_batch("DROP TRIGGER injected")
        .unwrap();
    assert!(store.commit(request()).is_ok());
}

#[test]
fn ended_transaction_commit_error_is_conservatively_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("hook.sqlite")).unwrap();
    let zero = store.head().unwrap();
    store
        .connection()
        .unwrap()
        .commit_hook(Some(|| true))
        .unwrap();
    let error = store.commit(request()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::Storage);
    assert_eq!(error.commit_outcome(), Some(CommitOutcome::Unknown));
    let wire: serde_json::Value = serde_json::from_str(&crate::wire::encode_error(&error)).unwrap();
    assert_eq!(wire["error"]["commit_outcome"], "Unknown");
    store
        .connection()
        .unwrap()
        .commit_hook(None::<fn() -> bool>)
        .unwrap();
    assert_eq!(store.head().unwrap(), zero);
    assert!(store.commit(request()).is_ok());
}
