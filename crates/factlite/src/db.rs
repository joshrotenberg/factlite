use std::collections::BTreeMap;

use rusqlite::{Transaction, TransactionBehavior};

use crate::{
    AttrName, Cardinality, EntityId, Error, ErrorCode, ErrorContext, Fact, Operation, Result,
    Revision, Schema, SnapshotId, Store, TxReport, Value,
};

/// Exact optional restrictions, combined conjunctively. Missing fields match all.
/// An attribute restriction must exist in the pinned schema. When both attribute
/// and value are supplied, their types must agree.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FactFilter {
    /// Match only this allocated identity; unknown identities match nothing.
    pub entity: Option<EntityId>,
    /// Match only this historically defined attribute.
    pub attribute: Option<AttrName>,
    /// Match this exact typed value, without numeric coercion.
    pub value: Option<Value>,
}

impl FactFilter {
    fn validate(&self, schema: &Schema) -> Result<()> {
        if let Some(attribute) = &self.attribute {
            let definition = require_attribute(schema, attribute)?;
            if let Some(value) = &self.value
                && value.value_type() != definition.value_type()
            {
                return Err(Error::new(
                    ErrorCode::TypeMismatch,
                    ErrorContext::TypeMismatch {
                        expected: definition.value_type(),
                        actual: value.value_type(),
                    },
                    "filter value has the wrong type for its attribute",
                ));
            }
        }
        Ok(())
    }

    fn matches(&self, fact: &Fact) -> bool {
        self.entity.as_ref().is_none_or(|id| id == &fact.entity)
            && self
                .attribute
                .as_ref()
                .is_none_or(|name| name == &fact.attribute)
            && self.value.as_ref().is_none_or(|value| value == &fact.value)
    }
}

/// A present field preserves schema cardinality; absent fields are omitted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PulledValue {
    /// A present cardinality-one field.
    One(Value),
    /// A present cardinality-many field, ordered by same-type value comparison.
    Many(Vec<Value>),
}

/// Owned, flat projection. Reference values remain IDs, without traversal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entity {
    /// The known allocated identity, even when no selected fields are present.
    pub id: EntityId,
    /// Present selected fields, ordered by qualified name.
    pub attributes: BTreeMap<AttrName, PulledValue>,
}

/// Effective event with its original ordinal, even when other events are filtered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactEvent {
    /// Committed revision containing this event.
    pub revision: Revision,
    /// Zero-based position in the complete effective delta.
    pub ordinal: u64,
    /// Retraction or assertion.
    pub operation: Operation,
    /// Resolved typed fact.
    pub fact: Fact,
}

/// Retained committed record, including empty deltas and caller metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TxRecord {
    /// Complete original committed report, including caller metadata.
    pub report: TxReport,
}

impl TxRecord {
    /// The committed revision, taken from the report's after snapshot.
    pub fn revision(&self) -> Revision {
        self.report.after.revision()
    }
}

fn require_attribute<'a>(
    schema: &'a Schema,
    name: &AttrName,
) -> Result<&'a crate::AttributeDefinition> {
    schema.get(name).ok_or_else(|| {
        Error::new(
            ErrorCode::UnknownAttribute,
            ErrorContext::Attribute {
                attribute: name.clone(),
            },
            "attribute is not defined at this snapshot",
        )
    })
}

/// Stable revision-pinned database value, owning a shared durable handle.
/// It never commits durable changes, refreshes implicitly, or retains a long-lived
/// SQL transaction. Query may build a temporary relation inside a read operation.
#[derive(Clone)]
pub struct Db {
    store: Store,
    snapshot: SnapshotId,
}

impl Db {
    /// Open an existing file through a read-only connection and pin its head.
    /// This cannot initialize or commit main-database changes; queries may use
    /// connection-local temporary tables. Backups preserve original tokens.
    pub fn open_read_only(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Store::reader(path.as_ref(), &crate::StoreOptions::default())
    }

    /// Like [`Db::open_read_only`], with explicit connection settings.
    pub fn open_read_only_with(
        path: impl AsRef<std::path::Path>,
        options: &crate::StoreOptions,
    ) -> Result<Self> {
        Store::reader(path.as_ref(), options)
    }

    /// Fully validates the latest committed state over this view's connection
    /// and returns the validated head; see [`Store::verify`]. This view stays
    /// pinned to its own revision.
    pub fn verify(&self) -> Result<SnapshotId> {
        self.store.verify()
    }

    /// A new view of the latest committed head over this view's connection.
    /// This view stays pinned; a read-only view refreshes only explicitly,
    /// without reopening or revalidating the file.
    pub fn at_head(&self) -> Result<Self> {
        self.store.db()
    }

    /// A new view of a retained snapshot over this view's connection, validated
    /// like [`Store::db_at`].
    pub fn at(&self, snapshot: &SnapshotId) -> Result<Self> {
        self.store.db_at(snapshot)
    }

    /// Open a retained full snapshot through a read-only file connection.
    /// Wrong identities, future bounds and unavailable history fail normally.
    pub fn open_read_only_at(
        path: impl AsRef<std::path::Path>,
        snapshot: &SnapshotId,
    ) -> Result<Self> {
        let reader = Store::reader(path.as_ref(), &crate::StoreOptions::default())?;
        reader.store.db_at(snapshot)
    }

    pub(crate) fn pinned(store: Store, snapshot: SnapshotId) -> Self {
        Self { store, snapshot }
    }

    /// Returns this view's immutable full snapshot identity.
    pub fn snapshot(&self) -> &SnapshotId {
        &self.snapshot
    }

    /// Returns definitions visible at this view's revision, as owned data.
    pub fn schema(&self) -> Result<Schema> {
        self.read_basis(|transaction| crate::index::schema_at(transaction, &self.snapshot))
    }

    /// Effective facts at this basis, in entity/attribute/typed-value order.
    /// Unknown or foreign entity restrictions match no facts.
    pub fn facts(&self, filter: FactFilter) -> Result<Vec<Fact>> {
        self.read_basis(|transaction| {
            filter.validate(&crate::index::schema_at(transaction, &self.snapshot)?)?;
            let mut facts = crate::index::facts_matching(
                transaction,
                &self.snapshot,
                filter.entity.as_ref(),
                filter.attribute.as_ref(),
                filter.value.as_ref(),
            )?;
            facts.sort_by(Fact::canonical_cmp);
            Ok(facts)
        })
    }

    /// Project exactly the requested attributes; an empty list projects none.
    /// Unknown/not-yet-allocated/foreign entities return None. All requested
    /// attributes must exist in the pinned schema, even for an unknown entity.
    pub fn pull(&self, id: &EntityId, attrs: &[AttrName]) -> Result<Option<Entity>> {
        self.read_basis(|transaction| {
            let schema = crate::index::schema_at(transaction, &self.snapshot)?;
            for name in attrs {
                require_attribute(&schema, name)?;
            }
            if !crate::index::known_at(transaction, &self.snapshot, id)? {
                return Ok(None);
            }
            let mut attributes = BTreeMap::new();
            for name in attrs {
                let mut values: Vec<_> = crate::index::facts_matching(
                    transaction,
                    &self.snapshot,
                    Some(id),
                    Some(name),
                    None,
                )?
                .into_iter()
                .map(|fact| fact.value)
                .collect();
                values.sort_by(Value::canonical_cmp);
                if values.is_empty() {
                    continue;
                }
                let definition = require_attribute(&schema, name)?;
                let value = match definition.cardinality() {
                    Cardinality::One => PulledValue::One(values.remove(0)),
                    Cardinality::Many => PulledValue::Many(values),
                };
                attributes.insert(name.clone(), value);
            }
            Ok(Some(Entity {
                id: id.clone(),
                attributes,
            }))
        })
    }

    /// The entity whose `attribute` has exactly `value` at this basis, if any.
    /// The attribute must be visible and `unique_one`, and the value must have
    /// its type, so at most one entity can match. A lookup never creates or
    /// merges entities.
    pub fn lookup(&self, attribute: &AttrName, value: &Value) -> Result<Option<EntityId>> {
        self.read_basis(|transaction| {
            let schema = crate::index::schema_at(transaction, &self.snapshot)?;
            let definition = require_attribute(&schema, attribute)?;
            if definition.uniqueness() != crate::Uniqueness::UniqueOne {
                return Err(Error::new(
                    ErrorCode::InvalidRequest,
                    ErrorContext::Attribute {
                        attribute: attribute.clone(),
                    },
                    "lookup attribute must be unique_one",
                ));
            }
            if definition.value_type() != value.value_type() {
                return Err(Error::new(
                    ErrorCode::TypeMismatch,
                    ErrorContext::TypeMismatch {
                        expected: definition.value_type(),
                        actual: value.value_type(),
                    },
                    "lookup value has the wrong type for its attribute",
                ));
            }
            let mut owners = crate::index::facts_matching(
                transaction,
                &self.snapshot,
                None,
                Some(attribute),
                Some(value),
            )?;
            if owners.len() > 1 {
                return Err(crate::store::corrupt("unique value has several owners"));
            }
            Ok(owners.pop().map(|fact| fact.entity))
        })
    }

    /// Effective events in (after, basis], in revision/ordinal order.
    /// Filters naming an entity or attribute read datom intervals; others
    /// read every retained report in the range.
    pub fn history(&self, filter: FactFilter, after: Revision) -> Result<Vec<FactEvent>> {
        if filter.entity.is_some() || filter.attribute.is_some() {
            self.history_from_index(filter, after)
        } else {
            self.history_from_log(filter, after)
        }
    }

    pub(crate) fn history_from_index(
        &self,
        filter: FactFilter,
        after: Revision,
    ) -> Result<Vec<FactEvent>> {
        self.validate_after(after)?;
        self.read_basis(|transaction| {
            filter.validate(&crate::index::schema_at(transaction, &self.snapshot)?)?;
            crate::index::history_matching(
                transaction,
                &self.snapshot,
                after,
                filter.entity.as_ref(),
                filter.attribute.as_ref(),
                filter.value.as_ref(),
            )
        })
    }

    pub(crate) fn history_from_log(
        &self,
        filter: FactFilter,
        after: Revision,
    ) -> Result<Vec<FactEvent>> {
        self.validate_after(after)?;
        self.read_basis(|transaction| {
            filter.validate(&crate::index::schema_at(transaction, &self.snapshot)?)?;
            let mut events = Vec::new();
            for report in crate::log::reports_between(transaction, after, self.snapshot.revision())?
            {
                let revision = report.after.revision();
                for (ordinal, delta) in report.effective_delta.into_iter().enumerate() {
                    if filter.matches(&delta.fact) {
                        events.push(FactEvent {
                            revision,
                            ordinal: ordinal as u64,
                            operation: delta.operation,
                            fact: delta.fact,
                        });
                    }
                }
            }
            Ok(events)
        })
    }

    /// All commits in (after, basis], including no-change commits and metadata.
    pub fn transactions(&self, after: Revision) -> Result<Vec<TxRecord>> {
        self.validate_after(after)?;
        self.read_basis(|transaction| {
            Ok(
                crate::log::reports_between(transaction, after, self.snapshot.revision())?
                    .into_iter()
                    .map(|report| TxRecord { report })
                    .collect(),
            )
        })
    }

    fn validate_after(&self, after: Revision) -> Result<()> {
        if after > self.snapshot.revision() {
            return Err(Error::invalid(
                "after",
                "lower bound exceeds this snapshot revision",
            ));
        }
        Ok(())
    }

    // One operation, one SQLite snapshot. No result may borrow a row/cursor.
    // Reads check their basis and then query revision-bounded rows directly.
    pub(crate) fn read_basis<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut connection = self.store.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        crate::index::check_basis(&transaction, &self.snapshot)?;
        let result = operation(&transaction)?;
        transaction.commit()?;
        Ok(result)
    }

    // Loads the complete state at this basis; for tests and the reference
    // query fallback only.
    #[cfg(test)]
    pub(crate) fn read<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>, &crate::model::Model) -> Result<T>,
    ) -> Result<T> {
        let mut connection = self.store.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let model = crate::index::model_at(&transaction, &self.snapshot)?;
        let result = operation(&transaction, &model)?;
        transaction.commit()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_only_view_has_no_writable_connection_or_commit_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("read.sqlite");
        let writer = Store::open(&path).unwrap();
        let before = writer.head().unwrap();
        let reader = Db::open_read_only(&path).unwrap();
        let error = reader
            .store
            .connection()
            .unwrap()
            .execute("DELETE FROM store_info", [])
            .unwrap_err();
        assert!(
            matches!(error,rusqlite::Error::SqliteFailure(ref code,_) if code.code == rusqlite::ErrorCode::ReadOnly)
        );
        let error = reader.store.commit(crate::Tx::default()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Storage);
        assert_eq!(
            error.commit_outcome(),
            Some(crate::CommitOutcome::RolledBack)
        );
        assert_eq!(*reader.snapshot(), before);
        assert_eq!(writer.head().unwrap(), before);
        assert_eq!(
            reader
                .query(&crate::Query::default(), &crate::Params::new())
                .unwrap()
                .rows,
            vec![vec![]]
        );
    }
}
