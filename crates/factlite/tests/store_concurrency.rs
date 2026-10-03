//! Separate-connection serialization and conditional-write contention.

use factlite::{
    AttributeDefinition, Cardinality, CommitOutcome, ErrorCode, Revision, Store, TempId, Tx,
    Uniqueness, Value, ValueType,
};
use std::{
    sync::{Arc, Barrier},
    time::{Duration, Instant},
};

fn create(path: &std::path::Path) -> (Store, factlite::EntityId) {
    let store = Store::open(path).unwrap();
    let label = TempId::new("task").unwrap();
    let report = store
        .commit(
            Tx::builder()
                .define_attribute(
                    AttributeDefinition::new(
                        "task/status".parse().unwrap(),
                        ValueType::String,
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
                .build()
                .unwrap(),
        )
        .unwrap();
    (store, report.tempids[&label].clone())
}

#[test]
fn competing_conditions_from_one_value_commit_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race.sqlite");
    let (store, task) = create(&path);
    let stores = [Store::open(&path).unwrap(), Store::open(&path).unwrap()];
    let barrier = Arc::new(Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = stores
            .into_iter()
            .enumerate()
            .map(|(i, connection)| {
                let barrier = barrier.clone();
                let task = task.clone();
                scope.spawn(move || {
                    barrier.wait();
                    connection.commit(
                        Tx::builder()
                            .expect(
                                task.clone(),
                                "task/status".parse().unwrap(),
                                vec![Value::String("todo".into())],
                            )
                            .assert(
                                task,
                                "task/status".parse().unwrap(),
                                Value::String(format!("doing-{i}")),
                            )
                            .build()
                            .unwrap(),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| r
                .as_ref()
                .is_err_and(|err| err.code() == ErrorCode::PreconditionFailed
                    && err.commit_outcome() == Some(CommitOutcome::RolledBack)))
            .count(),
        1
    );
    let winner = results.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(store.head().unwrap(), winner.after);
    assert_eq!(winner.after.revision(), Revision::new(2));
    assert_eq!(Store::open(&path).unwrap().head().unwrap(), winner.after);
}

#[test]
fn independent_concurrent_nochange_commits_have_unique_contiguous_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("writers.sqlite");
    let store = Store::open(&path).unwrap();
    let stores: Vec<_> = (0..12).map(|_| Store::open(&path).unwrap()).collect();
    let barrier = Arc::new(Barrier::new(stores.len()));
    let revisions = std::thread::scope(|scope| {
        let handles: Vec<_> = stores
            .into_iter()
            .enumerate()
            .map(|(i, connection)| {
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    connection
                        .commit(
                            Tx::builder()
                                .metadata("writer", Value::I64(i as i64))
                                .build()
                                .unwrap(),
                        )
                        .unwrap()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                let report = handle.join().unwrap();
                assert!(report.effective_delta.is_empty());
                report.after.revision().get()
            })
            .collect::<std::collections::BTreeSet<_>>()
    });
    assert_eq!(revisions, (1..=12).collect());
    assert_eq!(store.head().unwrap().revision(), Revision::new(12));
    assert_eq!(
        Store::open(&path).unwrap().head().unwrap(),
        store.head().unwrap()
    );
}

#[test]
fn commit_busy_is_bounded_confirmed_no_write_and_retry_after_unlock_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("busy.sqlite");
    let (store, _) = create(&path);
    let original = store.head().unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    let start = Instant::now();
    let error = store.commit(Tx::default()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::Busy);
    assert_eq!(error.commit_outcome(), Some(CommitOutcome::RolledBack));
    assert!(start.elapsed() < Duration::from_secs(10));
    assert_eq!(store.head().unwrap(), original);
    connection.execute_batch("ROLLBACK").unwrap();
    let next = store.commit(Tx::default()).unwrap();
    assert_eq!(next.before, original);
    assert_eq!(next.after.revision(), original.revision().next().unwrap());
}

#[test]
fn attribute_expectations_allow_unrelated_commits_but_head_fences_reject_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fences.sqlite");
    let (store, task) = create(&path);
    let original = store.head().unwrap();
    let other = Store::open(&path).unwrap();
    other
        .commit(
            Tx::builder()
                .new_entity(TempId::new("unrelated").unwrap())
                .metadata("note", Value::Bool(true))
                .build()
                .unwrap(),
        )
        .unwrap();
    let accepted = store
        .commit(
            Tx::builder()
                .expect(
                    task.clone(),
                    "task/status".parse().unwrap(),
                    vec![Value::String("todo".into())],
                )
                .assert(
                    task,
                    "task/status".parse().unwrap(),
                    Value::String("doing".into()),
                )
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(accepted.after.revision(), Revision::new(3));
    let error = store
        .commit(Tx::builder().expect_head(original).build().unwrap())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::HeadMismatch);
    assert_eq!(error.commit_outcome(), Some(CommitOutcome::RolledBack));
    assert_eq!(store.head().unwrap(), accepted.after);
}
