//! Runnable first-use workflow; every read deliberately names a pinned basis.

use factlite::{
    AttrName, AttributeDefinition, Cardinality, Db, Direction, ErrorCode, FactFilter, Order,
    Params, Pattern, PulledValue, Query, QueryName, Revision, Store, TempId, Term, Tx, Uniqueness,
    Value, ValueTerm, ValueType, wire,
};

fn attr(name: &str) -> AttrName {
    name.parse().expect("valid example attribute")
}
fn name(name: &str) -> QueryName {
    QueryName::new(name).expect("valid example query name")
}
fn string(value: &str) -> Value {
    Value::String(value.into())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A fresh, automatically cleaned directory makes repeated runs safe.
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("tasks.sqlite");
    let store = Store::open(&path)?;
    let person_label = TempId::new("person")?;
    let task_label = TempId::new("task")?;
    let mut create = Tx::builder().idempotency_key("create-task-v1");
    for (attribute, ty) in [
        ("person/name", ValueType::String),
        ("task/title", ValueType::String),
        ("task/status", ValueType::String),
        ("task/owner", ValueType::Ref),
    ] {
        create = create.define_attribute(AttributeDefinition::new(
            attr(attribute),
            ty,
            Cardinality::One,
            Uniqueness::None,
        )?);
    }
    let create = create
        .new_entity(task_label.clone())
        .new_entity(person_label.clone())
        .assert(
            task_label.clone(),
            attr("task/title"),
            string("Design Factlite"),
        )
        .assert(task_label.clone(), attr("task/status"), string("todo"))
        // Forward references resolve against declared labels in the same batch.
        .assert(
            task_label.clone(),
            attr("task/owner"),
            ValueTerm::TempRef(person_label.clone()),
        )
        .assert(person_label.clone(), attr("person/name"), string("Ada"))
        .build()?;
    let created = store.commit(create.clone())?;
    let task = created.tempids[&task_label].clone();
    let person = created.tempids[&person_label].clone();
    let a = store.db_at(&created.after)?;
    let pulled = a
        .pull(&task, &[attr("task/owner"), attr("task/status")])?
        .unwrap();
    let PulledValue::One(Value::Ref(owner)) = &pulled.attributes[&attr("task/owner")] else {
        panic!("owner must be a single reference");
    };
    assert_eq!(owner, &person);
    // Explicit traversal on A cannot accidentally read the owner's later state.
    assert_eq!(
        a.pull(owner, &[attr("person/name")])?.unwrap().attributes[&attr("person/name")],
        PulledValue::One(string("Ada"))
    );

    let start = Tx::builder()
        .idempotency_key("start-task-v1")
        .expect(task.clone(), attr("task/status"), vec![string("todo")])
        .assert(task.clone(), attr("task/status"), string("doing"))
        .metadata("source", string("rust-example"))
        .build()?;
    let started = store.commit(start.clone())?;
    let b = store.db_at(&started.after)?;
    let query = Query {
        inputs: [(name("wanted"), ValueType::String)].into(),
        patterns: vec![
            Pattern {
                entity: Term::Var(name("task")),
                attribute: attr("task/status"),
                value: Term::Param(name("wanted")),
            },
            Pattern {
                entity: Term::Var(name("task")),
                attribute: attr("task/title"),
                value: Term::Var(name("title")),
            },
            Pattern {
                entity: Term::Var(name("task")),
                attribute: attr("task/owner"),
                value: Term::Var(name("owner")),
            },
            Pattern {
                entity: Term::Var(name("owner")),
                attribute: attr("person/name"),
                value: Term::Var(name("name")),
            },
        ],
        select: vec![name("task"), name("title"), name("name")],
        order: vec![
            Order {
                field: name("title"),
                direction: Direction::Asc,
            },
            Order {
                field: name("task"),
                direction: Direction::Asc,
            },
        ],
        limit: Some(20),
        ..Query::default()
    };
    let params: Params = [(name("wanted"), string("doing"))].into();
    assert!(a.query(&query, &params)?.rows.is_empty());
    let result = b.query(&query, &params)?;
    assert_eq!(
        result.rows,
        vec![vec![
            Value::Ref(task.clone()),
            string("Design Factlite"),
            string("Ada")
        ]]
    );

    let stale = Tx::builder()
        .idempotency_key("finish-task-v1")
        .expect(task.clone(), attr("task/status"), vec![string("todo")])
        .assert(task, attr("task/status"), string("done"))
        .build()?;
    let failure = store.commit(stale).unwrap_err();
    assert_eq!(failure.code(), ErrorCode::PreconditionFailed);
    assert_eq!(store.head()?, started.after);
    // A new no-change transaction advances head; an exact retry does not.
    let latest = store.commit(Tx::default())?;
    assert!(latest.effective_delta.is_empty());
    assert_eq!(store.commit(start.clone())?, started);
    assert_eq!(store.head()?, latest.after);
    let fence = store
        .commit(Tx::builder().expect_head(b.snapshot().clone()).build()?)
        .unwrap_err();
    assert_eq!(fence.code(), ErrorCode::HeadMismatch);

    // Save a portable token, not a live handle or an unqualified revision.
    let token = wire::decode_snapshot(&wire::encode_snapshot(a.snapshot()))?;
    let backup_path = directory.path().join("backup.sqlite");
    assert_eq!(store.backup(&backup_path)?, latest.after);
    let backup = Db::open_read_only_at(&backup_path, &token)?;
    assert_eq!(
        backup.facts(FactFilter::default())?,
        a.facts(FactFilter::default())?
    );
    assert_eq!(store.db()?.transactions(Revision::ZERO)?.len(), 3);
    assert_eq!(b.history(FactFilter::default(), Revision::ZERO)?.len(), 6);
    drop(store);
    assert_eq!(b.query(&query, &params)?, result); // Db owns durable access.
    drop((a, b, backup)); // All handles close before reopening the original.
    let reopened = Store::open(&path)?;
    assert_eq!(
        reopened.db_at(&token)?.query(&query, &params)?.rows.len(),
        0
    );
    assert_eq!(reopened.commit(create)?, created);
    assert_eq!(reopened.commit(start)?, started);
    assert_eq!(reopened.head()?, latest.after);
    println!("{}", wire::encode_query_result(&result));
    Ok(())
}
