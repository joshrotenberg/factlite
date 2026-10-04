use std::collections::BTreeMap;

use rusqlite::{Transaction, TransactionBehavior};

use crate::{
    AttrName, Cardinality, EntityId, Error, ErrorCode, ErrorContext, Fact, Operation, Result,
    Revision, Schema, SnapshotId, Store, TxReport, Value, model::Model,
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

    /// Effective facts at this basis, in entity/attribute/typed-value order.
    /// Unknown or foreign entity restrictions match no facts.
    pub fn facts(&self, filter: FactFilter) -> Result<Vec<Fact>> {
        self.read(|_, model| {
            filter.validate(&model.catalog.schema_at(self.snapshot.revision())?)?;
            let mut facts: Vec<_> = model
                .facts
                .iter()
                .filter(|fact| filter.matches(fact))
                .cloned()
                .collect();
            facts.sort_by(Fact::canonical_cmp);
            Ok(facts)
        })
    }

    /// Project exactly the requested attributes; an empty list projects none.
    /// Unknown/not-yet-allocated/foreign entities return None. All requested
    /// attributes must exist in the pinned schema, even for an unknown entity.
    pub fn pull(&self, id: &EntityId, attrs: &[AttrName]) -> Result<Option<Entity>> {
        self.read(|_, model| {
            let schema = model.catalog.schema_at(self.snapshot.revision())?;
            for name in attrs {
                require_attribute(&schema, name)?;
            }
            if !model.catalog.known_at(id, self.snapshot.revision())? {
                return Ok(None);
            }
            let mut attributes = BTreeMap::new();
            for name in attrs {
                let mut values: Vec<_> = model
                    .facts
                    .iter()
                    .filter(|fact| &fact.entity == id && &fact.attribute == name)
                    .map(|fact| fact.value.clone())
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

    /// Effective events in (after, basis], in revision/ordinal order.
    pub fn history(&self, filter: FactFilter, after: Revision) -> Result<Vec<FactEvent>> {
        self.validate_after(after)?;
        self.read(|transaction, model| {
            filter.validate(&model.catalog.schema_at(self.snapshot.revision())?)?;
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
        self.read(|transaction, _| {
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
