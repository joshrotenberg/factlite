//! Durable initialization, identity, format, and failure boundaries.

use factlite::{ErrorCode, Revision, Store};
use rusqlite::Connection;
use std::error::Error as _;

#[test]
fn identities_survive_close_and_reopen_and_separate_files_differ() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.sqlite");
    let a = Store::open(&path).unwrap();
    let head = a.head().unwrap();
    assert_eq!(head.revision(), Revision::ZERO);
    drop(a);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.head().unwrap(), head);
    let separate = Store::open(dir.path().join("b.sqlite")).unwrap();
    assert_ne!(separate.head().unwrap().store(), head.store());
    assert_ne!(separate.head().unwrap().lineage(), head.lineage());
}

#[test]
fn unrelated_sqlite_files_and_future_formats_are_rejected_without_schema_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unrelated.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE TABLE user_data(value TEXT);INSERT INTO user_data VALUES('keep');")
        .unwrap();
    drop(connection);
    assert_eq!(
        Store::open(&path).err().unwrap().code(),
        ErrorCode::UnsupportedFormat
    );
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT value FROM user_data", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "keep"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='store_info'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    let future = dir.path().join("future.sqlite");
    drop(Store::open(&future).unwrap());
    Connection::open(&future)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    assert_eq!(
        Store::open(&future).err().unwrap().code(),
        ErrorCode::UnsupportedFormat
    );
}

#[test]
fn inconsistent_head_and_malformed_identity_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.sqlite");
    drop(Store::open(&path).unwrap());
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE store_info SET head=?1",
            [1_u64.to_be_bytes().as_slice()],
        )
        .unwrap();
    drop(connection);
    assert_eq!(Store::open(&path).err().unwrap().code(), ErrorCode::Storage);
    let path = dir.path().join("identity.sqlite");
    drop(Store::open(&path).unwrap());
    let connection = Connection::open(&path).unwrap();
    connection
        .execute("UPDATE store_info SET store_id='malformed'", [])
        .unwrap();
    drop(connection);
    assert_eq!(Store::open(&path).err().unwrap().code(), ErrorCode::Storage);
}

#[test]
fn invalid_paths_and_non_databases_retain_storage_source_errors() {
    let dir = tempfile::tempdir().unwrap();
    let error = Store::open(dir.path().join("missing/parent.sqlite"))
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::Storage);
    assert!(error.source().is_some());
    let path = dir.path().join("text.sqlite");
    std::fs::write(&path, b"not a SQLite database").unwrap();
    let error = Store::open(&path).err().unwrap();
    assert_eq!(error.code(), ErrorCode::Storage);
    assert!(error.source().is_some());
    assert_eq!(
        Store::open(":memory:").err().unwrap().code(),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        Store::open("").err().unwrap().code(),
        ErrorCode::InvalidRequest
    );
}

#[test]
fn schema_only_views_and_sqlite_like_prefixes_are_not_empty_databases() {
    let dir = tempfile::tempdir().unwrap();
    for (name, ddl) in [
        ("view.sqlite", "CREATE VIEW user_view AS SELECT 1"),
        ("prefix.sqlite", "CREATE TABLE sqliteXuser(value TEXT)"),
    ] {
        let path = dir.path().join(name);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(ddl).unwrap();
        drop(connection);
        assert_eq!(
            Store::open(&path).err().unwrap().code(),
            ErrorCode::UnsupportedFormat
        );
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE name='store_info'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}

#[test]
fn held_writer_lock_has_bounded_busy_failure() {
    // Initializing a new file needs the write lock, so it waits a bounded time.
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir.path().join("fresh.sqlite");
    let blocker = Connection::open(&fresh).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = std::time::Instant::now();
    let error = Store::open(&fresh).err().unwrap();
    assert_eq!(error.code(), ErrorCode::Busy);
    assert!(error.source().is_some());
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    blocker.execute_batch("ROLLBACK").unwrap();
    drop(blocker);
    // Opening an existing store does not; its commits wait a bounded time and
    // report a confirmed rollback.
    let path = dir.path().join("busy.sqlite");
    let original = Store::open(&path).unwrap().head().unwrap();
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    let store = Store::open(&path).unwrap();
    let started = std::time::Instant::now();
    let error = store.commit(factlite::Tx::default()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::Busy);
    assert_eq!(
        error.commit_outcome(),
        Some(factlite::CommitOutcome::RolledBack)
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    connection.execute_batch("ROLLBACK").unwrap();
    drop(connection);
    assert_eq!(Store::open(&path).unwrap().head().unwrap(), original);
    assert_eq!(
        Store::open(&fresh).unwrap().head().unwrap().revision(),
        Revision::ZERO
    );
}

#[test]
fn opening_an_existing_store_neither_takes_nor_waits_for_the_write_lock() {
    use std::time::{Duration, Instant};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("locked.sqlite");
    let store = Store::open(&path).unwrap();
    let head = store.commit(factlite::Tx::default()).unwrap().after;
    let blocker = Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = Instant::now();
    let opened = Store::open(&path).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "open waited {:?} for another writer",
        started.elapsed()
    );
    assert_eq!(opened.head().unwrap(), head);
    // Reads work while the other connection still holds the write lock.
    assert_eq!(
        opened
            .db()
            .unwrap()
            .transactions(Revision::ZERO)
            .unwrap()
            .len(),
        1
    );
    blocker.execute_batch("ROLLBACK").unwrap();
    assert!(opened.commit(factlite::Tx::default()).is_ok());
}

#[test]
fn concurrent_first_opens_initialize_one_store_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fresh.sqlite");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
    let handles: Vec<_> = (0..6)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                Store::open(&path).unwrap().head().unwrap()
            })
        })
        .collect();
    let heads: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(heads.iter().all(|head| head == &heads[0]));
    assert_eq!(heads[0].revision(), Revision::ZERO);
}

#[test]
fn busy_timeout_is_configurable_and_validated() {
    use factlite::StoreOptions;
    use std::time::{Duration, Instant};
    for invalid in [
        Duration::from_micros(1500),
        Duration::from_millis(i32::MAX as u64 + 1),
    ] {
        assert_eq!(
            StoreOptions::new()
                .with_busy_timeout(invalid)
                .unwrap_err()
                .code(),
            ErrorCode::InvalidRequest
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("timeout.sqlite");
    let options = StoreOptions::new()
        .with_busy_timeout(Duration::from_millis(150))
        .unwrap();
    assert_eq!(options.busy_timeout(), Duration::from_millis(150));
    let store = Store::open_with(&path, &options).unwrap();
    let blocker = Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = Instant::now();
    let error = store.commit(factlite::Tx::default()).unwrap_err();
    let waited = started.elapsed();
    assert_eq!(error.code(), ErrorCode::Busy);
    assert!(
        waited >= Duration::from_millis(150) && waited < Duration::from_secs(2),
        "{waited:?}"
    );
    // Zero fails at once.
    let instant = Store::open_with(
        &path,
        &StoreOptions::new()
            .with_busy_timeout(Duration::ZERO)
            .unwrap(),
    )
    .unwrap();
    let started = Instant::now();
    assert_eq!(
        instant.commit(factlite::Tx::default()).unwrap_err().code(),
        ErrorCode::Busy
    );
    assert!(started.elapsed() < Duration::from_millis(100));
    blocker.execute_batch("ROLLBACK").unwrap();
    assert!(store.commit(factlite::Tx::default()).is_ok());
}

#[test]
fn read_only_views_refresh_only_explicitly_over_one_connection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("refresh.sqlite");
    let store = Store::open(&path).unwrap();
    let a = store.commit(factlite::Tx::default()).unwrap().after;
    let reader = factlite::Db::open_read_only_with(&path, &factlite::StoreOptions::new()).unwrap();
    assert_eq!(reader.snapshot(), &a);
    let b = store.commit(factlite::Tx::default()).unwrap().after;
    // The pinned view does not move; an explicit refresh does.
    assert_eq!(reader.snapshot(), &a);
    assert_eq!(reader.transactions(Revision::ZERO).unwrap().len(), 1);
    let latest = reader.at_head().unwrap();
    assert_eq!(latest.snapshot(), &b);
    assert_eq!(latest.transactions(Revision::ZERO).unwrap().len(), 2);
    assert_eq!(reader.at(&a).unwrap().snapshot(), &a);
    let future =
        factlite::SnapshotId::new(b.store().clone(), b.lineage().clone(), Revision::new(9));
    assert_eq!(
        reader.at(&future).err().unwrap().code(),
        ErrorCode::InvalidSnapshot
    );
}
