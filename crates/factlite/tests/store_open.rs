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
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("busy.sqlite");
    let original = Store::open(&path).unwrap().head().unwrap();
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = std::time::Instant::now();
    let error = Store::open(&path).err().unwrap();
    assert_eq!(error.code(), ErrorCode::Busy);
    assert!(error.source().is_some());
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    connection.execute_batch("ROLLBACK").unwrap();
    drop(connection);
    assert_eq!(Store::open(&path).unwrap().head().unwrap(), original);
}
