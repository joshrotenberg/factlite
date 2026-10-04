//! The handoff's Ada/task graph through durable public APIs.
//! Exact retry and joined query assertions are added when those APIs land.

use factlite::{
    AttrName, AttributeDefinition, Cardinality, CommitOutcome, Db, EntityId, ErrorCode,
    ErrorContext, FactFilter, Operation, PulledValue, Revision, Store, TempId, Tx, TxReport,
    Uniqueness, Value, ValueTerm, ValueType,
};

fn attr(name: &str) -> AttrName {
    name.parse().unwrap()
}
fn string(value: &str) -> Value {
    Value::String(value.into())
}
fn create(store: &Store) -> TxReport {
    let person = TempId::new("person").unwrap();
    let task = TempId::new("task").unwrap();
    let mut tx = Tx::builder();
    for (name, ty) in [
        ("person/name", ValueType::String),
        ("task/title", ValueType::String),
        ("task/status", ValueType::String),
        ("task/owner", ValueType::Ref),
    ] {
        tx = tx.define_attribute(
            AttributeDefinition::new(attr(name), ty, Cardinality::One, Uniqueness::None).unwrap(),
        );
    }
    store
        .commit(
            tx.new_entity(person.clone())
                .new_entity(task.clone())
                .assert(person.clone(), attr("person/name"), string("Ada"))
                .assert(task.clone(), attr("task/title"), string("Design Factlite"))
                .assert(task.clone(), attr("task/status"), string("todo"))
                .assert(task, attr("task/owner"), ValueTerm::TempRef(person))
                .metadata("source", string("design-example"))
                .build()
                .unwrap(),
        )
        .unwrap()
}
fn update(task: &EntityId, expected: &str, next: &str) -> Tx {
    Tx::builder()
        .expect(task.clone(), attr("task/status"), vec![string(expected)])
        .assert(task.clone(), attr("task/status"), string(next))
        .metadata("source", string("design-example"))
        .build()
        .unwrap()
}
fn check_view(db: &Db, task: &EntityId, person: &EntityId, status: &str) {
    let projected = db
        .pull(
            task,
            &[attr("task/title"), attr("task/status"), attr("task/owner")],
        )
        .unwrap()
        .unwrap();
    assert_eq!(&projected.id, task);
    assert_eq!(projected.attributes.len(), 3);
    assert_eq!(
        projected.attributes[&attr("task/title")],
        PulledValue::One(string("Design Factlite"))
    );
    assert_eq!(
        projected.attributes[&attr("task/status")],
        PulledValue::One(string(status))
    );
    assert_eq!(
        projected.attributes[&attr("task/owner")],
        PulledValue::One(Value::Ref(person.clone()))
    );
    // Explicit traversal reuses the same Db, keeping the historical basis.
    let owner = db.pull(person, &[attr("person/name")]).unwrap().unwrap();
    assert_eq!(
        owner.attributes[&attr("person/name")],
        PulledValue::One(string("Ada"))
    );
    assert_eq!(db.facts(FactFilter::default()).unwrap().len(), 4);
    assert_eq!(db.schema().unwrap().attributes().count(), 4);
}

#[test]
fn ada_task_a_b_close_reopen_preserves_every_owned_read_and_report() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("demo.sqlite");
    let store = Store::open(&path).unwrap();
    let report_a = create(&store);
    let person = report_a.tempids[&TempId::new("person").unwrap()].clone();
    let task = report_a.tempids[&TempId::new("task").unwrap()].clone();
    assert_eq!(report_a.before.revision(), Revision::ZERO);
    assert_eq!(report_a.after.revision(), Revision::new(1));
    assert_eq!(
        report_a.created_entities,
        vec![person.clone(), task.clone()]
    );
    assert_eq!(report_a.schema_added.len(), 4);
    assert_eq!(report_a.effective_delta.len(), 4);
    assert!(
        report_a
            .effective_delta
            .iter()
            .all(|delta| delta.operation == Operation::Assert)
    );
    assert_eq!(report_a.metadata["source"], string("design-example"));
    let a = store.db_at(&report_a.after).unwrap();
    check_view(&a, &task, &person, "todo");
    let schema_a = a.schema().unwrap();
    let facts_a = a.facts(FactFilter::default()).unwrap();
    let pull_a = a.pull(&task, &[attr("task/status")]).unwrap();
    let history_a = a.history(FactFilter::default(), Revision::ZERO).unwrap();
    let transactions_a = a.transactions(Revision::ZERO).unwrap();
    let report_b = store.commit(update(&task, "todo", "doing")).unwrap();
    assert_eq!(report_b.before, report_a.after);
    assert_eq!(report_b.after.revision(), Revision::new(2));
    assert!(report_b.tempids.is_empty());
    assert!(report_b.created_entities.is_empty());
    assert!(report_b.schema_added.is_empty());
    assert_eq!(report_b.effective_delta.len(), 2);
    assert_eq!(report_b.effective_delta[0].operation, Operation::Retract);
    assert_eq!(report_b.effective_delta[0].fact.value, string("todo"));
    assert_eq!(report_b.effective_delta[1].operation, Operation::Assert);
    assert_eq!(report_b.effective_delta[1].fact.value, string("doing"));
    let b = store.db_at(&report_b.after).unwrap();
    check_view(&a, &task, &person, "todo");
    check_view(&b, &task, &person, "doing");
    let filter = FactFilter {
        entity: Some(task.clone()),
        attribute: Some(attr("task/status")),
        value: None,
    };
    let status_history = b.history(filter.clone(), Revision::ZERO).unwrap();
    assert_eq!(
        status_history
            .iter()
            .map(|event| (
                event.revision.get(),
                event.operation,
                event.fact.value.clone()
            ))
            .collect::<Vec<_>>(),
        vec![
            (1, Operation::Assert, string("todo")),
            (2, Operation::Retract, string("todo")),
            (2, Operation::Assert, string("doing"))
        ]
    );
    assert_eq!(a.schema().unwrap(), schema_a);
    assert_eq!(a.facts(FactFilter::default()).unwrap(), facts_a);
    assert_eq!(a.pull(&task, &[attr("task/status")]).unwrap(), pull_a);
    assert_eq!(
        a.history(FactFilter::default(), Revision::ZERO).unwrap(),
        history_a
    );
    assert_eq!(a.transactions(Revision::ZERO).unwrap(), transactions_a);
    let facts_b = b.facts(FactFilter::default()).unwrap();
    drop((a, b, store));
    // All handles really close before recovery; these are the original tokens.
    let reopened = Store::open(&path).unwrap();
    let a = reopened.db_at(&report_a.after).unwrap();
    let b = reopened.db_at(&report_b.after).unwrap();
    check_view(&a, &task, &person, "todo");
    check_view(&b, &task, &person, "doing");
    assert_eq!(a.schema().unwrap(), schema_a);
    assert_eq!(a.facts(FactFilter::default()).unwrap(), facts_a);
    assert_eq!(a.pull(&task, &[attr("task/status")]).unwrap(), pull_a);
    assert_eq!(
        a.history(FactFilter::default(), Revision::ZERO).unwrap(),
        history_a
    );
    assert_eq!(a.transactions(Revision::ZERO).unwrap(), transactions_a);
    assert_eq!(b.facts(FactFilter::default()).unwrap(), facts_b);
    assert_eq!(b.history(filter, Revision::ZERO).unwrap(), status_history);
    assert_eq!(
        b.transactions(Revision::ZERO)
            .unwrap()
            .into_iter()
            .map(|record| record.report)
            .collect::<Vec<_>>(),
        vec![report_a, report_b]
    );
}

#[test]
fn fixture_conditions_reject_stale_changes_allow_unrelated_writes_and_fence_head() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("conditions.sqlite")).unwrap();
    let a = create(&store);
    let task = a.tempids[&TempId::new("task").unwrap()].clone();
    let person = a.tempids[&TempId::new("person").unwrap()].clone();
    let b = store.commit(update(&task, "todo", "doing")).unwrap();
    let before_failure = store.db().unwrap().facts(FactFilter::default()).unwrap();
    let failed = store.commit(update(&task, "todo", "done")).unwrap_err();
    assert_eq!(failed.code(), ErrorCode::PreconditionFailed);
    assert_eq!(failed.commit_outcome(), Some(CommitOutcome::RolledBack));
    assert!(
        matches!(failed.context(),ErrorContext::Precondition { entity,attribute,expected,actual } if entity == &task && attribute == &attr("task/status") && expected == &vec![string("todo")] && actual == &vec![string("doing")])
    );
    assert_eq!(store.head().unwrap(), b.after);
    assert_eq!(
        store.db().unwrap().facts(FactFilter::default()).unwrap(),
        before_failure
    );
    let unrelated = store
        .commit(
            Tx::builder()
                .assert(person.clone(), attr("person/name"), string("Ada Lovelace"))
                .build()
                .unwrap(),
        )
        .unwrap();
    let fenced = Tx::builder()
        .expect_head(b.after.clone())
        .expect(task.clone(), attr("task/status"), vec![string("doing")])
        .assert(task.clone(), attr("task/status"), string("done"))
        .build()
        .unwrap();
    let failed = store.commit(fenced).unwrap_err();
    assert_eq!(failed.code(), ErrorCode::HeadMismatch);
    assert!(
        matches!(failed.context(),ErrorContext::HeadMismatch { expected,actual } if expected == &b.after && actual == &unrelated.after)
    );
    assert_eq!(store.head().unwrap(), unrelated.after);
    let done = store.commit(update(&task, "doing", "done")).unwrap();
    assert_eq!(done.before, unrelated.after);
    assert_eq!(
        store
            .db_at(&b.after)
            .unwrap()
            .pull(&person, &[attr("person/name")])
            .unwrap()
            .unwrap()
            .attributes[&attr("person/name")],
        PulledValue::One(string("Ada"))
    );
    assert_eq!(
        store
            .db()
            .unwrap()
            .transactions(Revision::ZERO)
            .unwrap()
            .len(),
        4
    );
    let noop = store.commit(Tx::default()).unwrap();
    assert!(noop.effective_delta.is_empty());
    assert_eq!(
        store
            .commit(Tx::builder().expect_head(done.after).build().unwrap())
            .unwrap_err()
            .code(),
        ErrorCode::HeadMismatch
    );
    assert_eq!(store.head().unwrap(), noop.after);
}
