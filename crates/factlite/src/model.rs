use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
};

use crate::catalog::{Catalog, CatalogDelta};
use crate::{
    AttrName, Cardinality, EntityId, EntityTerm, Error, ErrorCode, ErrorContext, Operation, Result,
    SnapshotId, Tx, Value, ValueTerm,
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

#[derive(Clone, Debug)]
pub(crate) struct Model {
    pub catalog: Catalog,
    pub facts: HashSet<Fact>,
}

#[derive(Clone, Debug)]
pub(crate) struct NormalizedBatch {
    pub next: Model,
    pub catalog_delta: CatalogDelta,
    pub effective_delta: Vec<FactDelta>,
}

impl Model {
    pub fn empty(snapshot: SnapshotId) -> Result<Self> {
        Ok(Self {
            catalog: Catalog::empty(snapshot)?,
            facts: HashSet::new(),
        })
    }

    // This is batch normalization, not commit. Preconditions and final-state
    // uniqueness are enforced by evaluate in item 1.5 before exposing a report.
    pub fn normalize(&self, tx: &Tx) -> Result<NormalizedBatch> {
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
        let mut effective_delta: Vec<_> = self
            .facts
            .difference(&facts)
            .cloned()
            .map(|fact| FactDelta {
                operation: Operation::Retract,
                fact,
            })
            .chain(
                facts
                    .difference(&self.facts)
                    .cloned()
                    .map(|fact| FactDelta {
                        operation: Operation::Assert,
                        fact,
                    }),
            )
            .collect();
        effective_delta.sort_by(|a, b| {
            a.operation
                .cmp(&b.operation)
                .then_with(|| a.fact.canonical_cmp(&b.fact))
        });
        Ok(NormalizedBatch {
            next: Model { catalog, facts },
            catalog_delta,
            effective_delta,
        })
    }
}

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
        assert!(removed.facts.is_empty());
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
        assert!(model.facts.is_empty());
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
        assert_eq!(result.next.facts.len(), 2);
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
            model.facts = initial.iter().map(|n| Fact { entity:id.clone(), attribute:attr("x/value"), value:Value::I64(*n) }).collect();
            let mut operations:Vec<_> = adds.iter().map(|n|change(&id,Operation::Assert,*n)).chain(removes.iter().map(|n|change(&id,Operation::Retract,*n))).collect();
            let tx = Tx { changes:operations.clone(), ..Tx::default() }; let result = model.normalize(&tx);
            operations.reverse(); let reversed = model.normalize(&Tx { changes:operations, ..Tx::default() });
            let add_set:BTreeSet<_> = adds.into_iter().collect(); let remove_set:BTreeSet<_> = removes.into_iter().collect();
            if !add_set.is_disjoint(&remove_set) {
                prop_assert_eq!(result.unwrap_err().code(), ErrorCode::ConflictingBatch); prop_assert_eq!(reversed.unwrap_err().code(), ErrorCode::ConflictingBatch);
            } else {
                let result=result.unwrap(); let reversed=reversed.unwrap();
                let original:BTreeSet<_> = initial.into_iter().collect(); let expected:BTreeSet<_> = original.difference(&remove_set).copied().chain(add_set).collect();
                let actual:BTreeSet<_> = result.next.facts.iter().map(|f| match f.value {Value::I64(n)=>n,_=>unreachable!()}).collect();
                prop_assert_eq!(actual,expected); prop_assert_eq!(result.next.facts,reversed.next.facts); prop_assert_eq!(result.effective_delta,reversed.effective_delta);
            }
        }
        #[test]
        fn valid_many_batches_match_set_equation_and_replay(initial in proptest::collection::vec(-16_i64..16, 0..32), adds in proptest::collection::vec(0_i64..16, 0..32), removes in proptest::collection::vec(-16_i64..0, 0..32)) {
            let (mut model,id)=setup(Cardinality::Many);
            model.facts=initial.iter().map(|n|Fact {entity:id.clone(),attribute:attr("x/value"),value:Value::I64(*n)}).collect();
            let operations=adds.iter().map(|n|change(&id,Operation::Assert,*n)).chain(removes.iter().map(|n|change(&id,Operation::Retract,*n))).collect();
            let result=model.normalize(&Tx {changes:operations,..Tx::default()}).unwrap();
            let expected:BTreeSet<_>=initial.into_iter().filter(|n|!removes.contains(n)).chain(adds).collect();
            let actual:BTreeSet<_>=result.next.facts.iter().map(|f|match f.value {Value::I64(n)=>n,_=>unreachable!()}).collect();
            prop_assert_eq!(actual,expected);
            let mut replay=model.facts;
            for event in result.effective_delta { match event.operation {Operation::Retract=>{prop_assert!(replay.remove(&event.fact));},Operation::Assert=>{prop_assert!(replay.insert(event.fact));}} }
            prop_assert_eq!(replay,result.next.facts);
        }
        #[test]
        fn one_valued_assertions_conflict_iff_distinct(a in any::<i64>(), b in any::<i64>()) {
            let (model,id)=setup(Cardinality::One); let tx=Tx::builder().assert(id.clone(),attr("x/value"),Value::I64(a)).assert(id,attr("x/value"),Value::I64(b)).build().unwrap();
            let result=model.normalize(&tx);
            if a==b { prop_assert_eq!(result.unwrap().next.facts.len(),1); } else {prop_assert_eq!(result.unwrap_err().code(),ErrorCode::ConflictingBatch);}
        }
    }
}
