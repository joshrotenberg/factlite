//! SQLite online backup from a fixed read snapshot, published without replacing files.
use crate::{
    Error, ErrorCode, ErrorContext, Result, SnapshotId, Store,
    store::{corrupt, read_head, validate_existing},
};
use rusqlite::{
    Connection, OpenFlags, TransactionBehavior,
    backup::{Backup, StepResult},
};
use std::{fs, path::Path, time::Duration};

fn require_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(Error::io(
            "backup destination",
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "backup destination or sidecar already exists",
            ),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::io("inspect backup destination", error)),
    }
}
impl Store {
    /// Back up one consistent committed head and all retained history using
    /// SQLite's backup API. Publish a validated, standalone file without replacing
    /// an existing path/sidecar. Returns the copied head token; the source can advance.
    ///
    /// Backups preserve store/lineage/IDs/reports and support Db::open_read_only.
    /// Resume writes only after retiring the old writer and verifying the backup
    /// includes identical complete committed history. Divergent writable clones
    /// and resuming an older backup after lost commits are unsupported.
    pub fn backup(&self, path: impl AsRef<Path>) -> Result<SnapshotId> {
        let path = path.as_ref();
        if path.as_os_str().is_empty() || path == Path::new(":memory:") {
            return Err(Error::invalid(
                "path",
                "backup requires a durable file path",
            ));
        }
        require_absent(path)?;
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = path.as_os_str().to_owned();
            sidecar.push(suffix);
            require_absent(Path::new(&sidecar))?;
        }
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let temporary = tempfile::Builder::new()
            .prefix(".factlite-backup-")
            .tempfile_in(parent)
            .map_err(|error| Error::io("create backup staging file", error))?;
        let mut destination = Connection::open_with_flags(
            temporary.path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )?;
        destination.busy_timeout(Duration::from_secs(2))?;
        destination.pragma_update(None, "synchronous", "FULL")?;
        destination.pragma_update(None, "foreign_keys", true)?;
        let basis = {
            let mut source = self.connection()?;
            let transaction = source.transaction_with_behavior(TransactionBehavior::Deferred)?;
            // Fix the source SQL snapshot before any independent writer can advance it.
            let basis = read_head(&transaction)?;
            validate_existing(&transaction)?;
            {
                let backup = Backup::new(&transaction, &mut destination)?;
                match backup.step(-1)? {
                    StepResult::Done => (),
                    StepResult::Busy | StepResult::Locked => {
                        return Err(Error::new(
                            ErrorCode::Busy,
                            ErrorContext::Storage {
                                operation: "sqlite backup",
                            },
                            "backup could not acquire SQLite locks",
                        ));
                    }
                    _ => return Err(corrupt("SQLite backup did not complete")),
                }
            }
            transaction.commit()?;
            basis
        };
        // A single-file artifact must not depend on a staged WAL sidecar.
        destination.pragma_update(None, "journal_mode", "DELETE")?;
        let mode: String =
            destination.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        if mode != "delete" {
            return Err(corrupt("backup could not become a standalone file"));
        }
        validate_existing(&destination)?;
        if read_head(&destination)? != basis {
            return Err(corrupt("backup does not match captured source snapshot"));
        }
        destination
            .close()
            .map_err(|(_, error)| Error::storage("close backup", error))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| Error::io("sync backup file", error))?;
        let published = temporary
            .persist_noclobber(path)
            .map_err(|error| Error::io("publish backup", error.error))?;
        drop(published);
        #[cfg(unix)]
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| Error::io("sync backup directory", error))?;
        Ok(basis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommitOutcome, Db, FactFilter, Revision, Tx, Value};
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    #[test]
    fn captured_backup_is_a_even_when_writer_commits_b_before_native_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.sqlite");
        let copy = dir.path().join("copy.sqlite");
        let store = Store::open(&path).unwrap();
        let a = store
            .commit(crate::wire::decode_tx(include_str!("../../../examples/create.json")).unwrap())
            .unwrap();
        let db_a = store.db().unwrap();
        let writer = Store::open(&path).unwrap();
        let mut pending = true;
        store
            .connection()
            .unwrap()
            .authorizer(Some(move |context: AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Read {
                        table_name: "transactions",
                        ..
                    }
                ) && pending
                {
                    pending = false;
                    writer
                        .commit(
                            Tx::builder()
                                .metadata("B", Value::Bool(true))
                                .build()
                                .unwrap(),
                        )
                        .unwrap();
                }
                Authorization::Allow
            }))
            .unwrap();
        assert_eq!(store.backup(&copy).unwrap(), a.after);
        store
            .connection()
            .unwrap()
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        let b = store.head().unwrap();
        assert_eq!(b.revision(), Revision::new(2));
        assert_eq!(
            Db::open_read_only_at(&copy, &b).err().unwrap().code(),
            ErrorCode::InvalidSnapshot
        );
        let copied = Db::open_read_only(&copy).unwrap();
        assert_eq!(*copied.snapshot(), a.after);
        assert_eq!(
            copied.facts(FactFilter::default()).unwrap(),
            db_a.facts(FactFilter::default()).unwrap()
        );
        assert_eq!(copied.transactions(Revision::ZERO).unwrap().len(), 1);
    }
    #[test]
    fn failure_cleans_staging_and_publication_race_preserves_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.sqlite");
        let destination = dir.path().join("copy.sqlite");
        let store = Store::open(&path).unwrap();
        store.commit(Tx::default()).unwrap();
        let head = store.head().unwrap();
        store
            .connection()
            .unwrap()
            .authorizer(Some(|context: AuthContext<'_>| match context.action {
                AuthAction::Read {
                    table_name: "transactions",
                    ..
                } => Authorization::Deny,
                _ => Authorization::Allow,
            }))
            .unwrap();
        assert_eq!(
            store.backup(&destination).unwrap_err().code(),
            ErrorCode::Storage
        );
        assert!(!destination.exists());
        store
            .connection()
            .unwrap()
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        let captured = destination.clone();
        let mut pending = true;
        store
            .connection()
            .unwrap()
            .authorizer(Some(move |context: AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Read {
                        table_name: "transactions",
                        ..
                    }
                ) && pending
                {
                    pending = false;
                    fs::write(&captured, b"raced-existing").unwrap();
                }
                Authorization::Allow
            }))
            .unwrap();
        let error = store.backup(&destination).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Storage);
        assert_eq!(error.commit_outcome(), None::<CommitOutcome>);
        store
            .connection()
            .unwrap()
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"raced-existing");
        assert_eq!(store.head().unwrap(), head);
        assert!(fs::read_dir(dir.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".factlite-backup-")
        }));
    }
}
