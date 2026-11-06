//! Lookups resolve an entity by a unique attribute's value at a pinned view.

use factlite::{
    AttributeDefinition, Cardinality, ErrorCode, Store, TempId, Tx, Uniqueness, Value, ValueType,
};

fn definition(name: &str, ty: ValueType, unique: Uniqueness) -> AttributeDefinition {
    AttributeDefinition::new(name.parse().unwrap(), ty, Cardinality::One, unique).unwrap()
}

#[test]
fn lookups_follow_assignment_retraction_and_transfer_at_each_revision() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("lookup.sqlite")).unwrap();
    let key = "run/key".parse().unwrap();
    let a = TempId::new("a").unwrap();
    let b = TempId::new("b").unwrap();
    let created = store
        .commit(
            Tx::builder()
                .define_attribute(definition(
                    "run/key",
                    ValueType::String,
                    Uniqueness::UniqueOne,
                ))
                .define_attribute(definition(
                    "run/status",
                    ValueType::String,
                    Uniqueness::None,
                ))
                .new_entity(a.clone())
                .new_entity(b.clone())
                .build()
                .unwrap(),
        )
        .unwrap();
    let (a, b) = (created.tempids[&a].clone(), created.tempids[&b].clone());
    let value = Value::String("A/run-3".into());
    let lookup = |snapshot| store.db_at(snapshot).unwrap().lookup(&key, &value).unwrap();
    assert_eq!(lookup(&created.after), None);
    let assigned = store
        .commit(
            Tx::builder()
                .assert(a.clone(), key.clone(), value.clone())
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(lookup(&assigned.after), Some(a.clone()));
    // An atomic transfer moves the value to another entity.
    let moved = store
        .commit(
            Tx::builder()
                .retract(a.clone(), key.clone(), value.clone())
                .assert(b.clone(), key.clone(), value.clone())
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(lookup(&moved.after), Some(b.clone()));
    let removed = store
        .commit(
            Tx::builder()
                .retract(b.clone(), key.clone(), value.clone())
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(lookup(&removed.after), None);
    // Historical views keep their answers.
    assert_eq!(lookup(&assigned.after), Some(a));
    assert_eq!(lookup(&moved.after), Some(b));
    let db = store.db().unwrap();
    assert_eq!(
        db.lookup(&key, &Value::String("absent".into())).unwrap(),
        None
    );
}

#[test]
fn lookups_require_a_visible_unique_attribute_of_the_value_type() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("errors.sqlite")).unwrap();
    let before = store.head().unwrap();
    store
        .commit(
            Tx::builder()
                .define_attribute(definition(
                    "run/key",
                    ValueType::String,
                    Uniqueness::UniqueOne,
                ))
                .define_attribute(definition(
                    "run/status",
                    ValueType::String,
                    Uniqueness::None,
                ))
                .build()
                .unwrap(),
        )
        .unwrap();
    let db = store.db().unwrap();
    let text = Value::String("x".into());
    let code =
        |name: &str, value: &Value| db.lookup(&name.parse().unwrap(), value).unwrap_err().code();
    assert_eq!(code("run/nope", &text), ErrorCode::UnknownAttribute);
    assert_eq!(code("run/status", &text), ErrorCode::InvalidRequest);
    assert_eq!(code("run/key", &Value::I64(1)), ErrorCode::TypeMismatch);
    // An attribute defined later is unknown at an earlier view.
    assert_eq!(
        store
            .db_at(&before)
            .unwrap()
            .lookup(&"run/key".parse().unwrap(), &text)
            .unwrap_err()
            .code(),
        ErrorCode::UnknownAttribute
    );
}
