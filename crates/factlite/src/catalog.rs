use std::collections::{BTreeMap, BTreeSet};

use crate::{
    AttrName, AttributeDefinition, EntityId, Error, ErrorCode, ErrorContext, Result, Revision,
    Schema, SnapshotId, TempId,
};

#[derive(Clone, Debug)]
pub(crate) struct Catalog {
    pub snapshot: SnapshotId,
    pub schema: BTreeMap<AttrName, (AttributeDefinition, Revision)>,
    pub entities: BTreeMap<EntityId, Revision>,
    next_allocation: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CatalogDelta {
    pub schema_added: Vec<AttributeDefinition>,
    pub tempids: BTreeMap<TempId, EntityId>,
}

impl Catalog {
    pub fn empty(snapshot: SnapshotId) -> Result<Self> {
        if snapshot.revision() != Revision::ZERO {
            return Err(Error::invalid(
                "catalog",
                "an empty catalog starts at revision zero",
            ));
        }
        Ok(Self {
            snapshot,
            schema: BTreeMap::new(),
            entities: BTreeMap::new(),
            next_allocation: Some(1),
        })
    }

    pub fn schema_at(&self, revision: Revision) -> Result<Schema> {
        self.validate_revision(revision)?;
        Ok(Schema {
            definitions: self
                .schema
                .iter()
                .filter(|(_, (_, created))| *created <= revision)
                .map(|(name, (definition, _))| (name.clone(), definition.clone()))
                .collect(),
        })
    }

    pub fn known_at(&self, entity: &EntityId, revision: Revision) -> Result<bool> {
        self.validate_revision(revision)?;
        Ok(entity.store() == self.snapshot.store()
            && self
                .entities
                .get(entity)
                .is_some_and(|created| *created <= revision))
    }

    pub fn require_known(&self, entity: &EntityId) -> Result<()> {
        if !self.known_at(entity, self.snapshot.revision())? {
            return Err(Error::new(
                ErrorCode::UnknownEntity,
                ErrorContext::Entity {
                    entity: entity.clone(),
                },
                "entity is not allocated in this store at this revision",
            ));
        }
        Ok(())
    }

    fn validate_revision(&self, revision: Revision) -> Result<()> {
        if revision > self.snapshot.revision() {
            return Err(Error::new(
                ErrorCode::InvalidSnapshot,
                ErrorContext::InvalidInput { field: "revision" },
                "catalog view exceeds its head",
            ));
        }
        Ok(())
    }

    // Staging returns a separate value; any later failure discards allocations.
    pub fn stage(
        &self,
        definitions: &[AttributeDefinition],
        labels: &[TempId],
    ) -> Result<(Self, CatalogDelta)> {
        let revision = self.snapshot.revision().next()?;
        let mut next = self.clone();
        let mut seen = BTreeSet::new();
        let mut schema_added = Vec::new();
        for definition in definitions {
            let name = definition.name();
            if !seen.insert(name.clone()) {
                return Err(Error::invalid(
                    "schema",
                    "an attribute may be declared at most once per request",
                ));
            }
            match next.schema.get(name) {
                Some((existing, _)) if existing != definition => {
                    return Err(Error::new(
                        ErrorCode::ConflictingBatch,
                        ErrorContext::Attribute {
                            attribute: name.clone(),
                        },
                        "attribute definition is immutable",
                    ));
                }
                Some(_) => {}
                None => {
                    next.schema
                        .insert(name.clone(), (definition.clone(), revision));
                    schema_added.push(definition.clone());
                }
            }
        }
        schema_added.sort_by(|a, b| a.name().cmp(b.name()));
        let mut tempids = BTreeMap::new();
        for label in labels {
            if tempids.contains_key(label) {
                return Err(Error::invalid(
                    "new_entities",
                    "temporary label is declared more than once",
                ));
            }
            let allocation = next
                .next_allocation
                .ok_or_else(|| Error::invalid("entity", "allocation space exhausted"))?;
            let id = EntityId::allocated(next.snapshot.store().clone(), allocation)?;
            next.next_allocation = allocation.checked_add(1);
            next.entities.insert(id.clone(), revision);
            tempids.insert(label.clone(), id);
        }
        next.snapshot = SnapshotId::new(
            self.snapshot.store().clone(),
            self.snapshot.lineage().clone(),
            revision,
        );
        Ok((
            next,
            CatalogDelta {
                schema_added,
                tempids,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cardinality, Uniqueness, ValueType};
    use proptest::prelude::*;

    pub(super) fn empty() -> Catalog {
        Catalog::empty(
            "d1.00000000000000000000000000000001.00000000000000000000000000000002.0"
                .parse()
                .unwrap(),
        )
        .unwrap()
    }
    fn definition(name: &str, ty: ValueType) -> AttributeDefinition {
        AttributeDefinition::new(
            name.parse().unwrap(),
            ty,
            Cardinality::One,
            Uniqueness::None,
        )
        .unwrap()
    }

    #[test]
    fn schema_is_historical_and_redeclaration_is_noop() {
        let initial = empty();
        let def = definition("person/name", ValueType::String);
        let (a, delta) = initial.stage(std::slice::from_ref(&def), &[]).unwrap();
        assert_eq!(delta.schema_added, vec![def.clone()]);
        assert!(
            a.schema_at(Revision::ZERO)
                .unwrap()
                .get(def.name())
                .is_none()
        );
        assert_eq!(
            a.schema_at(Revision::new(1)).unwrap().get(def.name()),
            Some(&def)
        );
        let (b, delta) = a.stage(std::slice::from_ref(&def), &[]).unwrap();
        assert!(delta.schema_added.is_empty());
        assert_eq!(b.snapshot.revision(), Revision::new(2));
        assert_eq!(
            a.stage(&[definition("person/name", ValueType::I64)], &[])
                .unwrap_err()
                .code(),
            ErrorCode::ConflictingBatch
        );
        assert_eq!(
            a.stage(&[def.clone(), def], &[]).unwrap_err().code(),
            ErrorCode::InvalidRequest
        );
        assert!(initial.schema.is_empty());
        let new_def = definition("task/title", ValueType::String);
        assert!(
            a.stage(
                &[new_def.clone(), definition("person/name", ValueType::I64)],
                &[]
            )
            .is_err()
        );
        assert!(!a.schema.contains_key(new_def.name()));
        assert_eq!(
            a.schema_at(Revision::new(2)).unwrap_err().code(),
            ErrorCode::InvalidSnapshot
        );
    }

    #[test]
    fn empty_entities_foreign_tokens_and_failed_allocations() {
        let initial = empty();
        let label = TempId::new("empty").unwrap();
        let (a, delta) = initial.stage(&[], std::slice::from_ref(&label)).unwrap();
        let id = &delta.tempids[&label];
        assert!(!a.known_at(id, Revision::ZERO).unwrap());
        assert!(a.known_at(id, Revision::new(1)).unwrap());
        assert!(a.require_known(id).is_ok());
        let fabricated = "e1.00000000000000000000000000000001.0000000000000002"
            .parse()
            .unwrap();
        assert_eq!(
            a.require_known(&fabricated).unwrap_err().code(),
            ErrorCode::UnknownEntity
        );
        assert_eq!(
            initial.require_known(id).unwrap_err().code(),
            ErrorCode::UnknownEntity
        );
        let foreign = id
            .to_string()
            .replace(
                "00000000000000000000000000000001",
                "00000000000000000000000000000003",
            )
            .parse()
            .unwrap();
        assert_eq!(
            a.require_known(&foreign).unwrap_err().code(),
            ErrorCode::UnknownEntity
        );
        assert!(initial.stage(&[], &[label.clone(), label.clone()]).is_err());
        let (_, retry) = initial.stage(&[], &[label]).unwrap();
        assert_eq!(delta.tempids, retry.tempids);
        assert!(initial.entities.is_empty());
    }

    #[test]
    fn last_allocation_is_valid_then_exhausted_without_wrapping() {
        let mut catalog = empty();
        catalog.next_allocation = Some(u64::MAX);
        let (last, delta) = catalog.stage(&[], &[TempId::new("last").unwrap()]).unwrap();
        assert_eq!(
            delta.tempids.values().next().unwrap().to_string(),
            "e1.00000000000000000000000000000001.ffffffffffffffff"
        );
        assert!(
            last.stage(&[], &[TempId::new("overflow").unwrap()])
                .is_err()
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        #[test]
        fn distinct_labels_allocate_distinct_empty_identities(count in 0_usize..64) {
            let labels: Vec<_> = (0..count).map(|i| TempId::new(format!("e{i}")).unwrap()).collect();
            let (catalog, delta) = empty().stage(&[], &labels).unwrap();
            let ids: BTreeSet<_> = delta.tempids.values().collect();
            prop_assert_eq!(ids.len(), count);
            for id in ids { prop_assert!(!catalog.known_at(id, Revision::ZERO).unwrap()); prop_assert!(catalog.known_at(id, Revision::new(1)).unwrap()); }
        }
    }
}
