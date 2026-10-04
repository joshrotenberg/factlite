//! Deterministic WAL interleavings; hooks exist only in the test build.

use std::sync::{Arc, Mutex};

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

use crate::{
    AttributeDefinition, Cardinality, FactFilter, PulledValue, Revision, Store, TempId, Tx,
    Uniqueness, Value, ValueType, log::reports_between, store::read_head,
};

fn definition(name: &str) -> AttributeDefinition {
    AttributeDefinition::new(
        name.parse().unwrap(),
        ValueType::I64,
        Cardinality::One,
        Uniqueness::None,
    )
    .unwrap()
}

#[test]
fn operation_sql_snapshot_keeps_head_and_log_consistent_while_writer_commits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.sqlite");
    let store = Store::open(&path).unwrap();
    let a = store.commit(Tx::default()).unwrap();
    let db = store.db().unwrap();
    let writer = Store::open(&path).unwrap();
    let b = db
        .read(|transaction, model| {
            assert_eq!(model.catalog.snapshot, a.after);
            assert_eq!(read_head(transaction)?, a.after);
            let b = writer.commit(
                Tx::builder()
                    .metadata("writer", Value::Bool(true))
                    .build()
                    .unwrap(),
            )?;
            // WAL permits this commit while our read operation is still open.
            assert_eq!(writer.head()?, b.after);
            assert_eq!(read_head(transaction)?, a.after);
            let count: i64 =
                transaction.query_row("SELECT COUNT(*) FROM transactions", [], |row| row.get(0))?;
            assert_eq!(count, 1);
            // Even with a deliberately broad bound, this SQL snapshot has no B row.
            assert_eq!(
                reports_between(transaction, Revision::ZERO, b.after.revision())?,
                vec![a.clone()]
            );
            Ok(b)
        })
        .unwrap();
    assert_eq!(store.head().unwrap(), b.after);
    assert_eq!(db.transactions(Revision::ZERO).unwrap()[0].report, a);
    assert_eq!(
        store
            .db()
            .unwrap()
            .transactions(Revision::ZERO)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn every_public_read_stays_at_a_when_writer_commits_between_head_and_replay() {
    for operation in [
        "schema",
        "facts",
        "pull",
        "history",
        "transactions",
        "query",
        "query-temp",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("interleaving.sqlite");
        let store = Store::open(&path).unwrap();
        let label = TempId::new("e").unwrap();
        let a = store
            .commit(
                Tx::builder()
                    .define_attribute(definition("x/value"))
                    .new_entity(label.clone())
                    .assert(label, "x/value".parse().unwrap(), Value::I64(1))
                    .build()
                    .unwrap(),
            )
            .unwrap();
        let id = a.created_entities[0].clone();
        let db = store.db().unwrap();
        let schema_a = db.schema().unwrap();
        let facts_a = db.facts(FactFilter::default()).unwrap();
        let pull_a = db.pull(&id, &["x/value".parse().unwrap()]).unwrap();
        let history_a = db.history(FactFilter::default(), Revision::ZERO).unwrap();
        let transactions_a = db.transactions(Revision::ZERO).unwrap();
        let query = crate::Query {
            patterns: vec![crate::Pattern {
                entity: crate::Term::Var(crate::QueryName::new("e").unwrap()),
                attribute: "x/value".parse().unwrap(),
                value: crate::Term::Var(crate::QueryName::new("v").unwrap()),
            }],
            select: vec![crate::QueryName::new("v").unwrap()],
            ..crate::Query::default()
        };
        let query_a = db.query(&query, &crate::Params::new()).unwrap();
        let writer = Store::open(&path).unwrap();
        let mut pending = Some(
            Tx::builder()
                .define_attribute(definition("x/later"))
                .assert(id.clone(), "x/value".parse().unwrap(), Value::I64(2))
                .metadata("writer", Value::Bool(true))
                .build()
                .unwrap(),
        );
        let outcome = Arc::new(Mutex::new(None));
        let captured = outcome.clone();
        store
            .connection()
            .unwrap()
            .authorizer(Some(move |context: AuthContext<'_>| {
                // Force B either before log replay or after the query's pinned
                // relation is materialized, immediately before SQL reads it.
                let table = if operation == "query-temp" { "factlite_query_facts" } else { "transactions" };
                if matches!(context.action, AuthAction::Read { table_name, .. } if table_name == table)
                    && let Some(tx) = pending.take()
                {
                    *captured.lock().unwrap() = Some(writer.commit(tx));
                }
                Authorization::Allow
            }))
            .unwrap();
        match operation {
            "schema" => assert_eq!(db.schema().unwrap(), schema_a),
            "facts" => assert_eq!(db.facts(FactFilter::default()).unwrap(), facts_a),
            "pull" => assert_eq!(db.pull(&id, &["x/value".parse().unwrap()]).unwrap(), pull_a),
            "history" => assert_eq!(
                db.history(FactFilter::default(), Revision::ZERO).unwrap(),
                history_a
            ),
            "transactions" => assert_eq!(db.transactions(Revision::ZERO).unwrap(), transactions_a),
            "query" | "query-temp" => {
                assert_eq!(db.query(&query, &crate::Params::new()).unwrap(), query_a)
            }
            _ => unreachable!(),
        }
        store
            .connection()
            .unwrap()
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        let b = outcome
            .lock()
            .unwrap()
            .take()
            .expect("writer hook must fire")
            .unwrap();
        assert_eq!(b.before, a.after);
        assert_eq!(store.head().unwrap(), b.after);
        let current = store.db().unwrap();
        assert_eq!(current.schema().unwrap().attributes().count(), 2);
        assert_eq!(
            current
                .pull(&id, &["x/value".parse().unwrap()])
                .unwrap()
                .unwrap()
                .attributes[&"x/value".parse().unwrap()],
            PulledValue::One(Value::I64(2))
        );
        assert_eq!(current.transactions(Revision::ZERO).unwrap().len(), 2);
        assert_eq!(db.schema().unwrap(), schema_a);
        assert_eq!(db.facts(FactFilter::default()).unwrap(), facts_a);
    }
}
