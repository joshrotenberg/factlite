//! The full-state transitions used before item 9.4, kept verbatim in test
//! builds as the reference for the incremental ones. Every check here scans or
//! clones the complete fact set.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::{Fact, FactDelta, Model, NormalizedBatch, TxReport};
use crate::{
    AttrName, Cardinality, EntityId, EntityTerm, Error, ErrorCode, ErrorContext, Operation, Result,
    SnapshotId, Tx, Uniqueness, Value, ValueTerm,
};

impl Model {
    // Reference copies of the pre-9.4 full-state transitions, used only by tests.
    pub fn apply_report_reference(&self, report: &TxReport) -> Result<Self> {
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
        let (catalog, delta) = self
            .catalog
            .stage(&report.schema_added, &labels)
            .map_err(|err| corrupt(err.to_string()))?;
        if delta.schema_added != report.schema_added || delta.tempids != report.tempids {
            return Err(corrupt(
                "report declarations do not match catalog allocations",
            ));
        }
        let mut facts = self.facts.clone();
        for event in &report.effective_delta {
            let effective = match event.operation {
                Operation::Retract => facts.remove(&event.fact),
                Operation::Assert => facts.insert(event.fact.clone()),
            };
            if !effective {
                return Err(corrupt("fact log contains an ineffective event"));
            }
        }
        if effective_changes_reference(&self.facts, &facts) != report.effective_delta {
            return Err(corrupt(
                "fact events are not canonical effective differences",
            ));
        }
        let next = Model::new(catalog, facts);
        next.validate_state_reference()
            .map_err(|err| corrupt(err.to_string()))?;
        Ok(next)
    }

    fn validate_state_reference(&self) -> Result<()> {
        let schema = self.catalog.schema_at(self.catalog.snapshot.revision())?;
        let mut one_counts: HashMap<(&EntityId, &AttrName), usize> = HashMap::new();
        for fact in &self.facts {
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
            if definition.cardinality() == Cardinality::One {
                let count = one_counts
                    .entry((&fact.entity, &fact.attribute))
                    .or_default();
                *count += 1;
                if *count > 1 {
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
        }
        self.validate_uniqueness_reference()
    }

    pub fn evaluate_reference(&self, tx: &Tx) -> Result<(Self, TxReport)> {
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
            let mut actual: Vec<_> = self
                .facts
                .iter()
                .filter(|fact| {
                    fact.entity == expectation.entity && fact.attribute == expectation.attribute
                })
                .map(|fact| fact.value.clone())
                .collect();
            actual.sort_by(Value::canonical_cmp);
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
        let batch = self.normalize_reference(&tx)?;
        batch.next.validate_uniqueness_reference()?;
        let mut created_entities: Vec<_> = batch.catalog_delta.tempids.values().cloned().collect();
        created_entities.sort();
        let report = TxReport {
            before: self.catalog.snapshot.clone(),
            after: batch.next.catalog.snapshot.clone(),
            tempids: batch.catalog_delta.tempids,
            created_entities,
            schema_added: batch.catalog_delta.schema_added,
            effective_delta: batch.effective_delta,
            metadata: tx.metadata,
        };
        Ok((batch.next, report))
    }

    fn validate_uniqueness_reference(&self) -> Result<()> {
        let mut by_attr: BTreeMap<AttrName, HashMap<Value, BTreeSet<EntityId>>> = BTreeMap::new();
        for fact in &self.facts {
            if self.catalog.schema[&fact.attribute].0.uniqueness() == Uniqueness::UniqueOne {
                by_attr
                    .entry(fact.attribute.clone())
                    .or_default()
                    .entry(fact.value.clone())
                    .or_default()
                    .insert(fact.entity.clone());
            }
        }
        for (attribute, values) in by_attr {
            if let Some((value, owners)) = values
                .into_iter()
                .filter(|(_, owners)| owners.len() > 1)
                .min_by(|(a, _), (b, _)| a.canonical_cmp(b))
            {
                return Err(Error::new(
                    ErrorCode::UniqueViolation,
                    ErrorContext::UniqueOwnership {
                        attribute,
                        value,
                        owners: owners.into_iter().collect(),
                    },
                    "unique value has multiple final-state owners",
                ));
            }
        }
        Ok(())
    }

    /// Lookup resolution by scanning every pre-state fact, independent of the
    /// production owners index.
    fn lookup_reference(&self, attribute: &AttrName, value: &Value) -> Result<EntityId> {
        let schema = self.catalog.schema_at(self.catalog.snapshot.revision())?;
        let definition = schema.get(attribute).ok_or_else(|| {
            Error::new(
                ErrorCode::UnknownAttribute,
                ErrorContext::Attribute {
                    attribute: attribute.clone(),
                },
                "lookup attribute is unknown at pre-head",
            )
        })?;
        if definition.uniqueness() != crate::Uniqueness::UniqueOne {
            return Err(Error::invalid(
                "lookup",
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
                "lookup value has the wrong attribute type",
            ));
        }
        self.facts()
            .iter()
            .filter(|fact| &fact.attribute == attribute && &fact.value == value)
            .map(|fact| fact.entity.clone())
            .min()
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::UnknownEntity,
                    ErrorContext::Lookup {
                        attribute: attribute.clone(),
                        value: value.clone(),
                    },
                    "lookup matched no entity at pre-head",
                )
            })
    }

    // Batch normalization is private; evaluate adds preconditions and final
    // uniqueness before storage can expose a report.
    pub fn normalize_reference(&self, tx: &Tx) -> Result<NormalizedBatch> {
        let (catalog, catalog_delta) = self.catalog.stage(&tx.schema, &tx.new_entities)?;
        let schema = catalog.schema_at(catalog.snapshot.revision())?;
        let mut assertions = HashSet::new();
        let mut retractions = HashSet::new();
        for change in &tx.changes {
            let entity = match &change.entity {
                EntityTerm::Id(id) => {
                    self.catalog.require_known(id)?;
                    id.clone()
                }
                EntityTerm::Temp(label) => catalog_delta
                    .tempids
                    .get(label)
                    .cloned()
                    .ok_or_else(|| Error::invalid("tempid", "undeclared temporary entity"))?,
                EntityTerm::Lookup(attribute, value) => self.lookup_reference(attribute, value)?,
            };
            let value = match &change.value {
                ValueTerm::Value(value) => {
                    if let Value::Ref(id) = value {
                        self.catalog.require_known(id)?;
                    }
                    value.clone()
                }
                ValueTerm::TempRef(label) => {
                    Value::Ref(catalog_delta.tempids.get(label).cloned().ok_or_else(|| {
                        Error::invalid("tempid", "undeclared temporary reference")
                    })?)
                }
                ValueTerm::LookupRef(attribute, value) => {
                    Value::Ref(self.lookup_reference(attribute, value)?)
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
        let mut facts = self.facts.clone();
        facts.retain(|fact| {
            !retractions.contains(fact)
                && !one_values.contains_key(&(fact.entity.clone(), fact.attribute.clone()))
        });
        facts.extend(assertions);
        let effective_delta = effective_changes_reference(&self.facts, &facts);
        Ok(NormalizedBatch {
            next: Model::new(catalog, facts),
            catalog_delta,
            effective_delta,
        })
    }
}

fn effective_changes_reference(before: &HashSet<Fact>, after: &HashSet<Fact>) -> Vec<FactDelta> {
    let mut delta: Vec<_> = before
        .difference(after)
        .cloned()
        .map(|fact| FactDelta {
            operation: Operation::Retract,
            fact,
        })
        .chain(after.difference(before).cloned().map(|fact| FactDelta {
            operation: Operation::Assert,
            fact,
        }))
        .collect();
    delta.sort_by(|a, b| {
        a.operation
            .cmp(&b.operation)
            .then_with(|| a.fact.canonical_cmp(&b.fact))
    });
    delta
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::tests::{attr, def, empty};
    use super::*;
    use crate::{Cardinality, TempId, Uniqueness, ValueType};
    use proptest::prelude::*;

    // Attributes: cardinality one, many, unique one, a reference, and a
    // unique reference, so every incremental check has a path to exercise.
    pub(crate) const ATTRS: [(&str, ValueType, Cardinality, Uniqueness); 5] = [
        ("x/one", ValueType::I64, Cardinality::One, Uniqueness::None),
        (
            "x/many",
            ValueType::I64,
            Cardinality::Many,
            Uniqueness::None,
        ),
        (
            "x/unique",
            ValueType::I64,
            Cardinality::One,
            Uniqueness::UniqueOne,
        ),
        ("x/ref", ValueType::Ref, Cardinality::Many, Uniqueness::None),
        (
            "x/owner",
            ValueType::Ref,
            Cardinality::One,
            Uniqueness::UniqueOne,
        ),
    ];

    /// Every attribute, entities k0 to k2, and owned unique values so that
    /// generated lookups usually resolve: k0, k1 and k2 own `x/unique` 0, 1
    /// and 2, and k1 owns `x/owner` naming k0.
    pub(crate) fn seed() -> Tx {
        let label = |n: usize| TempId::new(format!("k{n}")).unwrap();
        let mut changes: Vec<_> = (0..3)
            .map(|n| crate::Change {
                operation: Operation::Assert,
                entity: EntityTerm::Temp(label(n)),
                attribute: attr("x/unique"),
                value: ValueTerm::Value(Value::I64(n as i64)),
            })
            .collect();
        changes.push(crate::Change {
            operation: Operation::Assert,
            entity: EntityTerm::Temp(label(1)),
            attribute: attr("x/owner"),
            value: ValueTerm::TempRef(label(0)),
        });
        Tx {
            schema: (0..ATTRS.len()).map(definition).collect(),
            new_entities: (0..3).map(label).collect(),
            changes,
            ..Tx::default()
        }
    }

    pub(crate) fn definition(index: usize) -> crate::AttributeDefinition {
        let (name, ty, cardinality, uniqueness) = ATTRS[index];
        def(name, ty, cardinality, uniqueness)
    }

    /// One generated change: (assert?, entity slot, attribute, value slot).
    /// Entity and value slots past the known entities name temporary labels;
    /// slot 7 is a fabricated, never-allocated identity. Slots 8 to 11 are
    /// lookups of `x/unique` values 0 to 3, slot 12 a lookup on the
    /// non-unique `x/one`, and slot 13 a lookup of `x/owner` naming the first
    /// known entity.
    type Change = (bool, u8, usize, u8);
    /// (changes, expectations, new labels, declare schema, stale head fence).
    pub(crate) type Step = (Vec<Change>, Vec<(u8, usize, Vec<u8>)>, u8, u8, bool);

    fn entity(model: &Model, known: &[EntityId], slot: u8) -> Option<EntityTerm> {
        match slot {
            7 => Some(EntityTerm::Id(
                EntityId::allocated(model.catalog.snapshot.store().clone(), 0xfff).unwrap(),
            )),
            slot if (slot as usize) < known.len() => {
                Some(EntityTerm::Id(known[slot as usize].clone()))
            }
            slot if slot < 6 => Some(EntityTerm::Temp(
                TempId::new(format!("t{}", slot % 2)).unwrap(),
            )),
            8..=11 => Some(EntityTerm::Lookup(
                attr("x/unique"),
                Value::I64(i64::from(slot - 8)),
            )),
            12 => Some(EntityTerm::Lookup(attr("x/one"), Value::I64(0))),
            13 => Some(EntityTerm::Lookup(
                attr("x/owner"),
                Value::Ref(known.first().cloned().unwrap_or_else(|| {
                    EntityId::allocated(model.catalog.snapshot.store().clone(), 0xffd).unwrap()
                })),
            )),
            _ => None,
        }
    }

    fn value(model: &Model, known: &[EntityId], attribute: usize, slot: u8) -> ValueTerm {
        // A few slots give a value of the wrong type for any attribute.
        if slot >= 248 {
            return ValueTerm::Value(Value::String("wrong".into()));
        }
        if ATTRS[attribute].1 == ValueType::I64 {
            return ValueTerm::Value(Value::I64(i64::from(slot % 4)));
        }
        match entity(model, known, slot % 14) {
            Some(EntityTerm::Id(id)) => ValueTerm::Value(Value::Ref(id)),
            Some(EntityTerm::Temp(label)) => ValueTerm::TempRef(label),
            Some(EntityTerm::Lookup(attribute, value)) => ValueTerm::LookupRef(attribute, value),
            None => ValueTerm::Value(Value::Ref(known.first().cloned().unwrap_or_else(|| {
                EntityId::allocated(model.catalog.snapshot.store().clone(), 0xffe).unwrap()
            }))),
        }
    }

    pub(crate) fn build(model: &Model, known: &[EntityId], step: &Step) -> Tx {
        let (changes, expectations, labels, schema, stale) = step;
        let mut tx = Tx::default();
        for index in 0..ATTRS.len() {
            if schema & (1 << index) != 0 {
                tx.schema.push(definition(index));
            }
        }
        // Bit 7 redeclares x/one with a conflicting cardinality.
        if schema & 0x80 != 0 {
            tx.schema.push(def(
                "x/one",
                ValueType::I64,
                Cardinality::Many,
                Uniqueness::None,
            ));
        }
        for label in 0..*labels % 3 {
            tx.new_entities
                .push(TempId::new(format!("t{label}")).unwrap());
        }
        for &(assert, slot, attribute, value_slot) in changes {
            let Some(entity) = entity(model, known, slot) else {
                continue;
            };
            tx.changes.push(crate::Change {
                operation: if assert {
                    Operation::Assert
                } else {
                    Operation::Retract
                },
                entity,
                attribute: attr(ATTRS[attribute].0),
                value: value(model, known, attribute, value_slot),
            });
        }
        for (slot, attribute, values) in expectations {
            let Some(EntityTerm::Id(entity)) = entity(model, known, *slot) else {
                continue;
            };
            let values = values
                .iter()
                .filter_map(|slot| match value(model, known, *attribute, *slot) {
                    ValueTerm::Value(value) => Some(value),
                    ValueTerm::TempRef(_) | ValueTerm::LookupRef(..) => None,
                })
                .collect();
            tx.expectations.push(crate::Expectation {
                entity,
                attribute: attr(ATTRS[*attribute].0),
                values,
            });
        }
        if *stale {
            tx.expected_head = Some(model.catalog.snapshot.clone());
            if model.catalog.snapshot.revision() > crate::Revision::ZERO {
                tx.expected_head = Some(SnapshotId::new(
                    model.catalog.snapshot.store().clone(),
                    model.catalog.snapshot.lineage().clone(),
                    crate::Revision::ZERO,
                ));
            }
        }
        tx
    }

    fn same(a: &Model, b: &Model) -> bool {
        crate::index::same_state(a, b)
    }

    pub(crate) fn same_error(a: &crate::Error, b: &crate::Error) -> bool {
        a.code() == b.code()
            && format!("{:?}", a.context()) == format!("{:?}", b.context())
            && a.to_string() == b.to_string()
    }

    /// Deterministic report damage: drop, duplicate, flip, reorder, rewrite.
    fn tamper(report: &TxReport, kind: u8) -> Option<TxReport> {
        let mut damaged = report.clone();
        let delta = &mut damaged.effective_delta;
        match kind % 5 {
            0 if !delta.is_empty() => {
                delta.pop();
            }
            1 if !delta.is_empty() => delta.push(delta[0].clone()),
            2 if !delta.is_empty() => {
                delta[0].operation = match delta[0].operation {
                    Operation::Assert => Operation::Retract,
                    Operation::Retract => Operation::Assert,
                }
            }
            3 if delta.len() > 1 => delta.swap(0, 1),
            4 if !delta.is_empty() => {
                delta[0].fact.value = match &delta[0].fact.value {
                    Value::I64(n) => Value::I64(n.wrapping_add(17)),
                    _ => Value::I64(0),
                }
            }
            _ => return None,
        }
        Some(damaged)
    }

    fn run(steps: &[Step], seed_schema: bool) {
        let mut model = empty();
        if seed_schema {
            model = model.evaluate_reference(&seed()).unwrap().0;
        }
        for (index, step) in steps.iter().enumerate() {
            let known: Vec<_> = model.catalog.entities.keys().cloned().collect();
            let tx = build(&model, &known, step);
            let incremental = model.evaluate(&tx);
            let reference = model.evaluate_reference(&tx);
            match (incremental, reference) {
                (Ok((next, report)), Ok((expected, expected_report))) => {
                    assert_eq!(report, expected_report, "step {index}: {tx:?}");
                    assert!(same(&next, &expected), "step {index}: {tx:?}");
                    let applied = model.apply_report(&report).unwrap();
                    assert!(same(
                        &applied,
                        &model.apply_report_reference(&report).unwrap()
                    ));
                    assert!(same(&applied, &next));
                    for kind in 0..5 {
                        if let Some(damaged) = tamper(&report, kind + index as u8) {
                            let fast = model.apply_report(&damaged);
                            let full = model.apply_report_reference(&damaged);
                            assert_eq!(fast.is_ok(), full.is_ok(), "tamper {kind}: {damaged:?}");
                            if let (Ok(fast), Ok(full)) = (fast, full) {
                                assert!(same(&fast, &full));
                            }
                        }
                    }
                    model = next;
                }
                (Err(error), Err(expected)) => {
                    assert!(
                        same_error(&error, &expected),
                        "step {index}: {error:?} vs {expected:?}"
                    );
                }
                (actual, expected) => panic!(
                    "step {index} diverged: {:?} vs {:?} for {tx:?}",
                    actual.map(|(_, report)| report),
                    expected.map(|(_, report)| report)
                ),
            }
        }
    }

    pub(crate) fn step() -> impl Strategy<Value = Step> {
        (
            proptest::collection::vec((any::<bool>(), 0_u8..14, 0_usize..5, any::<u8>()), 0..6),
            proptest::collection::vec(
                (
                    0_u8..8,
                    0_usize..5,
                    proptest::collection::vec(any::<u8>(), 0..3),
                ),
                0..2,
            ),
            any::<u8>(),
            prop_oneof![8 => Just(0_u8), 1 => any::<u8>()],
            proptest::bool::weighted(0.05),
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        #[test]
        fn incremental_transitions_equal_full_state_reference(
            steps in proptest::collection::vec(step(), 1..16),
            seed_schema in proptest::bool::weighted(0.9),
        ) {
            run(&steps, seed_schema);
        }
    }

    #[test]
    fn unique_violation_choice_matches_full_scan() {
        // Two attributes, several simultaneous violations: the least attribute
        // and then least value must be reported, with all owners.
        let mut model = empty();
        let setup = Tx {
            schema: vec![
                def(
                    "a/u",
                    ValueType::I64,
                    Cardinality::One,
                    Uniqueness::UniqueOne,
                ),
                def(
                    "b/u",
                    ValueType::I64,
                    Cardinality::One,
                    Uniqueness::UniqueOne,
                ),
            ],
            new_entities: (0..4)
                .map(|n| TempId::new(format!("e{n}")).unwrap())
                .collect(),
            ..Tx::default()
        };
        model = model.evaluate_reference(&setup).unwrap().0;
        let ids: Vec<_> = model.catalog.entities.keys().cloned().collect();
        let mut tx = Tx::builder();
        for (index, id) in ids.iter().enumerate() {
            tx = tx.assert(id.clone(), attr("b/u"), Value::I64(1)).assert(
                id.clone(),
                attr("a/u"),
                Value::I64(3 - (index as i64 % 2)),
            );
        }
        let tx = tx.build().unwrap();
        let fast = model.evaluate(&tx).unwrap_err();
        let full = model.evaluate_reference(&tx).unwrap_err();
        assert!(same_error(&fast, &full), "{fast:?} vs {full:?}");
        assert_eq!(fast.code(), ErrorCode::UniqueViolation);
    }
}
