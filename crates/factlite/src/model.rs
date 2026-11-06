use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
};

use crate::catalog::{Catalog, CatalogDelta};
use crate::{
    AttrName, Cardinality, EntityId, EntityTerm, Error, ErrorCode, ErrorContext, Operation, Result,
    SnapshotId, TempId, Tx, Uniqueness, Value, ValueTerm,
};

/// An effective typed entity/attribute/value fact.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Fact {
    /// Allocated store-bound identity.
    pub entity: EntityId,
    /// Qualified attribute name.
    pub attribute: AttrName,
    /// Exact typed value.
    pub value: Value,
}

impl Fact {
    pub(crate) fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.entity
            .cmp(&other.entity)
            .then_with(|| self.attribute.cmp(&other.attribute))
            .then_with(|| self.value.canonical_cmp(&other.value))
    }
}

/// One effective change, not a submitted no-op or duplicate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactDelta {
    /// Retraction or assertion.
    pub operation: Operation,
    /// Resolved fact containing no temporary labels.
    pub fact: Fact,
}

/// Complete effective outcome, persisted atomically by the store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TxReport {
    /// Committed pre-head.
    pub before: SnapshotId,
    /// New committed revision; exact replay later returns this original token.
    pub after: SnapshotId,
    /// Temporary labels resolved to allocated store-bound IDs.
    pub tempids: BTreeMap<TempId, EntityId>,
    /// Newly allocated identities, including empty entities, in identity order.
    pub created_entities: Vec<EntityId>,
    /// Effective schema additions in qualified-name order.
    pub schema_added: Vec<crate::AttributeDefinition>,
    /// Effective changes in deterministic event order.
    pub effective_delta: Vec<FactDelta>,
    /// Exact caller scalar metadata.
    pub metadata: BTreeMap<String, Value>,
}

impl TxReport {
    /// Report encoding version.
    pub const fn version(&self) -> u32 {
        1
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Model {
    pub catalog: Catalog,
    facts: HashSet<Fact>,
    // Built on the first transition and maintained by every fact change, so a
    // transition only visits the facts it touches. Reads never build it.
    indexes: Option<Indexes>,
}

/// Value sets per entity/attribute, and owners per unique attribute/value.
#[derive(Clone, Debug, Default)]
struct Indexes {
    values: HashMap<(EntityId, AttrName), HashSet<Value>>,
    owners: HashMap<(AttrName, Value), BTreeSet<EntityId>>,
}

impl Indexes {
    fn add(&mut self, fact: &Fact, unique: bool) {
        self.values
            .entry((fact.entity.clone(), fact.attribute.clone()))
            .or_default()
            .insert(fact.value.clone());
        if unique {
            self.owners
                .entry((fact.attribute.clone(), fact.value.clone()))
                .or_default()
                .insert(fact.entity.clone());
        }
    }

    fn remove(&mut self, fact: &Fact, unique: bool) {
        let key = (fact.entity.clone(), fact.attribute.clone());
        if let Some(values) = self.values.get_mut(&key) {
            values.remove(&fact.value);
            if values.is_empty() {
                self.values.remove(&key);
            }
        }
        if unique {
            let key = (fact.attribute.clone(), fact.value.clone());
            if let Some(owners) = self.owners.get_mut(&key) {
                owners.remove(&fact.entity);
                if owners.is_empty() {
                    self.owners.remove(&key);
                }
            }
        }
    }
}

// Test-only: batch normalization without final-state checks.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct NormalizedBatch {
    pub next: Model,
    pub catalog_delta: CatalogDelta,
    pub effective_delta: Vec<FactDelta>,
}

/// Facts touched by one transition, with whether each was visible before it.
/// Only touched facts can differ between the states before and after, so the
/// effective delta is computed from them alone.
#[derive(Default)]
struct Touched(HashMap<Fact, bool>);

impl Touched {
    fn note(&mut self, model: &Model, fact: &Fact) {
        if !self.0.contains_key(fact) {
            self.0.insert(fact.clone(), model.facts.contains(fact));
        }
    }

    fn effective(self, model: &Model) -> Vec<FactDelta> {
        let mut delta: Vec<_> = self
            .0
            .into_iter()
            .filter_map(
                |(fact, before)| match (before, model.facts.contains(&fact)) {
                    (true, false) => Some(FactDelta {
                        operation: Operation::Retract,
                        fact,
                    }),
                    (false, true) => Some(FactDelta {
                        operation: Operation::Assert,
                        fact,
                    }),
                    _ => None,
                },
            )
            .collect();
        delta.sort_by(|a, b| {
            a.operation
                .cmp(&b.operation)
                .then_with(|| a.fact.canonical_cmp(&b.fact))
        });
        delta
    }
}

// Transitions are incremental. They assume, and preserve, a valid pre-state:
// known identities and attributes, schema types, cardinality one and unique
// ownership. Entities and definitions are never removed, so facts a transition
// does not touch stay valid; only asserted facts can break cardinality, typing
// or uniqueness. The empty model is valid, so every replayed state is too.
// `model_reference.rs` keeps the former full-state checks for differential
// tests.
impl Model {
    pub fn new(catalog: Catalog, facts: HashSet<Fact>) -> Self {
        Self {
            catalog,
            facts,
            indexes: None,
        }
    }

    pub fn facts(&self) -> &HashSet<Fact> {
        &self.facts
    }

    /// Replaces all facts, for tests that construct states directly.
    #[cfg(test)]
    pub fn set_facts(&mut self, facts: HashSet<Fact>) {
        self.facts = facts;
        self.indexes = None;
    }

    fn unique(&self, attribute: &AttrName) -> bool {
        self.catalog
            .schema
            .get(attribute)
            .is_some_and(|(definition, _)| definition.uniqueness() == Uniqueness::UniqueOne)
    }

    fn indexes(&mut self) -> &Indexes {
        if self.indexes.is_none() {
            let mut indexes = Indexes::default();
            for fact in &self.facts {
                indexes.add(fact, self.unique(&fact.attribute));
            }
            self.indexes = Some(indexes);
        }
        self.indexes.as_ref().expect("indexes were just built")
    }

    fn insert_fact(&mut self, fact: Fact) -> bool {
        if self.facts.contains(&fact) {
            return false;
        }
        let unique = self.unique(&fact.attribute);
        if let Some(indexes) = &mut self.indexes {
            indexes.add(&fact, unique);
        }
        self.facts.insert(fact)
    }

    fn remove_fact(&mut self, fact: &Fact) -> bool {
        if !self.facts.remove(fact) {
            return false;
        }
        let unique = self.unique(&fact.attribute);
        if let Some(indexes) = &mut self.indexes {
            indexes.remove(fact, unique);
        }
        true
    }

    /// Current values of one entity/attribute, in canonical order.
    fn values(&mut self, entity: &EntityId, attribute: &AttrName) -> Vec<Value> {
        let mut values: Vec<_> = self
            .indexes()
            .values
            .get(&(entity.clone(), attribute.clone()))
            .map(|values| values.iter().cloned().collect())
            .unwrap_or_default();
        values.sort_by(Value::canonical_cmp);
        values
    }

    #[cfg(test)]
    pub fn apply_report(&self, report: &TxReport) -> Result<Self> {
        self.clone().apply_report_owned(report)
    }

    /// Applies and validates one retained report in place.
    pub fn apply_report_owned(mut self, report: &TxReport) -> Result<Self> {
        use crate::store::corrupt;
        let expected_after = SnapshotId::new(
            self.catalog.snapshot.store().clone(),
            self.catalog.snapshot.lineage().clone(),
            self.catalog.snapshot.revision().next()?,
        );
        if report.before != self.catalog.snapshot || report.after != expected_after {
            return Err(corrupt(
                "transaction reports are not a contiguous retained lineage",
            ));
        }
        let mut sorted = report.created_entities.clone();
        sorted.sort();
        sorted.dedup();
        if sorted != report.created_entities || report.tempids.len() != sorted.len() {
            return Err(corrupt("invalid report allocation list"));
        }
        let mut labels = BTreeMap::new();
        for (label, id) in &report.tempids {
            if labels.insert(id.clone(), label.clone()).is_some() {
                return Err(corrupt("multiple labels alias an allocation"));
            }
        }
        let labels: Vec<_> = sorted
            .iter()
            .map(|id| {
                labels
                    .get(id)
                    .cloned()
                    .ok_or_else(|| corrupt("report allocation mapping mismatch"))
            })
            .collect::<Result<_>>()?;
        self.indexes();
        let delta = self
            .catalog
            .stage_in_place(&report.schema_added, &labels)
            .map_err(|err| corrupt(err.to_string()))?;
        if delta.schema_added != report.schema_added || delta.tempids != report.tempids {
            return Err(corrupt(
                "report declarations do not match catalog allocations",
            ));
        }
        let mut touched = Touched::default();
        let mut asserted = Vec::new();
        for event in &report.effective_delta {
            touched.note(&self, &event.fact);
            let effective = match event.operation {
                Operation::Retract => self.remove_fact(&event.fact),
                Operation::Assert => {
                    asserted.push(event.fact.clone());
                    self.insert_fact(event.fact.clone())
                }
            };
            if !effective {
                return Err(corrupt("fact log contains an ineffective event"));
            }
        }
        if touched.effective(&self) != report.effective_delta {
            return Err(corrupt(
                "fact events are not canonical effective differences",
            ));
        }
        self.validate_asserted(&asserted)
            .map_err(|err| corrupt(err.to_string()))?;
        Ok(self)
    }

    // The full-state invariants, checked only where this transition asserted.
    fn validate_asserted(&mut self, asserted: &[Fact]) -> Result<()> {
        let schema = self.catalog.schema_at(self.catalog.snapshot.revision())?;
        for fact in asserted {
            self.catalog.require_known(&fact.entity)?;
            if let Value::Ref(id) = &fact.value {
                self.catalog.require_known(id)?;
            }
            let definition = schema.get(&fact.attribute).ok_or_else(|| {
                Error::new(
                    ErrorCode::UnknownAttribute,
                    ErrorContext::Attribute {
                        attribute: fact.attribute.clone(),
                    },
                    "stored fact has unknown attribute",
                )
            })?;
            if definition.value_type() != fact.value.value_type() {
                return Err(Error::new(
                    ErrorCode::TypeMismatch,
                    ErrorContext::TypeMismatch {
                        expected: definition.value_type(),
                        actual: fact.value.value_type(),
                    },
                    "stored fact has wrong type",
                ));
            }
            if definition.cardinality() == Cardinality::One
                && self.values(&fact.entity, &fact.attribute).len() > 1
            {
                return Err(Error::new(
                    ErrorCode::ConflictingBatch,
                    ErrorContext::EntityAttribute {
                        entity: fact.entity.clone(),
                        attribute: fact.attribute.clone(),
                    },
                    "stored state violates cardinality one",
                ));
            }
        }
        self.validate_uniqueness(asserted)
    }

    #[cfg(test)]
    pub fn evaluate(&self, tx: &Tx) -> Result<(Self, TxReport)> {
        self.clone().evaluate_owned(tx)
    }

    /// Evaluates one request against this state in place.
    pub fn evaluate_owned(mut self, tx: &Tx) -> Result<(Self, TxReport)> {
        let tx = tx.clone().validate()?;
        if let Some(expected) = &tx.expected_head
            && expected != &self.catalog.snapshot
        {
            return Err(Error::new(
                ErrorCode::HeadMismatch,
                ErrorContext::HeadMismatch {
                    expected: expected.clone(),
                    actual: self.catalog.snapshot.clone(),
                },
                "expected head differs from current head",
            ));
        }
        let schema = self.catalog.schema_at(self.catalog.snapshot.revision())?;
        for expectation in &tx.expectations {
            self.catalog.require_known(&expectation.entity)?;
            let definition = schema.get(&expectation.attribute).ok_or_else(|| {
                Error::new(
                    ErrorCode::UnknownAttribute,
                    ErrorContext::Attribute {
                        attribute: expectation.attribute.clone(),
                    },
                    "expectation attribute is unknown at pre-head",
                )
            })?;
            for value in &expectation.values {
                if value.value_type() != definition.value_type() {
                    return Err(Error::new(
                        ErrorCode::TypeMismatch,
                        ErrorContext::TypeMismatch {
                            expected: definition.value_type(),
                            actual: value.value_type(),
                        },
                        "expectation value has the wrong type",
                    ));
                }
                if let Value::Ref(id) = value {
                    self.catalog.require_known(id)?;
                }
            }
            if definition.cardinality() == Cardinality::One && expectation.values.len() > 1 {
                return Err(Error::invalid(
                    "expect",
                    "cardinality-one expectation contains more than one distinct value",
                ));
            }
            let actual = self.values(&expectation.entity, &expectation.attribute);
            if actual != expectation.values {
                return Err(Error::new(
                    ErrorCode::PreconditionFailed,
                    ErrorContext::Precondition {
                        entity: expectation.entity.clone(),
                        attribute: expectation.attribute.clone(),
                        expected: expectation.values.clone(),
                        actual,
                    },
                    "exact attribute-set expectation failed",
                ));
            }
        }
        let before = self.catalog.snapshot.clone();
        let (catalog_delta, effective_delta, asserted) = self.normalize_in_place(&tx)?;
        self.validate_uniqueness(&asserted)?;
        let mut created_entities: Vec<_> = catalog_delta.tempids.values().cloned().collect();
        created_entities.sort();
        let report = TxReport {
            before,
            after: self.catalog.snapshot.clone(),
            tempids: catalog_delta.tempids,
            created_entities,
            schema_added: catalog_delta.schema_added,
            effective_delta,
            metadata: tx.metadata,
        };
        Ok((self, report))
    }

    // Before this transition no unique value had two owners, and only an
    // assertion can add an owner, so only asserted values need checking. The
    // reported violation is the least attribute, then least value, as a
    // full-state scan in attribute/value order would choose.
    fn validate_uniqueness(&mut self, asserted: &[Fact]) -> Result<()> {
        self.indexes();
        let mut violation: Option<(&AttrName, &Value, &BTreeSet<EntityId>)> = None;
        let indexes = self.indexes.as_ref().expect("indexes were just built");
        for fact in asserted {
            if !self.unique(&fact.attribute) {
                continue;
            }
            let Some(owners) = indexes
                .owners
                .get(&(fact.attribute.clone(), fact.value.clone()))
            else {
                continue;
            };
            if owners.len() < 2 {
                continue;
            }
            let lesser = violation.is_none_or(|(attribute, value, _)| {
                (&fact.attribute, &fact.value) != (attribute, value)
                    && fact
                        .attribute
                        .cmp(attribute)
                        .then_with(|| fact.value.canonical_cmp(value))
                        .is_lt()
            });
            if lesser {
                violation = Some((&fact.attribute, &fact.value, owners));
            }
        }
        if let Some((attribute, value, owners)) = violation {
            return Err(Error::new(
                ErrorCode::UniqueViolation,
                ErrorContext::UniqueOwnership {
                    attribute: attribute.clone(),
                    value: value.clone(),
                    owners: owners.iter().cloned().collect(),
                },
                "unique value has multiple final-state owners",
            ));
        }
        Ok(())
    }

    pub fn empty(snapshot: SnapshotId) -> Result<Self> {
        Ok(Self::new(Catalog::empty(snapshot)?, HashSet::new()))
    }

    // Batch normalization is private; evaluate adds preconditions and final
    // uniqueness before storage can expose a report.
    #[cfg(test)]
    pub fn normalize(&self, tx: &Tx) -> Result<NormalizedBatch> {
        let mut next = self.clone();
        let (catalog_delta, effective_delta, _) = next.normalize_in_place(tx)?;
        Ok(NormalizedBatch {
            next,
            catalog_delta,
            effective_delta,
        })
    }

    /// Stages the batch into this state. Returns the catalog delta, effective
    /// delta, and the asserted facts for final-state checks.
    fn normalize_in_place(&mut self, tx: &Tx) -> Result<(CatalogDelta, Vec<FactDelta>, Vec<Fact>)> {
        self.indexes();
        let pre = self.catalog.snapshot.revision();
        let catalog_delta = self.catalog.stage_in_place(&tx.schema, &tx.new_entities)?;
        let schema = self.catalog.schema_at(self.catalog.snapshot.revision())?;
        let mut assertions = HashSet::new();
        let mut retractions = HashSet::new();
        for change in &tx.changes {
            let entity = match &change.entity {
                EntityTerm::Id(id) => {
                    self.catalog.require_known_at(id, pre)?;
                    id.clone()
                }
                EntityTerm::Temp(label) => catalog_delta
                    .tempids
                    .get(label)
                    .cloned()
                    .ok_or_else(|| Error::invalid("tempid", "undeclared temporary entity"))?,
            };
            let value = match &change.value {
                ValueTerm::Value(value) => {
                    if let Value::Ref(id) = value {
                        self.catalog.require_known_at(id, pre)?;
                    }
                    value.clone()
                }
                ValueTerm::TempRef(label) => {
                    Value::Ref(catalog_delta.tempids.get(label).cloned().ok_or_else(|| {
                        Error::invalid("tempid", "undeclared temporary reference")
                    })?)
                }
            };
            let definition = schema.get(&change.attribute).ok_or_else(|| {
                Error::new(
                    ErrorCode::UnknownAttribute,
                    ErrorContext::Attribute {
                        attribute: change.attribute.clone(),
                    },
                    "attribute is not visible in the applicable schema",
                )
            })?;
            if definition.value_type() != value.value_type() {
                return Err(Error::new(
                    ErrorCode::TypeMismatch,
                    ErrorContext::TypeMismatch {
                        expected: definition.value_type(),
                        actual: value.value_type(),
                    },
                    "fact value has the wrong attribute type",
                ));
            }
            let fact = Fact {
                entity,
                attribute: change.attribute.clone(),
                value,
            };
            match change.operation {
                Operation::Assert => {
                    assertions.insert(fact);
                }
                Operation::Retract => {
                    retractions.insert(fact);
                }
            }
        }
        if let Some(conflict) = assertions
            .intersection(&retractions)
            .min_by(|a, b| a.canonical_cmp(b))
        {
            return Err(Error::new(
                ErrorCode::ConflictingBatch,
                ErrorContext::Fact {
                    fact: conflict.clone(),
                },
                "same fact asserted and retracted in one batch",
            ));
        }
        let mut one_values: BTreeMap<(EntityId, AttrName), HashSet<Value>> = BTreeMap::new();
        for fact in &assertions {
            if schema
                .get(&fact.attribute)
                .expect("resolved attribute")
                .cardinality()
                == Cardinality::One
            {
                one_values
                    .entry((fact.entity.clone(), fact.attribute.clone()))
                    .or_default()
                    .insert(fact.value.clone());
            }
        }
        for ((entity, attribute), values) in &one_values {
            if values.len() > 1 {
                return Err(Error::new(
                    ErrorCode::ConflictingBatch,
                    ErrorContext::EntityAttribute {
                        entity: entity.clone(),
                        attribute: attribute.clone(),
                    },
                    "multiple distinct one-valued assertions",
                ));
            }
        }
        // Cardinality-one assertions replace every current value of their
        // entity/attribute, as the former full-state retain did.
        let mut replaced = Vec::new();
        for (entity, attribute) in one_values.keys() {
            for value in self.values(entity, attribute) {
                replaced.push(Fact {
                    entity: entity.clone(),
                    attribute: attribute.clone(),
                    value,
                });
            }
        }
        let mut touched = Touched::default();
        for fact in retractions.iter().chain(&replaced).chain(&assertions) {
            touched.note(self, fact);
        }
        for fact in retractions.iter().chain(&replaced) {
            self.remove_fact(fact);
        }
        let asserted: Vec<_> = assertions.into_iter().collect();
        for fact in &asserted {
            self.insert_fact(fact.clone());
        }
        let effective_delta = touched.effective(self);
        Ok((catalog_delta, effective_delta, asserted))
    }
}

#[cfg(test)]
#[path = "model_reference.rs"]
mod reference;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{AttributeDefinition, Revision, TempId, Uniqueness, ValueType};
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    pub fn empty() -> Model {
        Model::empty(
            "d1.00000000000000000000000000000001.00000000000000000000000000000002.0"
                .parse()
                .unwrap(),
        )
        .unwrap()
    }
    pub fn attr(name: &str) -> AttrName {
        name.parse().unwrap()
    }
    pub fn def(
        name: &str,
        ty: ValueType,
        cardinality: Cardinality,
        uniqueness: Uniqueness,
    ) -> AttributeDefinition {
        AttributeDefinition::new(attr(name), ty, cardinality, uniqueness).unwrap()
    }
    pub fn setup(cardinality: Cardinality) -> (Model, EntityId) {
        let label = TempId::new("e").unwrap();
        let tx = Tx::builder()
            .define_attribute(def(
                "x/value",
                ValueType::I64,
                cardinality,
                Uniqueness::None,
            ))
            .new_entity(label.clone())
            .build()
            .unwrap();
        let result = empty().normalize(&tx).unwrap();
        (result.next, result.catalog_delta.tempids[&label].clone())
    }
    fn change(id: &EntityId, operation: Operation, value: i64) -> crate::Change {
        crate::Change {
            operation,
            entity: id.clone().into(),
            attribute: attr("x/value"),
            value: Value::I64(value).into(),
        }
    }

    #[test]
    fn duplicates_noops_replacement_and_explicit_old_retraction() {
        let (model, id) = setup(Cardinality::One);
        let a = model
            .normalize(
                &Tx::builder()
                    .assert(id.clone(), attr("x/value"), Value::I64(1))
                    .build()
                    .unwrap(),
            )
            .unwrap()
            .next;
        let noop = a
            .normalize(
                &Tx::builder()
                    .assert(id.clone(), attr("x/value"), Value::I64(1))
                    .assert(id.clone(), attr("x/value"), Value::I64(1))
                    .retract(id.clone(), attr("x/value"), Value::I64(99))
                    .build()
                    .unwrap(),
            )
            .unwrap();
        assert!(noop.effective_delta.is_empty());
        assert_eq!(
            noop.next.catalog.snapshot.revision(),
            a.catalog.snapshot.revision().next().unwrap()
        );
        let replace = a
            .normalize(
                &Tx::builder()
                    .assert(id.clone(), attr("x/value"), Value::I64(2))
                    .build()
                    .unwrap(),
            )
            .unwrap();
        let explicit = a
            .normalize(
                &Tx::builder()
                    .retract(id.clone(), attr("x/value"), Value::I64(1))
                    .assert(id.clone(), attr("x/value"), Value::I64(2))
                    .build()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(replace.effective_delta, explicit.effective_delta);
        assert_eq!(replace.effective_delta.len(), 2);
        assert_eq!(replace.effective_delta[0].operation, Operation::Retract);
        assert_eq!(replace.effective_delta[0].fact.value, Value::I64(1));
        assert_eq!(replace.effective_delta[1].fact.value, Value::I64(2));
        let removed = replace
            .next
            .normalize(
                &Tx::builder()
                    .retract(id.clone(), attr("x/value"), Value::I64(2))
                    .build()
                    .unwrap(),
            )
            .unwrap()
            .next;
        assert!(removed.facts().is_empty());
        assert!(removed.catalog.require_known(&id).is_ok());
    }

    #[test]
    fn batch_conflicts_are_not_sequential_and_never_mutate_input() {
        let (model, id) = setup(Cardinality::One);
        for operations in [
            vec![
                change(&id, Operation::Assert, 1),
                change(&id, Operation::Retract, 1),
            ],
            vec![
                change(&id, Operation::Assert, 1),
                change(&id, Operation::Assert, 2),
            ],
        ] {
            let tx = Tx {
                changes: operations,
                ..Tx::default()
            };
            assert_eq!(
                model.normalize(&tx).unwrap_err().code(),
                ErrorCode::ConflictingBatch
            );
            let reversed = Tx {
                changes: tx.changes.iter().cloned().rev().collect(),
                ..tx
            };
            assert_eq!(
                model.normalize(&reversed).unwrap_err().code(),
                ErrorCode::ConflictingBatch
            );
        }
        assert!(model.facts().is_empty());
        assert_eq!(model.catalog.snapshot.revision(), Revision::new(1));
    }

    #[test]
    fn temporary_refs_cycles_and_fabricated_future_ids() {
        let a = TempId::new("a").unwrap();
        let b = TempId::new("b").unwrap();
        let tx = Tx::builder()
            .define_attribute(def(
                "x/ref",
                ValueType::Ref,
                Cardinality::One,
                Uniqueness::None,
            ))
            .new_entity(a.clone())
            .new_entity(b.clone())
            .assert(a.clone(), attr("x/ref"), ValueTerm::TempRef(b.clone()))
            .assert(b, attr("x/ref"), ValueTerm::TempRef(a))
            .build()
            .unwrap();
        let result = empty().normalize(&tx).unwrap();
        assert_eq!(result.next.facts().len(), 2);
        let guessed: EntityId = "e1.00000000000000000000000000000001.0000000000000001"
            .parse()
            .unwrap();
        let mut fabricated = tx.clone();
        fabricated.changes[0].entity = EntityTerm::Id(guessed.clone());
        assert_eq!(
            empty().normalize(&fabricated).unwrap_err().code(),
            ErrorCode::UnknownEntity
        );
        let mut fabricated_ref = tx;
        fabricated_ref.changes[0].value = Value::Ref(guessed).into();
        assert_eq!(
            empty().normalize(&fabricated_ref).unwrap_err().code(),
            ErrorCode::UnknownEntity
        );
    }

    #[test]
    fn schema_types_unknown_ids_and_absent_retractions_are_validated() {
        let (model, id) = setup(Cardinality::Many);
        assert_eq!(
            model
                .normalize(
                    &Tx::builder()
                        .assert(id.clone(), attr("x/value"), Value::String("wrong".into()))
                        .build()
                        .unwrap()
                )
                .unwrap_err()
                .code(),
            ErrorCode::TypeMismatch
        );
        assert_eq!(
            model
                .normalize(
                    &Tx::builder()
                        .retract(id.clone(), attr("x/unknown"), Value::I64(1))
                        .build()
                        .unwrap()
                )
                .unwrap_err()
                .code(),
            ErrorCode::UnknownAttribute
        );
        let unknown = "e1.00000000000000000000000000000001.0000000000000002"
            .parse::<EntityId>()
            .unwrap();
        assert_eq!(
            model
                .normalize(
                    &Tx::builder()
                        .retract(unknown, attr("x/value"), Value::I64(1))
                        .build()
                        .unwrap()
                )
                .unwrap_err()
                .code(),
            ErrorCode::UnknownEntity
        );
        assert!(
            model
                .normalize(
                    &Tx::builder()
                        .retract(id, attr("x/value"), Value::I64(1))
                        .build()
                        .unwrap()
                )
                .unwrap()
                .effective_delta
                .is_empty()
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        #[test]
        fn many_sets_match_independent_set_equation(initial in proptest::collection::vec(0_i64..16, 0..32), adds in proptest::collection::vec(0_i64..16, 0..32), removes in proptest::collection::vec(0_i64..16, 0..32)) {
            let (mut model, id) = setup(Cardinality::Many);
            model.set_facts(initial.iter().map(|n| Fact { entity:id.clone(), attribute:attr("x/value"), value:Value::I64(*n) }).collect());
            let mut operations:Vec<_> = adds.iter().map(|n|change(&id,Operation::Assert,*n)).chain(removes.iter().map(|n|change(&id,Operation::Retract,*n))).collect();
            let tx = Tx { changes:operations.clone(), ..Tx::default() }; let result = model.normalize(&tx);
            operations.reverse(); let reversed = model.normalize(&Tx { changes:operations, ..Tx::default() });
            let add_set:BTreeSet<_> = adds.into_iter().collect(); let remove_set:BTreeSet<_> = removes.into_iter().collect();
            if !add_set.is_disjoint(&remove_set) {
                prop_assert_eq!(result.unwrap_err().code(), ErrorCode::ConflictingBatch); prop_assert_eq!(reversed.unwrap_err().code(), ErrorCode::ConflictingBatch);
            } else {
                let result=result.unwrap(); let reversed=reversed.unwrap();
                let original:BTreeSet<_> = initial.into_iter().collect(); let expected:BTreeSet<_> = original.difference(&remove_set).copied().chain(add_set).collect();
                let actual:BTreeSet<_> = result.next.facts().iter().map(|f| match f.value {Value::I64(n)=>n,_=>unreachable!()}).collect();
                prop_assert_eq!(actual,expected); prop_assert_eq!(result.next.facts(),reversed.next.facts()); prop_assert_eq!(result.effective_delta,reversed.effective_delta);
            }
        }
        #[test]
        fn valid_many_batches_match_set_equation_and_replay(initial in proptest::collection::vec(-16_i64..16, 0..32), adds in proptest::collection::vec(0_i64..16, 0..32), removes in proptest::collection::vec(-16_i64..0, 0..32)) {
            let (mut model,id)=setup(Cardinality::Many);
            model.set_facts(initial.iter().map(|n|Fact {entity:id.clone(),attribute:attr("x/value"),value:Value::I64(*n)}).collect());
            let operations=adds.iter().map(|n|change(&id,Operation::Assert,*n)).chain(removes.iter().map(|n|change(&id,Operation::Retract,*n))).collect();
            let result=model.normalize(&Tx {changes:operations,..Tx::default()}).unwrap();
            let expected:BTreeSet<_>=initial.into_iter().filter(|n|!removes.contains(n)).chain(adds).collect();
            let actual:BTreeSet<_>=result.next.facts().iter().map(|f|match f.value {Value::I64(n)=>n,_=>unreachable!()}).collect();
            prop_assert_eq!(actual,expected);
            let mut replay=model.facts().clone();
            for event in result.effective_delta { match event.operation {Operation::Retract=>{prop_assert!(replay.remove(&event.fact));},Operation::Assert=>{prop_assert!(replay.insert(event.fact));}} }
            prop_assert_eq!(&replay,result.next.facts());
        }
        #[test]
        fn one_valued_assertions_conflict_iff_distinct(a in any::<i64>(), b in any::<i64>()) {
            let (model,id)=setup(Cardinality::One); let tx=Tx::builder().assert(id.clone(),attr("x/value"),Value::I64(a)).assert(id,attr("x/value"),Value::I64(b)).build().unwrap();
            let result=model.normalize(&tx);
            if a==b { prop_assert_eq!(result.unwrap().next.facts().len(),1); } else {prop_assert_eq!(result.unwrap_err().code(),ErrorCode::ConflictingBatch);}
        }
    }
}
