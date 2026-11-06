use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use rusqlite::{Connection, OpenFlags, TransactionBehavior};

use crate::{
    CommitOutcome, Error, ErrorCode, ErrorContext, Result, Revision, SnapshotId, Tx, TxReport,
};

const APPLICATION_ID: i32 = 0x464c4954;
const FORMAT_VERSION: i32 = 4;
/// Format 1 lacks the datom table, format 2 the attribute/value index, and
/// format 3 type tags 6 and 7; writable opens upgrade step by step.
const FORMAT_V2: i32 = 2;
const FORMAT_V3: i32 = 3;
/// Format 1 lacks the derived datom index; writable opens upgrade it in place.
const FORMAT_V1: i32 = 1;
const TABLES_V1: [&str; 5] = [
    "entity_events",
    "fact_events",
    "schema_events",
    "store_info",
    "transactions",
];
const TABLES: [&str; 6] = [
    "datoms",
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
";

/// Authoritative effective fact events. Type tags 0..=7 are bool, i64, f64,
/// string, bytes, ref, keyword and inst.
fn fact_events_table(name: &str) -> String {
    format!(
        "CREATE TABLE {name} (
 revision BLOB NOT NULL REFERENCES transactions(revision),
 ordinal INTEGER NOT NULL CHECK(ordinal>=0),
 operation INTEGER NOT NULL CHECK(operation IN (0,1)),
 entity TEXT NOT NULL REFERENCES entity_events(entity),
 attribute TEXT NOT NULL REFERENCES schema_events(attribute),
 type_tag INTEGER NOT NULL CHECK(type_tag BETWEEN 0 AND 7),
 value BLOB NOT NULL,
 PRIMARY KEY(revision,ordinal)
);"
    )
}

/// How much an open checks before returning.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Validation {
    /// Replay and validate the complete retained log, and check the datom
    /// index against it. Required for writable opens.
    #[default]
    Full,
    /// Check only the format markers, the table and index sets, and that the
    /// head row matches the latest transaction. Read-only opens only. Later
    /// reads trust the stored index; a corrupted file can give wrong answers or
    /// `Storage` errors until [`Store::verify`] or [`crate::Db::verify`] runs.
    Structural,
}

/// Connection settings for [`Store::open_with`] and [`crate::Db::open_read_only_with`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreOptions {
    busy_timeout: Duration,
    validation: Validation,
}

impl Default for StoreOptions {
    fn default() -> Self {
        Self {
            busy_timeout: Duration::from_secs(2),
            validation: Validation::Full,
        }
    }
}

impl StoreOptions {
    /// The defaults: a two-second busy timeout.
    pub fn new() -> Self {
        Self::default()
    }

    /// How long an operation waits for another connection's lock before failing
    /// `Busy`. Zero fails at once. SQLite takes whole milliseconds up to
    /// `i32::MAX`; longer timeouts and sub-millisecond remainders are rejected.
    pub fn with_busy_timeout(mut self, timeout: Duration) -> Result<Self> {
        if !timeout.as_nanos().is_multiple_of(1_000_000) || timeout.as_millis() > i32::MAX as u128 {
            return Err(Error::invalid(
                "busy_timeout",
                "busy timeout must be whole milliseconds no greater than i32::MAX",
            ));
        }
        self.busy_timeout = timeout;
        Ok(self)
    }

    /// The configured busy timeout.
    pub fn busy_timeout(&self) -> Duration {
        self.busy_timeout
    }

    /// How much an open validates. [`Validation::Structural`] is accepted only
    /// by read-only opens; [`Store::open_with`] refuses it.
    pub fn with_validation(mut self, validation: Validation) -> Self {
        self.validation = validation;
        self
    }

    /// The configured validation.
    pub fn validation(&self) -> Validation {
        self.validation
    }
}

/// Writable durable store. Clones share one connection; other opens use SQLite locking.
#[derive(Clone)]
pub struct Store {
    connection: Arc<Mutex<Connection>>,
}

impl Store {
    /// Captures committed head once and returns a stable, shareable database view.
    pub fn db(&self) -> Result<crate::Db> {
        Ok(crate::Db::pinned(self.clone(), self.head()?))
    }

    /// Opens an explicit retained snapshot, validating full identity and history.
    pub fn db_at(&self, snapshot: &SnapshotId) -> Result<crate::Db> {
        let db = crate::Db::pinned(self.clone(), snapshot.clone());
        db.schema()?;
        Ok(db)
    }

    /// Validates and commits one declarative batch against serialized pre-head.
    /// Fresh no-change requests receive revisions. Exact keyed retries return the
    /// original report before current-state validation. Failures expose outcome
    /// evidence, including confirmed rollback and uncertain COMMIT outcomes.
    pub fn commit(&self, request: Tx) -> Result<TxReport> {
        let request = request
            .validate()
            .map_err(|err| err.with_commit_outcome(CommitOutcome::RolledBack))?;
        let canonical = request.canonical_request();
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
            if let Some(key) = request.idempotency_key()
                && let Some(report) = crate::log::retry_report(&transaction, key, &canonical)?
            {
                return Ok(report);
            }
            let basis = read_head(&transaction)?;
            let model = crate::index::model_for_request(&transaction, &basis, &request)?;
            let (_, report) = model.evaluate_owned(&request)?;
            crate::log::persist_report(
                &transaction,
                &report,
                request.idempotency_key().map(|key| (key, &canonical)),
            )?;
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
        Self::open_with(path, &StoreOptions::default())
    }

    /// Like [`Store::open`], with explicit connection settings.
    pub fn open_with(path: impl AsRef<Path>, options: &StoreOptions) -> Result<Self> {
        let path = path.as_ref();
        // A commit evaluates against the stored index, so a writable store must
        // be fully validated. Refuse before the file is opened or created.
        if options.validation != Validation::Full {
            return Err(Error::invalid(
                "validation",
                "structural validation is only for read-only opens",
            ));
        }
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
        let configure = |e| Error::storage("open: configure connection", e);
        connection
            .busy_timeout(options.busy_timeout)
            .map_err(configure)?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(configure)?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(configure)?;
        // An existing current-format store is validated in a deferred read
        // transaction: under WAL its snapshot never blocks writers, and it never
        // waits for them. Only initialization and the format-1 upgrade write,
        // so only they take the write lock, and they recheck the file once it
        // is held because another process may have changed it meanwhile.
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|e| Error::storage("open: begin read", e))?;
        match file_kind(&transaction)? {
            FileKind::Current => {
                validate_current(&transaction)?;
                transaction.commit()?;
            }
            FileKind::Empty | FileKind::Older(_) => {
                transaction.rollback()?;
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(|e| Error::storage("open: begin write", e))?;
                match file_kind(&transaction)? {
                    FileKind::Empty => initialize(&transaction)?,
                    FileKind::Older(version) => upgrade(&transaction, version)?,
                    FileKind::Current => {}
                }
                validate_current(&transaction)?;
                transaction
                    .commit()
                    .map_err(|e| Error::storage("open: commit initialization", e))?;
            }
        }
        enable_wal(&connection, options.busy_timeout)?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(|e| Error::storage("open: configure connection", e))?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    pub(crate) fn reader(path: &Path, options: &StoreOptions) -> Result<crate::Db> {
        if path.as_os_str().is_empty() || path == Path::new(":memory:") {
            return Err(Error::invalid(
                "path",
                "Factlite requires a durable file path",
            ));
        }
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )?;
        connection.busy_timeout(options.busy_timeout)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        match options.validation {
            Validation::Full => validate_existing(&transaction)?,
            Validation::Structural => validate_structure(&transaction)?,
        }
        let basis = read_head(&transaction)?;
        transaction.commit()?;
        let store = Self {
            connection: Arc::new(Mutex::new(connection)),
        };
        Ok(crate::Db::pinned(store, basis))
    }

    /// Fully validates the latest committed state, as a full open does, in one
    /// read transaction, and returns the head it validated. It holds this
    /// handle's connection for as long as validation takes, so clones of this
    /// `Store` and views opened from it wait; other processes and handles do
    /// not. Errors keep
    /// their codes: `Storage` names the failed check, `UnsupportedFormat` a
    /// changed format marker, and `Busy` a lock that outlasted the timeout.
    pub fn verify(&self) -> Result<SnapshotId> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        validate_existing(&transaction)?;
        let head = read_head(&transaction)?;
        transaction.commit()?;
        Ok(head)
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

// Switching a rollback-journal file to WAL needs exclusive access. When other
// connections are opening the same new file at once, SQLite can return
// SQLITE_BUSY for this step without invoking the busy handler, or report the
// old mode instead of switching. Retry until the pragma reports "wal", within
// the configured busy timeout overall: each attempt's own busy wait is limited
// to the remaining budget, and the original timeout is restored afterwards.
// Once any connection has switched, the pragma is a no-op that reports "wal".
// A file another connection keeps open in rollback mode also uses up the whole
// budget before failing Busy.
fn enable_wal(connection: &Connection, timeout: Duration) -> Result<()> {
    let label = |source| Error::storage("open: enable WAL", source);
    let deadline = Instant::now() + timeout;
    let mut pause = Duration::from_millis(1);
    let result = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        connection.busy_timeout(remaining).map_err(label)?;
        let (error, retryable) = match connection
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
        {
            Ok(mode) if mode == "wal" => break Ok(()),
            // SQLite reports the unchanged mode when it could not switch.
            Ok(mode) => (
                corrupt(format!("journal mode stayed {mode}, not wal")),
                true,
            ),
            Err(source) => {
                let error = label(source);
                let busy = error.code() == ErrorCode::Busy;
                (error, busy)
            }
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !retryable || remaining.is_zero() {
            break Err(error);
        }
        std::thread::sleep(pause.min(remaining));
        pause = (pause * 2).min(Duration::from_millis(50));
    };
    connection.busy_timeout(timeout).map_err(label)?;
    result
}

enum FileKind {
    Empty,
    /// A supported older format, upgraded by writable opens.
    Older(i32),
    Current,
}

// Classifies the file without changing it. Unrelated or unsupported files fail
// here, before any schema change.
fn file_kind(connection: &Connection) -> Result<FileKind> {
    let application: i32 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let user_objects: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*'",
        [],
        |row| row.get(0),
    )?;
    if application == 0 && version == 0 && user_objects == 0 {
        Ok(FileKind::Empty)
    } else if application == APPLICATION_ID && (FORMAT_V1..FORMAT_VERSION).contains(&version) {
        Ok(FileKind::Older(version))
    } else if application == APPLICATION_ID && version == FORMAT_VERSION {
        Ok(FileKind::Current)
    } else {
        Err(Error::new(
            ErrorCode::UnsupportedFormat,
            ErrorContext::InvalidInput { field: "format" },
            "file is not a supported Factlite database",
        ))
    }
}

fn initialize(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    transaction.execute_batch(DDL)?;
    transaction.execute_batch(&fact_events_table("fact_events"))?;
    transaction.execute_batch(&crate::index::ddl())?;
    transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
    transaction.pragma_update(None, "user_version", FORMAT_VERSION)?;
    let store = format!("s1.{}", uuid::Uuid::new_v4().simple());
    let lineage = format!("l1.{}", uuid::Uuid::new_v4().simple());
    transaction.execute(
        "INSERT INTO store_info(singleton,format,store_id,lineage_id,head) VALUES(1,?1,?2,?3,?4)",
        rusqlite::params![
            FORMAT_VERSION,
            store,
            lineage,
            revision_bytes(Revision::ZERO).as_slice()
        ],
    )?;
    Ok(())
}

fn validate_current(connection: &Connection) -> Result<()> {
    if table_names(connection)? != TABLES {
        return Err(corrupt("database tables do not match the supported format"));
    }
    validate_indexes(connection)?;
    validate_head(connection)
}

fn validate_indexes(connection: &Connection) -> Result<()> {
    let names: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='index' AND tbl_name='datoms' AND name NOT GLOB 'sqlite_*' ORDER BY name")?
        .query_map([], |row| row.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    if names != crate::index::INDEXES {
        return Err(corrupt("datom indexes do not match the supported format"));
    }
    Ok(())
}

// Upgrades an older file to the current format, one step at a time, inside
// the open's write transaction. Any failure rolls back and leaves the file
// unchanged; full validation of the result follows in the same transaction.
fn upgrade(transaction: &rusqlite::Transaction<'_>, from: i32) -> Result<()> {
    let format: i32 = transaction.query_row(
        "SELECT format FROM store_info WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    if format != from {
        return Err(corrupt("stored format differs from file format marker"));
    }
    let expected = if from == FORMAT_V1 {
        &TABLES_V1[..]
    } else {
        &TABLES[..]
    };
    if table_names(transaction)? != expected {
        return Err(corrupt("database tables do not match the supported format"));
    }
    for version in from..FORMAT_VERSION {
        match version {
            // Derive the datom intervals from retained fact events.
            FORMAT_V1 => {
                transaction.execute_batch(&crate::index::datoms_table("datoms"))?;
                transaction.execute_batch(crate::index::OPEN_DDL)?;
                crate::index::rebuild(transaction)?;
            }
            // Add the attribute/value index.
            FORMAT_V2 => transaction.execute_batch(crate::index::AVET_DDL)?,
            // Rebuild both fact tables so their type-tag checks admit keyword
            // and inst. SQLite cannot alter a CHECK constraint, and no table
            // references either of them, so copy, drop and rename is safe with
            // foreign keys enabled.
            FORMAT_V3 => {
                transaction.execute_batch(&format!(
                    "{}
INSERT INTO fact_events_v4(revision,ordinal,operation,entity,attribute,type_tag,value)
 SELECT revision,ordinal,operation,entity,attribute,type_tag,value FROM fact_events;
DROP TABLE fact_events;
ALTER TABLE fact_events_v4 RENAME TO fact_events;
{}
INSERT INTO datoms_v4(entity,attribute,type_tag,value,asserted,retracted)
 SELECT entity,attribute,type_tag,value,asserted,retracted FROM datoms;
DROP TABLE datoms;
ALTER TABLE datoms_v4 RENAME TO datoms;
{}
{}",
                    fact_events_table("fact_events_v4"),
                    crate::index::datoms_table("datoms_v4"),
                    crate::index::OPEN_DDL,
                    crate::index::AVET_DDL,
                ))?;
            }
            _ => unreachable!("supported older formats"),
        }
        let next = version + 1;
        transaction.execute("UPDATE store_info SET format=?1 WHERE singleton=1", [next])?;
        transaction.pragma_update(None, "user_version", next)?;
    }
    Ok(())
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

/// The format markers, table and index sets, and head row only.
fn validate_structure(connection: &Connection) -> Result<()> {
    check_markers(connection)?;
    if table_names(connection)? != TABLES {
        return Err(corrupt("database tables do not match the supported format"));
    }
    validate_indexes(connection)?;
    check_head_row(connection).map(|_| ())
}

fn check_markers(connection: &Connection) -> Result<()> {
    let application: i32 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if application != APPLICATION_ID || version != FORMAT_VERSION {
        return Err(Error::new(
            ErrorCode::UnsupportedFormat,
            ErrorContext::InvalidInput { field: "format" },
            "file is not a supported Factlite database",
        ));
    }
    Ok(())
}

pub(crate) fn validate_existing(connection: &Connection) -> Result<()> {
    check_markers(connection)?;
    validate_current(connection)
}

/// The head row is well formed and equals the latest retained transaction.
fn check_head_row(connection: &Connection) -> Result<SnapshotId> {
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
    Ok(basis)
}

fn validate_head(connection: &Connection) -> Result<()> {
    let basis = check_head_row(connection)?;
    crate::log::verify_foreign_keys(connection)?;
    let replayed = crate::log::replay(connection, &basis)?;
    crate::index::verify(connection)?;
    crate::index::verify_against(connection, &replayed)
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
