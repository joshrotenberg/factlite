//! Public immutable-definition contracts.

use factlite::{
    AttrName, AttributeDefinition, Cardinality, ErrorCode, Schema, Uniqueness, ValueType,
};

#[test]
fn definitions_expose_exact_types_and_validate_ownership_constraints() {
    let name = AttrName::new("person/name").unwrap();
    let definition = AttributeDefinition::new(
        name.clone(),
        ValueType::String,
        Cardinality::One,
        Uniqueness::UniqueOne,
    )
    .unwrap();
    assert_eq!(definition.name(), &name);
    assert_eq!(definition.value_type(), ValueType::String);
    assert_eq!(definition.cardinality(), Cardinality::One);
    assert_eq!(definition.uniqueness(), Uniqueness::UniqueOne);
    assert_eq!(
        AttributeDefinition::new(
            name,
            ValueType::String,
            Cardinality::Many,
            Uniqueness::UniqueOne
        )
        .unwrap_err()
        .code(),
        ErrorCode::InvalidRequest
    );
    assert!(Schema::default().attributes().next().is_none());
}
