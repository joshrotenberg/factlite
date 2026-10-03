use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, TransactionBehavior};

use crate::{
    CommitOutcome, Error, ErrorCode, ErrorContext, Result, Revision, SnapshotId, Tx, TxReport,
};

const APPLICATION_ID: i32 = 0x464c4954;
const FORMAT_VERSION: i32 = 1;
const TABLES: [&str; 5] = [
    "entity_events",
    "fact_events",
    "schema_events",
    "store_info",
    "transactions",
];

const DDL: &str = "
CREATE TABLE store_info (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 format INTEGER NOT NULL,
 store_id TEXT NOT NULL,
 lineage_id TEXT NOT NULL,
 head BLOB NOT NULL CHECK(typeof(head)='blob' AND length(head)=8)
);
CREATE TABLE transactions (
 revision BLOB PRIMARY KEY NOT NULL CHECK(typeof(revision)='blob' AND length(revision)=8),
 report BLOB NOT NULL,
 report_encoding INTEGER NOT NULL,
 request_version INTEGER NOT NULL,
 canonical_version INTEGER,
 canonical_request BLOB,
 idempotency_key TEXT UNIQUE,
 CHECK((idempotency_key IS NULL AND canonical_version IS NULL AND canonical_request IS NULL)
    OR (idempotency_key IS NOT NULL AND canonical_version IS NOT NULL AND canonical_request IS NOT NULL))
);
CREATE TABLE schema_events (
 revision BLOB NOT NULL REFERENCES transactions(revision),
 ordinal INTEGER NOT NULL CHECK(ordinal>=0),
 attribute TEXT NOT NULL UNIQUE,
 definition BLOB NOT NULL,
 PRIMARY KEY(revision,ordinal)
);
CREATE TABLE entity_events (
 revision BLOB NOT NULL REFERENCES transactions(revision),
 ordinal INTEGER NOT NULL CHECK(ordinal>=0),
 entity TEXT NOT NULL UNIQUE,
 PRIMARY KEY(revision,ordinal)
);
CREATE TABLE fact_events (
 revision BLOB NOT NULL REFERENCES transactions(revision),
 ordinal INTEGER NOT NULL CHECK(ordinal>=0),
 operation INTEGER NOT NULL CHECK(operation IN (0,1)),
 entity TEXT NOT NULL REFERENCES entity_events(entity),
 attribute TEXT NOT NULL REFERENCES schema_events(attribute),
 type_tag INTEGER NOT NULL CHECK(type_tag BETWEEN 0 AND 5),
 value BLOB NOT NULL,
 PRIMARY KEY(revision,ordinal)
);";

/// Writable durable store. Clones share one connection; other opens use SQLite locking.
#[derive(Clone)]
pub struct Store {
    connection: Arc<Mutex<Connection>>,
}

impl Store {
    /// Validates and commits one declarative batch against serialized pre-head.
    /// Fresh no-change requests receive revisions. Retry keys are explicitly
    /// rejected until exact idempotency is implemented. Failures expose outcome
    /// evidence, including confirmed rollback and uncertain COMMIT outcomes.
    pub fn commit(&self, request: Tx) -> Result<TxReport> {
        let request = request
            .validate()
            .map_err(|err| err.with_commit_outcome(CommitOutcome::RolledBack))?;
        if request.idempotency_key().is_some() {
            return Err(Error::invalid(
                "idempotency_key",
                "idempotency keys are not supported by this build",
            )
            .with_commit_outcome(CommitOutcome::RolledBack));
        }
        let mut connection = self
            .connection()
            .map_err(|err| err.with_commit_outcome(CommitOutcome::RolledBack))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|source| {
                Error::storage("begin commit", source)
                    .with_commit_outcome(CommitOutcome::RolledBack)
            })?;
        let result = (|| -> Result<TxReport> {
            let basis = read_head(&transaction)?;
            let model = crate::log::replay(&transaction, &basis)?;
            let (_, report) = model.evaluate(&request)?;
            crate::log::persist_report(&transaction, &report)?;
            Ok(report)
        })();
        let report = match result {
            Ok(report) => report,
            Err(error) => return Err(abort_commit(transaction, error)),
        };
        // Keep the guard until COMMIT's outcome is inspected; consuming rusqlite's
        // commit method would hide errors from its drop-time rollback attempt.
        match transaction.execute_batch("COMMIT") {
            Ok(()) => {
                drop(transaction);
                Ok(report)
            }
            Err(source) => {
                let error = Error::storage("commit", source);
                if transaction.is_autocommit() {
                    drop(transaction);
                    Err(error.with_commit_outcome(CommitOutcome::Unknown))
                } else {
                    Err(abort_commit(transaction, error))
                }
            }
        }
    }

    /// Opens or atomically initializes a file with WAL, FULL sync, and a two-second
    /// busy timeout. Unsupported/non-Factlite files fail before schema changes.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.as_os_str().is_empty() || path == Path::new(":memory:") {
            return Err(Error::invalid(
                "path",
                "Factlite requires a durable file path",
            ));
        }
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let application: i32 =
            transaction.pragma_query_value(None, "application_id", |row| row.get(0))?;
        let version: i32 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let tables = table_names(&transaction)?;
        let user_objects: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*'",
            [],
            |row| row.get(0),
        )?;
        if application == 0 && version == 0 && tables.is_empty() && user_objects == 0 {
            transaction.execute_batch(DDL)?;
            transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
            transaction.pragma_update(None, "user_version", FORMAT_VERSION)?;
            let store = format!("s1.{}", uuid::Uuid::new_v4().simple());
            let lineage = format!("l1.{}", uuid::Uuid::new_v4().simple());
            transaction.execute("INSERT INTO store_info(singleton,format,store_id,lineage_id,head) VALUES(1,?1,?2,?3,?4)",rusqlite::params![FORMAT_VERSION,store,lineage,revision_bytes(Revision::ZERO).as_slice()])?;
        } else if application != APPLICATION_ID || version != FORMAT_VERSION {
            return Err(Error::new(
                ErrorCode::UnsupportedFormat,
                ErrorContext::InvalidInput { field: "format" },
                "file is not a supported Factlite database",
            ));
        }
        if table_names(&transaction)? != TABLES {
            return Err(corrupt("database tables do not match the supported format"));
        }
        validate_head(&transaction)?;
        transaction.commit()?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let mode: String = connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        if mode != "wal" {
            return Err(corrupt("WAL journal mode could not be enabled"));
        }
        connection.pragma_update(None, "synchronous", "FULL")?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Captures the latest committed head in one short SQLite read transaction.
    pub fn head(&self) -> Result<SnapshotId> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let snapshot = read_head(&transaction)?;
        transaction.commit()?;
        Ok(snapshot)
    }

    pub(crate) fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| corrupt("store connection lock poisoned"))
    }
}

fn abort_commit(transaction: rusqlite::Transaction<'_>, error: Error) -> Error {
    if transaction.is_autocommit() {
        drop(transaction);
        return error.with_commit_outcome(CommitOutcome::RolledBack);
    }
    match transaction.rollback() {
        Ok(()) => error.with_commit_outcome(CommitOutcome::RolledBack),
        Err(source) => Error::storage("rollback after failed commit", source)
            .with_commit_outcome(CommitOutcome::Unknown),
    }
}

pub(crate) fn revision_bytes(revision: Revision) -> [u8; 8] {
    revision.get().to_be_bytes()
}

pub(crate) fn decode_revision(bytes: &[u8]) -> Result<Revision> {
    let bytes: [u8; 8] = bytes
        .try_into()
        .map_err(|_| corrupt("invalid stored revision encoding"))?;
    Ok(Revision::new(u64::from_be_bytes(bytes)))
}

pub(crate) fn corrupt(message: impl Into<String>) -> Error {
    Error::new(
        ErrorCode::Storage,
        ErrorContext::Storage {
            operation: "validate stored state",
        },
        message,
    )
}

pub(crate) fn read_head(connection: &Connection) -> Result<SnapshotId> {
    let count: i64 =
        connection.query_row("SELECT COUNT(*) FROM store_info", [], |row| row.get(0))?;
    if count != 1 {
        return Err(corrupt("store_info must contain exactly one row"));
    }
    let (format, store, lineage, head): (i32, String, String, Vec<u8>) = connection.query_row(
        "SELECT format,store_id,lineage_id,head FROM store_info WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    if format != FORMAT_VERSION {
        return Err(Error::new(
            ErrorCode::UnsupportedFormat,
            ErrorContext::InvalidInput { field: "format" },
            "unsupported stored format version",
        ));
    }
    let store = store
        .parse()
        .map_err(|err: Error| corrupt(err.to_string()))?;
    let lineage = lineage
        .parse()
        .map_err(|err: Error| corrupt(err.to_string()))?;
    Ok(SnapshotId::new(store, lineage, decode_revision(&head)?))
}

fn table_names(connection: &Connection) -> Result<Vec<String>> {
    Ok(connection.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name")?.query_map([],|row|row.get(0))?.collect::<std::result::Result<_,_>>()?)
}

fn validate_head(connection: &Connection) -> Result<()> {
    let basis = read_head(connection)?;
    let head = basis.revision();
    let maximum: Option<Vec<u8>> =
        connection.query_row("SELECT MAX(revision) FROM transactions", [], |row| {
            row.get(0)
        })?;
    let maximum = maximum.as_deref().map(decode_revision).transpose()?;
    if maximum.is_some_and(|revision| revision == Revision::ZERO)
        || maximum.unwrap_or(Revision::ZERO) != head
    {
        return Err(corrupt(
            "committed head does not match retained transaction log",
        ));
    }
    crate::log::verify_foreign_keys(connection)?;
    crate::log::replay(connection, &basis)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn durable_settings_and_zero_state_are_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("store.sqlite")).unwrap();
        assert_eq!(store.head().unwrap().revision(), Revision::ZERO);
        let connection = store.connection().unwrap();
        let sync: i32 = connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .unwrap();
        assert_eq!(sync, 2);
        let fk: i32 = connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(fk, 1);
        let busy: i32 = connection
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        assert_eq!(busy, 2000);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        #[test]
        fn big_endian_revisions_preserve_full_unsigned_order(a in any::<u64>(),b in any::<u64>()) {
            prop_assert_eq!(revision_bytes(Revision::new(a)).cmp(&revision_bytes(Revision::new(b))),a.cmp(&b));
            prop_assert_eq!(decode_revision(&revision_bytes(Revision::new(a))).unwrap(),Revision::new(a));
        }
    }
}
