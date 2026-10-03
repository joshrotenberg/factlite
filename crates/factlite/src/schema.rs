use std::{collections::BTreeMap, fmt, str::FromStr};

use crate::{Error, Result, ValueType};

/// An immutable namespace/name pair with no whitespace or control characters.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AttrName(String);

impl AttrName {
    /// Validates a qualified attribute name without normalization.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        let valid = name
            .split_once('/')
            .is_some_and(|(ns, local)| !ns.is_empty() && !local.is_empty() && !local.contains('/'));
        if !valid || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(Error::invalid(
                "attribute",
                "attribute must be namespace/name without whitespace or control characters",
            ));
        }
        Ok(Self(name))
    }
    /// Returns its exact qualified spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for AttrName {
    type Err = Error;
    fn from_str(name: &str) -> Result<Self> {
        Self::new(name)
    }
}
impl fmt::Display for AttrName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Number of distinct effective values allowed for an entity/attribute.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cardinality {
    /// At most one value; assertions replace the prior value.
    One,
    /// A set of zero or more values.
    Many,
}

/// Whether a typed value can be owned by more than one entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Uniqueness {
    /// Multiple owners permitted.
    None,
    /// One owner; requires cardinality one and never merges entities.
    UniqueOne,
}

/// A validated, immutable attribute definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeDefinition {
    name: AttrName,
    value_type: ValueType,
    cardinality: Cardinality,
    uniqueness: Uniqueness,
}

impl AttributeDefinition {
    /// Rejects uniqueness/cardinality combinations outside the v1 contract.
    pub fn new(
        name: AttrName,
        value_type: ValueType,
        cardinality: Cardinality,
        uniqueness: Uniqueness,
    ) -> Result<Self> {
        if uniqueness == Uniqueness::UniqueOne && cardinality != Cardinality::One {
            return Err(Error::invalid(
                "schema",
                "unique_one requires cardinality one",
            ));
        }
        Ok(Self {
            name,
            value_type,
            cardinality,
            uniqueness,
        })
    }
    /// Qualified name.
    pub fn name(&self) -> &AttrName {
        &self.name
    }
    /// Exact value type.
    pub fn value_type(&self) -> ValueType {
        self.value_type
    }
    /// Set cardinality.
    pub fn cardinality(&self) -> Cardinality {
        self.cardinality
    }
    /// Ownership constraint.
    pub fn uniqueness(&self) -> Uniqueness {
        self.uniqueness
    }
}

/// An owned schema projection. Committed definitions cannot be mutated.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Schema {
    pub(crate) definitions: BTreeMap<AttrName, AttributeDefinition>,
}

impl Schema {
    /// Finds a definition by exact qualified name.
    pub fn get(&self, name: &AttrName) -> Option<&AttributeDefinition> {
        self.definitions.get(name)
    }
    /// Enumerates definitions in qualified-name order.
    pub fn attributes(&self) -> impl Iterator<Item = &AttributeDefinition> {
        self.definitions.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn name_and_uniqueness_boundaries() {
        for name in [
            "",
            "name",
            "/name",
            "person/",
            "person/name/x",
            "person/full name",
            "p/x\0",
        ] {
            assert!(AttrName::new(name).is_err(), "{name:?}");
        }
        assert_eq!(AttrName::new("人/名前").unwrap().as_str(), "人/名前");
        assert!(
            AttributeDefinition::new(
                "x/a".parse().unwrap(),
                ValueType::String,
                Cardinality::Many,
                Uniqueness::UniqueOne
            )
            .is_err()
        );
        assert!(Schema::default().attributes().next().is_none());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        #[test]
        fn qualified_names_roundtrip(ns in "[a-zA-Z0-9_.-]{1,24}", local in "[a-zA-Z0-9_.-]{1,24}") {
            let spelling = format!("{ns}/{local}"); let name = AttrName::new(spelling.clone()).unwrap();
            prop_assert_eq!(name.to_string(), spelling);
            prop_assert_eq!(name.as_str().parse::<AttrName>().unwrap(), name);
        }
    }
}
