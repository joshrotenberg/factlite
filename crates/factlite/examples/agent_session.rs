//! Conventions for agents that share a Factlite store. See `docs/agents.md`.
//!
//! Two agents, Ada and Grace, record notes and one current decision per
//! topic. Every step is checked. Times are supplied by the caller.
//!
//! ```sh
//! cargo run -p factlite --example agent_session
//! ```

use factlite::{
    AttrName, AttributeDefinition, Cardinality, Db, Direction, EntityId, ErrorCode, FactFilter,
    Keyword, Operation, Order, Params, Pattern, PulledValue, Query, QueryName, Result, Revision,
    Store, TempId, Term, Tx, TxBuilder, Uniqueness, Value, ValueType, wire,
};

fn attr(name: &str) -> AttrName {
    name.parse().expect("valid attribute name")
}
fn var(name: &str) -> QueryName {
    QueryName::new(name).expect("valid query name")
}
fn string(text: &str) -> Value {
    Value::String(text.into())
}
fn keyword(text: &str) -> Value {
    Value::Keyword(Keyword::new(text).expect("valid keyword"))
}

/// Who is writing. Factlite records neither an author nor a time, so every
/// commit carries both as metadata.
struct Writer {
    agent: &'static str,
    session: &'static str,
}

impl Writer {
    /// Starts a keyed request stamped with this writer and `now`.
    ///
    /// The key names the intent: the session and the step within it. A retry
    /// of the same step returns the first commit's report instead of
    /// committing again.
    fn step(&self, step: &str, now: i64) -> TxBuilder {
        Tx::builder()
            .idempotency_key(format!("{}/{step}", self.session))
            .metadata("agent", string(self.agent))
            .metadata("session", string(self.session))
            .metadata("at", Value::Inst(now))
    }
}

/// Declares the schema. Any agent may call this, so the request is the same
/// for all of them: a fixed key and no author or time. A second call returns
/// the first call's report.
fn define_schema(store: &Store) -> Result<()> {
    let mut tx = Tx::builder().idempotency_key("memory/schema/v1");
    for (name, ty, unique) in [
        ("topic/name", ValueType::Keyword, Uniqueness::UniqueOne),
        // One value at a time: the decision in force for the topic.
        ("topic/decision", ValueType::Ref, Uniqueness::None),
        ("note/id", ValueType::String, Uniqueness::UniqueOne),
        ("note/topic", ValueType::Ref, Uniqueness::None),
        ("note/text", ValueType::String, Uniqueness::None),
        // A pointer to material kept outside the store, never the material.
        ("note/source", ValueType::String, Uniqueness::None),
        ("note/at", ValueType::Inst, Uniqueness::None),
    ] {
        tx = tx.define_attribute(AttributeDefinition::new(
            attr(name),
            ty,
            Cardinality::One,
            unique,
        )?);
    }
    store.commit(tx.build()?)?;
    Ok(())
}

/// Returns the topic, creating it if `view` does not have it. Unique values
/// do not upsert, so this is a lookup, a create, and a second lookup at the
/// head if another writer created the topic after `view` was taken.
fn topic(store: &Store, view: &Db, writer: &Writer, name: &str, now: i64) -> Result<EntityId> {
    let key = keyword(name);
    if let Some(found) = view.lookup(&attr("topic/name"), &key)? {
        return Ok(found);
    }
    let label = TempId::new("topic")?;
    let create = writer
        .step(&format!("topic/{name}"), now)
        .new_entity(label.clone())
        .assert(label.clone(), attr("topic/name"), key.clone())
        .build()?;
    match store.commit(create) {
        Ok(report) => Ok(report.tempids[&label].clone()),
        Err(error) if error.code() == ErrorCode::UniqueViolation => Ok(store
            .db()?
            .lookup(&attr("topic/name"), &key)?
            .expect("the unique value has an owner")),
        Err(error) => Err(error),
    }
}

/// Records a note and makes it the topic's decision, provided the decision in
/// force is still `supersedes`. Returns `None` if someone else decided first.
fn decide(
    store: &Store,
    writer: &Writer,
    topic: &EntityId,
    supersedes: Option<&EntityId>,
    (id, text, source): (&str, &str, &str),
    now: i64,
) -> Result<Option<EntityId>> {
    let note = TempId::new("note")?;
    let prior = supersedes.map(|note| Value::Ref(note.clone()));
    let tx = writer
        .step(&format!("decide/{id}"), now)
        .expect(
            topic.clone(),
            attr("topic/decision"),
            prior.into_iter().collect(),
        )
        .new_entity(note.clone())
        .assert(note.clone(), attr("note/id"), string(id))
        .assert(note.clone(), attr("note/topic"), Value::Ref(topic.clone()))
        .assert(note.clone(), attr("note/text"), string(text))
        .assert(note.clone(), attr("note/source"), string(source))
        .assert(note.clone(), attr("note/at"), Value::Inst(now))
        .assert(
            topic.clone(),
            attr("topic/decision"),
            factlite::ValueTerm::TempRef(note.clone()),
        )
        .build()?;
    match store.commit(tx) {
        Ok(report) => Ok(Some(report.tempids[&note].clone())),
        Err(error) if error.code() == ErrorCode::PreconditionFailed => Ok(None),
        Err(error) => Err(error),
    }
}

/// The decision in force for `topic` in this view, with its text.
fn decision(db: &Db, topic: &EntityId) -> Result<Option<(EntityId, String)>> {
    let pulled = db.pull(topic, &[attr("topic/decision")])?;
    let Some(PulledValue::One(Value::Ref(note))) = pulled
        .as_ref()
        .and_then(|topic| topic.attributes.get(&attr("topic/decision")))
    else {
        return Ok(None);
    };
    let text = db.pull(note, &[attr("note/text")])?.expect("a note exists");
    let PulledValue::One(Value::String(text)) = &text.attributes[&attr("note/text")] else {
        unreachable!("note/text is a single string");
    };
    Ok(Some((note.clone(), text.clone())))
}

/// The most recent notes on `topic` in this view, newest first.
fn recent_notes(db: &Db, topic: &EntityId, limit: u64) -> Result<Vec<String>> {
    let pattern = |attribute: &str, value: Term| Pattern {
        entity: Term::Var(var("note")),
        attribute: attr(attribute),
        value,
    };
    let query = Query {
        inputs: [(var("topic"), ValueType::Ref)].into(),
        patterns: vec![
            pattern("note/topic", Term::Param(var("topic"))),
            pattern("note/at", Term::Var(var("at"))),
            pattern("note/id", Term::Var(var("id"))),
        ],
        select: vec![var("at"), var("id")],
        order: vec![
            Order {
                field: var("at"),
                direction: Direction::Desc,
            },
            Order {
                field: var("id"),
                direction: Direction::Asc,
            },
        ],
        limit: Some(limit),
        ..Query::default()
    };
    let params: Params = [(var("topic"), Value::Ref(topic.clone()))].into();
    Ok(db
        .query(&query, &params)?
        .rows
        .into_iter()
        .map(|row| match &row[1] {
            Value::String(id) => id.clone(),
            other => unreachable!("note/id is a string, not {other:?}"),
        })
        .collect())
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    // One file per scope. Deleting the file is the only way to forget it.
    let store = Store::open(directory.path().join("project-memory.sqlite"))?;
    define_schema(&store)?;
    let head = store.head()?;
    define_schema(&store)?;
    assert_eq!(store.head()?, head);
    let ada = Writer {
        agent: "ada",
        session: "session-41",
    };
    let grace = Writer {
        agent: "grace",
        session: "session-42",
    };

    // Both agents look for the topic in the same empty view and both try to
    // create it. One create commits; the other is refused and finds the first.
    let empty = store.db()?;
    let citations = topic(&store, &empty, &ada, "citations", 1_000)?;
    let head = store.head()?;
    assert_eq!(
        topic(&store, &empty, &grace, "citations", 1_001)?,
        citations
    );
    assert_eq!(store.head()?, head);
    assert_eq!(
        topic(&store, &store.db()?, &grace, "citations", 1_002)?,
        citations
    );

    // Ada loads her context: one snapshot, then every read at that snapshot.
    let ada_view = store.db()?;
    assert_eq!(decision(&ada_view, &citations)?, None);

    // Grace decides first. The note points at its evidence; it does not copy it.
    let by_path = decide(
        &store,
        &grace,
        &citations,
        None,
        (
            "cite-by-path",
            "Cite a turn by file path",
            "run://41/turn/7",
        ),
        2_000,
    )?
    .expect("no decision was in force");

    // Ada's view has not moved, so her reads still agree with each other.
    assert_eq!(decision(&ada_view, &citations)?, None);
    // Her write was planned against that view. It is refused, and commits
    // nothing: there is never a second decision in force.
    let head = store.head()?;
    let ada_note = (
        "cite-by-session",
        "Cite a turn by session id",
        "run://42/turn/3",
    );
    assert_eq!(
        decide(&store, &ada, &citations, None, ada_note, 3_000)?,
        None
    );
    assert_eq!(store.head()?, head);

    // She asks what changed since her snapshot. The answer names the writer.
    let since = store.db()?.transactions(ada_view.snapshot().revision())?;
    assert_eq!(since.len(), 1);
    assert_eq!(since[0].report.metadata["agent"], string("grace"));
    assert_eq!(since[0].report.metadata["at"], Value::Inst(2_000));

    // She reads again and supersedes the decision she now knows about.
    let (in_force, text) = decision(&store.db()?, &citations)?.expect("Grace decided");
    assert_eq!(
        (in_force.clone(), text.as_str()),
        (by_path.clone(), "Cite a turn by file path")
    );
    let by_session = decide(&store, &ada, &citations, Some(&in_force), ada_note, 3_500)?
        .expect("the decision Ada read is still in force");

    // Her harness retries the step. The same key and request return the first
    // report: no second note, no new revision.
    let head = store.head()?;
    assert_eq!(
        decide(&store, &ada, &citations, Some(&in_force), ada_note, 3_500)?,
        Some(by_session.clone())
    );
    assert_eq!(store.head()?, head);

    // Handoff: Ada passes a token, not a summary. It is a string that names
    // this store and revision, and it survives reopening and backups.
    let token = wire::encode_snapshot(&head);
    let handed_over = store.db_at(&wire::decode_snapshot(&token)?)?;
    assert_eq!(
        decision(&handed_over, &citations)?.map(|(note, _)| note),
        Some(by_session.clone())
    );
    assert_eq!(
        recent_notes(&handed_over, &citations, 10)?,
        ["cite-by-session", "cite-by-path"]
    );
    assert_eq!(
        recent_notes(&handed_over, &citations, 1)?,
        ["cite-by-session"]
    );

    // Nothing was overwritten. The earlier decision is readable as it stood,
    // and the topic's history lists every decision in order.
    let earlier = store.db_at(&since[0].report.after)?;
    assert_eq!(
        decision(&earlier, &citations)?.map(|(note, _)| note),
        Some(by_path.clone())
    );
    let filter = FactFilter {
        entity: Some(citations),
        attribute: Some(attr("topic/decision")),
        value: None,
    };
    let decided: Vec<Value> = store
        .db()?
        .history(filter, Revision::ZERO)?
        .into_iter()
        .filter(|event| event.operation == Operation::Assert)
        .map(|event| event.fact.value)
        .collect();
    assert_eq!(decided, [Value::Ref(by_path), Value::Ref(by_session)]);
    println!("{token}");
    Ok(())
}
