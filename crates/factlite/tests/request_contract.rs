//! Builder/public request parity without storage access.

use factlite::{Operation, TempId, Tx, Value, ValueTerm};

#[test]
fn related_entities_can_be_described_before_allocation() {
    let person = TempId::new("person").unwrap();
    let task = TempId::new("task").unwrap();
    let request = Tx::builder()
        .new_entity(person.clone())
        .new_entity(task.clone())
        .assert(
            task,
            "task/owner".parse().unwrap(),
            ValueTerm::TempRef(person),
        )
        .metadata("source", Value::String("request-contract".into()))
        .idempotency_key("create-v1")
        .build()
        .unwrap();
    assert_eq!(request.version(), 1);
    assert_eq!(request.idempotency_key(), Some("create-v1"));
    assert_eq!(request.changes()[0].operation, Operation::Assert);
    assert!(request.schema().is_empty());
    assert!(request.expected_head().is_none());
    assert_eq!(
        request.metadata()["source"],
        Value::String("request-contract".into())
    );
}
