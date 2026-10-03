use std::collections::{BTreeMap, BTreeSet};

use crate::{AttrName, AttributeDefinition, EntityId, Error, Result, SnapshotId, TempId, Value};

/// An entity position in transaction input, before temporary allocation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum EntityTerm {
    /// Existing store-bound identity.
    Id(EntityId),
    /// Declared transaction-local entity.
    Temp(TempId),
}
impl From<EntityId> for EntityTerm {
    fn from(id: EntityId) -> Self {
        Self::Id(id)
    }
}
impl From<TempId> for EntityTerm {
    fn from(id: TempId) -> Self {
        Self::Temp(id)
    }
}

/// A typed value position, including forward temporary references.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ValueTerm {
    /// A concrete typed value, including allocated entity references.
    Value(Value),
    /// Reference to a declared temporary entity.
    TempRef(TempId),
}
impl From<Value> for ValueTerm {
    fn from(value: Value) -> Self {
        Self::Value(value)
    }
}

/// Explicit declarative fact operation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Operation {
    /// Remove this exact fact if present.
    Retract,
    /// Add this fact; cardinality one replaces the old value.
    Assert,
}

/// One submitted operation; its position remains part of retry identity.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Change {
    /// Operation kind.
    pub operation: Operation,
    /// Entity position.
    pub entity: EntityTerm,
    /// Qualified attribute name.
    pub attribute: AttrName,
    /// Typed value position.
    pub value: ValueTerm,
}

/// Exact typed set equality against the committed pre-transaction state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Expectation {
    /// Existing entity; temporary entities cannot be expected.
    pub entity: EntityId,
    /// Attribute that must already exist at pre-head.
    pub attribute: AttrName,
    /// Exact expected set, sorted/deduplicated during structural validation.
    pub values: Vec<Value>,
}

/// Owned transaction request. Construction never borrows or writes a database.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Tx {
    pub(crate) schema: Vec<AttributeDefinition>,
    pub(crate) new_entities: Vec<TempId>,
    pub(crate) changes: Vec<Change>,
    pub(crate) expectations: Vec<Expectation>,
    pub(crate) metadata: BTreeMap<String, Value>,
    pub(crate) expected_head: Option<SnapshotId>,
    pub(crate) idempotency_key: Option<String>,
}

impl Tx {
    /// Starts an owned request builder.
    pub fn builder() -> TxBuilder {
        TxBuilder::default()
    }
    /// Request encoding version.
    pub const fn version(&self) -> u32 {
        1
    }
    /// Submitted schema declarations, in submitted order.
    pub fn schema(&self) -> &[AttributeDefinition] {
        &self.schema
    }
    /// Temporary declarations, in submitted order.
    pub fn new_entities(&self) -> &[TempId] {
        &self.new_entities
    }
    /// Submitted operations, preserving duplicates and order.
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }
    /// Preconditions, with normalized typed sets.
    pub fn expectations(&self) -> &[Expectation] {
        &self.expectations
    }
    /// Caller metadata with exact scalar values.
    pub fn metadata(&self) -> &BTreeMap<String, Value> {
        &self.metadata
    }
    /// Optional exact-head fence.
    pub fn expected_head(&self) -> Option<&SnapshotId> {
        self.expected_head.as_ref()
    }
    /// Optional exact-retry key. Requires retry-capable commit implementation.
    pub fn idempotency_key(&self) -> Option<&str> {
        self.idempotency_key.as_deref()
    }

    pub(crate) fn validate(mut self) -> Result<Self> {
        let declared: BTreeSet<_> = self.new_entities.iter().collect();
        if declared.len() != self.new_entities.len() {
            return Err(Error::invalid(
                "new_entities",
                "temporary label declared more than once",
            ));
        }
        let attrs: BTreeSet<_> = self.schema.iter().map(AttributeDefinition::name).collect();
        if attrs.len() != self.schema.len() {
            return Err(Error::invalid(
                "schema",
                "attribute declared more than once",
            ));
        }
        for change in &self.changes {
            if let EntityTerm::Temp(label) = &change.entity {
                require_declared(&declared, label)?;
            }
            if let ValueTerm::TempRef(label) = &change.value {
                require_declared(&declared, label)?;
            }
        }
        for expectation in &mut self.expectations {
            expectation.values.sort_by(Value::canonical_cmp);
            expectation.values.dedup();
        }
        if self
            .metadata
            .values()
            .any(|value| matches!(value, Value::Ref(_)))
        {
            return Err(Error::invalid(
                "metadata",
                "metadata permits typed scalars but not references",
            ));
        }
        Ok(self)
    }
}

fn require_declared(declared: &BTreeSet<&TempId>, label: &TempId) -> Result<()> {
    if !declared.contains(label) {
        return Err(Error::invalid(
            "tempid",
            format!("undeclared temporary label: {label}"),
        ));
    }
    Ok(())
}

/// Fluent owned builder. `build` performs only schema-independent validation.
#[derive(Clone, Debug, Default)]
pub struct TxBuilder {
    request: Tx,
}

impl TxBuilder {
    /// Adds an immutable attribute definition.
    pub fn define_attribute(mut self, definition: AttributeDefinition) -> Self {
        self.request.schema.push(definition);
        self
    }
    /// Declares a new entity, including an empty entity.
    pub fn new_entity(mut self, label: TempId) -> Self {
        self.request.new_entities.push(label);
        self
    }
    /// Adds a declarative assertion.
    pub fn assert(
        self,
        entity: impl Into<EntityTerm>,
        attribute: AttrName,
        value: impl Into<ValueTerm>,
    ) -> Self {
        self.change(Operation::Assert, entity.into(), attribute, value.into())
    }
    /// Adds an exact declarative retraction.
    pub fn retract(
        self,
        entity: impl Into<EntityTerm>,
        attribute: AttrName,
        value: impl Into<ValueTerm>,
    ) -> Self {
        self.change(Operation::Retract, entity.into(), attribute, value.into())
    }
    fn change(
        mut self,
        operation: Operation,
        entity: EntityTerm,
        attribute: AttrName,
        value: ValueTerm,
    ) -> Self {
        self.request.changes.push(Change {
            operation,
            entity,
            attribute,
            value,
        });
        self
    }
    /// Adds exact pre-head set equality; empty means absence.
    pub fn expect(mut self, entity: EntityId, attribute: AttrName, values: Vec<Value>) -> Self {
        self.request.expectations.push(Expectation {
            entity,
            attribute,
            values,
        });
        self
    }
    /// Requires this exact head, including identity and revision.
    pub fn expect_head(mut self, snapshot: SnapshotId) -> Self {
        self.request.expected_head = Some(snapshot);
        self
    }
    /// Sets one metadata entry. A repeated key replaces its prior map entry.
    pub fn metadata(mut self, key: impl Into<String>, value: Value) -> Self {
        self.request.metadata.insert(key.into(), value);
        self
    }
    /// Sets the retry key; the key itself is excluded from canonical content.
    pub fn idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.request.idempotency_key = Some(key.into());
        self
    }
    /// Validates local structure and returns the owned request.
    pub fn build(self) -> Result<Tx> {
        self.request.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cardinality, ErrorCode, Uniqueness, ValueType};
    use proptest::prelude::*;

    fn entity() -> EntityId {
        "e1.00000000000000000000000000000001.0000000000000001"
            .parse()
            .unwrap()
    }
    fn attr() -> AttrName {
        "task/status".parse().unwrap()
    }

    #[test]
    fn structure_rejects_duplicates_undeclared_labels_and_reference_metadata() {
        let label = TempId::new("task").unwrap();
        assert!(
            Tx::builder()
                .new_entity(label.clone())
                .new_entity(label.clone())
                .build()
                .is_err()
        );
        assert!(
            Tx::builder()
                .assert(label.clone(), attr(), Value::String("todo".into()))
                .build()
                .is_err()
        );
        assert!(
            Tx::builder()
                .assert(entity(), attr(), ValueTerm::TempRef(label))
                .build()
                .is_err()
        );
        assert!(
            Tx::builder()
                .metadata("ref", Value::Ref(entity()))
                .build()
                .is_err()
        );
        let definition = AttributeDefinition::new(
            attr(),
            ValueType::String,
            Cardinality::One,
            Uniqueness::None,
        )
        .unwrap();
        assert_eq!(
            Tx::builder()
                .define_attribute(definition.clone())
                .define_attribute(definition)
                .build()
                .unwrap_err()
                .code(),
            ErrorCode::InvalidRequest
        );
    }

    #[test]
    fn cycles_forward_references_and_operation_order_are_preserved() {
        let a = TempId::new("a").unwrap();
        let b = TempId::new("b").unwrap();
        let request = Tx::builder()
            .assert(a.clone(), attr(), ValueTerm::TempRef(b.clone()))
            .assert(b.clone(), attr(), ValueTerm::TempRef(a.clone()))
            .new_entity(a.clone())
            .new_entity(b.clone())
            .build()
            .unwrap();
        assert_eq!(request.new_entities(), &[a, b]);
        assert_eq!(request.changes().len(), 2);
        // State-dependent failures must wait for commit, after retry lookup.
        let conflict = Tx::builder()
            .assert(entity(), attr(), Value::I64(1))
            .retract(entity(), attr(), Value::I64(1))
            .build()
            .unwrap();
        assert_eq!(conflict.changes()[0].operation, Operation::Assert);
        assert_eq!(conflict.changes()[1].operation, Operation::Retract);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        #[test]
        fn expectations_are_sets_but_operations_preserve_duplicates(values in proptest::collection::vec(any::<i64>(), 0..32)) {
            let input: Vec<_> = values.iter().copied().map(Value::I64).collect();
            let mut builder = Tx::builder().expect(entity(), attr(), input.clone());
            for value in &input { builder = builder.assert(entity(), attr(), value.clone()); }
            let request = builder.build().unwrap();
            prop_assert_eq!(request.changes().len(), input.len());
            let submitted: Vec<_> = request.changes().iter().map(|change| change.value.clone()).collect();
            prop_assert_eq!(submitted, input.into_iter().map(ValueTerm::Value).collect::<Vec<_>>());
            let expected: BTreeSet<_> = values.into_iter().collect();
            let actual: Vec<_> = request.expectations()[0].values.iter().map(|v| match v { Value::I64(n) => *n, _=>unreachable!() }).collect();
            prop_assert_eq!(actual, expected.into_iter().collect::<Vec<_>>());
        }
    }
}
