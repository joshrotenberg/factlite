//! Dispatch against the library: every call made through `Dispatcher::call`
//! must give the wire output, or the wire error, that the same typed request
//! gives through the library on the same file.
//!
//! The dispatcher is the only writer. A second library handle on the file
//! reads at the basis each reply names, and repeats each refused commit to
//! get the library's own error.

use factlite::{
    AttrName, AttributeDefinition, Cardinality, Comparison, Db, Direction, EntityId, EntityTerm,
    FactFilter, Order, Params, Pattern, Predicate, Query, QueryName, Revision, SnapshotId, Store,
    TempId, Term, Tx, Uniqueness, Value, ValueTerm, ValueType, wire,
};
use factlite_rpc::{DispatchOptions, Dispatcher, FACTLITE_ERROR, Failure};
use proptest::prelude::*;
use serde_json::{Value as Json, json};

fn attr(name: &str) -> AttrName {
    name.parse().unwrap()
}
fn parse(text: &str) -> Json {
    serde_json::from_str(text).unwrap()
}
fn name(index: u8) -> Value {
    Value::String(format!("name-{index}"))
}
fn owner(index: u8) -> EntityTerm {
    EntityTerm::Lookup(attr("item/name"), name(index))
}

fn schema() -> Tx {
    let define = |name: &str, ty, cardinality, unique| {
        AttributeDefinition::new(attr(name), ty, cardinality, unique).unwrap()
    };
    Tx::builder()
        .define_attribute(define(
            "item/name",
            ValueType::String,
            Cardinality::One,
            Uniqueness::UniqueOne,
        ))
        .define_attribute(define(
            "item/n",
            ValueType::I64,
            Cardinality::One,
            Uniqueness::None,
        ))
        .define_attribute(define(
            "item/tags",
            ValueType::Keyword,
            Cardinality::Many,
            Uniqueness::None,
        ))
        .define_attribute(define(
            "item/next",
            ValueType::Ref,
            Cardinality::One,
            Uniqueness::None,
        ))
        .idempotency_key("schema")
        .build()
        .unwrap()
}

/// One write. Names are drawn from a small pool so that writes collide:
/// duplicate names, owners that do not exist yet, and retractions of absent
/// values all occur.
#[derive(Clone, Debug)]
enum Write {
    Create {
        item: u8,
        n: i64,
    },
    SetN {
        item: u8,
        n: i64,
    },
    Tag {
        item: u8,
        tag: u8,
        assert: bool,
    },
    Link {
        from: u8,
        to: u8,
    },
    /// Send an earlier request again, byte for byte.
    Repeat {
        earlier: usize,
    },
}

fn writes() -> impl Strategy<Value = Write> {
    let item = 0..4u8;
    prop_oneof![
        3 => (item.clone(), -3..3i64).prop_map(|(item, n)| Write::Create { item, n }),
        2 => (item.clone(), -3..3i64).prop_map(|(item, n)| Write::SetN { item, n }),
        2 => (item.clone(), 0..3u8, any::<bool>())
            .prop_map(|(item, tag, assert)| Write::Tag { item, tag, assert }),
        1 => (item.clone(), item).prop_map(|(from, to)| Write::Link { from, to }),
        1 => (0..8usize).prop_map(|earlier| Write::Repeat { earlier }),
    ]
}

fn request(write: &Write, step: usize, keyed: bool) -> Option<Tx> {
    let builder = match write {
        Write::Create { item, n } => {
            let label = TempId::new("new").unwrap();
            Tx::builder()
                .new_entity(label.clone())
                .assert(label.clone(), attr("item/name"), name(*item))
                .assert(label, attr("item/n"), Value::I64(*n))
        }
        Write::SetN { item, n } => {
            Tx::builder().assert(owner(*item), attr("item/n"), Value::I64(*n))
        }
        Write::Tag { item, tag, assert } => {
            let value = Value::keyword(format!("tag/t{tag}")).unwrap();
            if *assert {
                Tx::builder().assert(owner(*item), attr("item/tags"), value)
            } else {
                Tx::builder().retract(owner(*item), attr("item/tags"), value)
            }
        }
        Write::Link { from, to } => Tx::builder().assert(
            owner(*from),
            attr("item/next"),
            ValueTerm::LookupRef(attr("item/name"), name(*to)),
        ),
        Write::Repeat { .. } => return None,
    };
    let builder = builder.metadata("step", Value::I64(step as i64));
    let builder = if keyed {
        builder.idempotency_key(format!("step-{step}"))
    } else {
        builder
    };
    Some(builder.build().unwrap())
}

/// The reads made after every write, as (method, params without handle and
/// `at`, the same read through the library).
type LibraryRead = Box<dyn Fn(&Db) -> factlite::Result<String>>;

fn reads(pick: &Picks, entities: &[EntityId]) -> Vec<(&'static str, Json, LibraryRead)> {
    let entity = entities
        .get(pick.entity % entities.len().max(1))
        .cloned()
        // An identity of this shape that no store allocated.
        .unwrap_or_else(|| {
            "e1.00000000000000000000000000000001.0000000000000001"
                .parse()
                .unwrap()
        });
    let attributes = ["item/name", "item/n", "item/tags", "item/next", "item/none"];
    let attribute = attr(attributes[pick.attribute % attributes.len()]);
    let value = match pick.value % 4 {
        0 => name((pick.value / 4 % 4) as u8),
        1 => Value::I64((pick.value / 4 % 6) as i64 - 3),
        2 => Value::keyword(format!("tag/t{}", pick.value / 4 % 3)).unwrap(),
        _ => Value::Ref(entity.clone()),
    };
    let filter = FactFilter {
        entity: (pick.filter & 1 != 0).then(|| entity.clone()),
        attribute: (pick.filter & 2 != 0).then(|| attribute.clone()),
        value: (pick.filter & 4 != 0).then(|| value.clone()),
    };
    let after = Revision::new(pick.after);
    let projection: Vec<AttrName> = attributes
        .iter()
        .enumerate()
        .filter(|(index, _)| pick.projection & (1 << index) != 0)
        .map(|(_, name)| attr(name))
        .collect();
    let minimum = Value::I64(pick.minimum);
    let var = |name: &str| Term::Var(QueryName::new(name).unwrap());
    let query = Query {
        inputs: [(QueryName::new("min").unwrap(), ValueType::I64)].into(),
        patterns: vec![
            Pattern {
                entity: var("e"),
                attribute: attr("item/n"),
                value: var("n"),
            },
            Pattern {
                entity: var("e"),
                attribute: attr("item/name"),
                value: var("name"),
            },
        ],
        predicates: vec![Predicate {
            left: var("n"),
            comparison: Comparison::Ge,
            right: Term::Param(QueryName::new("min").unwrap()),
        }],
        select: vec![
            QueryName::new("name").unwrap(),
            QueryName::new("n").unwrap(),
        ],
        order: vec![Order {
            field: QueryName::new("name").unwrap(),
            direction: Direction::Desc,
        }],
        limit: Some(3),
        ..Query::default()
    };
    let params: Params = [(QueryName::new("min").unwrap(), minimum)].into();

    vec![
        (
            "schema",
            json!({}),
            Box::new(|db| Ok(wire::encode_schema(&db.schema()?))),
        ),
        (
            "facts",
            json!({"filter": parse(&wire::encode_filter(&filter))}),
            {
                let filter = filter.clone();
                Box::new(move |db| Ok(wire::encode_facts(&db.facts(filter.clone())?)))
            },
        ),
        ("facts", json!({}), {
            Box::new(|db| Ok(wire::encode_facts(&db.facts(FactFilter::default())?)))
        }),
        (
            "pull",
            json!({
                "entity": parse(&wire::encode_entity_id(&entity)),
                "attrs": parse(&wire::encode_attrs(&projection)),
            }),
            {
                let entity = entity.clone();
                Box::new(move |db| Ok(wire::encode_entity(db.pull(&entity, &projection)?.as_ref())))
            },
        ),
        (
            "lookup",
            json!({
                "attr": parse(&wire::encode_attr(&attribute)),
                "value": parse(&wire::encode_value(&value)),
            }),
            {
                let (attribute, value) = (attribute.clone(), value.clone());
                Box::new(move |db| Ok(wire::encode_lookup(db.lookup(&attribute, &value)?.as_ref())))
            },
        ),
        (
            "history",
            json!({
                "filter": parse(&wire::encode_filter(&filter)),
                "after": parse(&wire::encode_revision(after)),
            }),
            {
                let filter = filter.clone();
                Box::new(move |db| Ok(wire::encode_history(&db.history(filter.clone(), after)?)))
            },
        ),
        (
            "history",
            json!({}),
            Box::new(|db| {
                Ok(wire::encode_history(
                    &db.history(FactFilter::default(), Revision::ZERO)?,
                ))
            }),
        ),
        (
            "transactions",
            json!({"after": parse(&wire::encode_revision(after))}),
            Box::new(move |db| Ok(wire::encode_transactions(&db.transactions(after)?))),
        ),
        (
            "query",
            json!({"query": parse(&wire::encode_query(&query, &params))}),
            Box::new(move |db| Ok(wire::encode_query_result(&db.query(&query, &params)?))),
        ),
    ]
}

/// Free choices for the reads of one step.
#[derive(Clone, Debug)]
struct Picks {
    entity: usize,
    attribute: usize,
    value: usize,
    filter: u8,
    projection: u8,
    after: u64,
    minimum: i64,
    /// Which earlier revision to read at, if any; and in which spelling.
    at: Option<(u64, bool)>,
}
fn picks() -> impl Strategy<Value = Picks> {
    (
        (0..8usize, 0..5usize, 0..48usize, 0..8u8, 0..32u8),
        (0..12u64, -4..4i64),
        proptest::option::of((0..12u64, any::<bool>())),
    )
        .prop_map(
            |((entity, attribute, value, filter, projection), (after, minimum), at)| Picks {
                entity,
                attribute,
                value,
                filter,
                projection,
                after,
                minimum,
                at,
            },
        )
}

/// The Factlite error object the library's own error encodes to.
fn library_error(error: &factlite::Error) -> Json {
    parse(&wire::encode_error(error))["error"].clone()
}
fn failure_data(failure: &Failure) -> Json {
    assert_eq!(failure.code(), FACTLITE_ERROR, "{failure:?}");
    parse(&failure.data().expect("a Factlite failure has data"))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn every_call_matches_the_library_on_the_same_file(
        steps in proptest::collection::vec((writes(), any::<bool>(), picks()), 1..7),
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.sqlite");
        let mut dispatcher = Dispatcher::new(DispatchOptions::default());
        let opened = parse(
            &dispatcher
                .call("open", Some(&json!({"path": path, "create": true}).to_string()))
                .unwrap(),
        );
        let handle = opened["handle"].clone();
        let library = Store::open(&path).unwrap();
        let transact = |dispatcher: &mut Dispatcher, text: &str| {
            dispatcher.call(
                "transact",
                Some(&format!(r#"{{"handle":{handle},"tx":{text}}}"#)),
            )
        };
        transact(&mut dispatcher, &wire::encode_tx(&schema())).unwrap();

        // Every request sent, with the reply to a committed one.
        let mut sent: Vec<(String, Option<Json>)> = Vec::new();
        let mut entities: Vec<EntityId> = Vec::new();

        for (step, (write, keyed, pick)) in steps.iter().enumerate() {
            let text = match request(write, step, *keyed) {
                Some(tx) => wire::encode_tx(&tx),
                None => {
                    let Write::Repeat { earlier } = write else { unreachable!() };
                    match sent.get(earlier % sent.len().max(1)) {
                        Some((text, _)) => text.clone(),
                        None => wire::encode_tx(&Tx::default()),
                    }
                }
            };
            let before = library.head().unwrap();
            match transact(&mut dispatcher, &text) {
                Ok(report) => {
                    let report = parse(&report);
                    let after: SnapshotId =
                        wire::decode_snapshot(&report["after"].to_string()).unwrap();
                    // The log holds the report the client was given.
                    let record = library
                        .db()
                        .unwrap()
                        .transactions(Revision::new(after.revision().get() - 1))
                        .unwrap()
                        .into_iter()
                        .next()
                        .expect("the revision the report names is in the log");
                    prop_assert_eq!(&report, &parse(&wire::encode_report(&record.report)));
                    for id in report["created_entities"].as_array().unwrap() {
                        let id: EntityId = id.as_str().unwrap().parse().unwrap();
                        if !entities.contains(&id) {
                            entities.push(id);
                        }
                    }
                    // A request that names a key and committed before is
                    // answered again, and commits nothing.
                    let replayed = sent
                        .iter()
                        .find(|(earlier, reply)| *earlier == text && reply.is_some())
                        .filter(|_| text.contains("idempotency_key"));
                    if let Some((_, reply)) = replayed {
                        prop_assert_eq!(Some(&report), reply.as_ref());
                        prop_assert_eq!(&library.head().unwrap(), &before);
                    } else {
                        prop_assert_eq!(&library.head().unwrap(), &after);
                    }
                    sent.push((text, Some(report)));
                }
                Err(failure) => {
                    // Nothing was committed, so the library refuses the same
                    // request for the same reason.
                    prop_assert_eq!(&library.head().unwrap(), &before);
                    let error = library
                        .commit(wire::decode_tx(&text).unwrap())
                        .expect_err("the library must refuse what the dispatcher refused");
                    prop_assert_eq!(failure_data(&failure), library_error(&error));
                    prop_assert_eq!(failure.message(), error.to_string());
                    sent.push((text, None));
                }
            }

            let head = library.head().unwrap();
            let at = pick.at.map(|(revision, as_token)| {
                // A revision past the head is refused by both sides.
                let snapshot = SnapshotId::new(
                    head.store().clone(),
                    head.lineage().clone(),
                    Revision::new(revision),
                );
                let spelling = if as_token {
                    json!(snapshot.to_string())
                } else {
                    parse(&wire::encode_snapshot(&snapshot))
                };
                (snapshot, spelling)
            });
            for (method, mut params, read) in reads(pick, &entities) {
                params["handle"] = handle.clone();
                if let Some((_, spelling)) = &at {
                    params["at"] = spelling.clone();
                }
                let view = match &at {
                    Some((snapshot, _)) => library.db_at(snapshot),
                    None => library.db(),
                };
                let expected = view.and_then(|db| Ok((db.snapshot().clone(), read(&db)?)));
                let actual = dispatcher.call(method, Some(&params.to_string()));
                match (actual, expected) {
                    (Ok(actual), Ok((basis, value))) => prop_assert_eq!(
                        parse(&actual),
                        json!({"basis": parse(&wire::encode_snapshot(&basis)), "value": parse(&value)}),
                        "{} {}", method, params
                    ),
                    (Err(failure), Err(error)) => {
                        prop_assert_eq!(
                            failure_data(&failure), library_error(&error), "{} {}", method, params
                        );
                    }
                    (actual, expected) => prop_assert!(
                        false,
                        "{method} {params}: dispatcher {actual:?}, library {:?}",
                        expected.map(|(_, value)| value).map_err(|error| error.to_string())
                    ),
                }
            }
            prop_assert_eq!(
                parse(&dispatcher.call("head", Some(&json!({"handle": handle}).to_string())).unwrap()),
                parse(&wire::encode_snapshot(&head))
            );
        }
    }
}
