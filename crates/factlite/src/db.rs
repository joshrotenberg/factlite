use rusqlite::{Transaction, TransactionBehavior};

use crate::{Result, Schema, SnapshotId, Store, model::Model};

/// Stable revision-pinned database value, owning a shared durable handle.
/// It never writes, refreshes implicitly, or retains a long-lived SQL transaction.
#[derive(Clone)]
pub struct Db {
    store: Store,
    snapshot: SnapshotId,
}

impl Db {
    pub(crate) fn pinned(store: Store, snapshot: SnapshotId) -> Self {
        Self { store, snapshot }
    }

    /// Returns this view's immutable full snapshot identity.
    pub fn snapshot(&self) -> &SnapshotId {
        &self.snapshot
    }

    /// Returns definitions visible at this view's revision, as owned data.
    pub fn schema(&self) -> Result<Schema> {
        self.read(|_, model| model.catalog.schema_at(self.snapshot.revision()))
    }

    // One operation, one SQLite snapshot. No result may borrow a row/cursor.
    pub(crate) fn read<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>, &Model) -> Result<T>,
    ) -> Result<T> {
        let mut connection = self.store.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let model = crate::log::replay(&transaction, &self.snapshot)?;
        let result = operation(&transaction, &model)?;
        transaction.commit()?;
        Ok(result)
    }
}
