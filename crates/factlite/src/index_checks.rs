//! Evidence that the derived datom index equals authoritative replay.
use crate::{
    AttrName, AttributeDefinition, Cardinality, Db, EntityId, ErrorCode, Revision, SnapshotId,
    Store, TempId, Tx, Uniqueness, Value, ValueType, index, log,
};
use proptest::prelude::*;
use rusqlite::Connection;

const TYPES: [ValueType; 6] = [
    ValueType::Bool,
    ValueType::I64,
    ValueType::F64,
    ValueType::String,
    ValueType::Bytes,
    ValueType::Ref,
];

fn attribute(index: usize, cardinality: Cardinality) -> AttrName {
    let suffix = match cardinality {
        Cardinality::One => "one",
        Cardinality::Many => "many",
    };
    format!("t{index}/{suffix}").parse().unwrap()
}

fn value(index: usize, number: u8, entities: &[EntityId]) -> Value {
    let number = i64::from(number % 4);
    match index {
        0 => Value::Bool(number % 2 == 0),
        1 => Value::I64(number - 2),
        2 => Value::f64(number as f64 / 2.0 - 0.5).unwrap(),
        3 => Value::String(format!("v\0{number}")),
        4 => Value::Bytes(vec![number as u8; number as usize]),
        _ => Value::Ref(entities[number as usize % entities.len()].clone()),
    }
}

/// Creates both cardinalities for all six types, plus four entities.
fn seed(store: &Store) -> Vec<EntityId> {
    let mut tx = Tx::builder();
    for (index, ty) in TYPES.into_iter().enumerate() {
        for cardinality in [Cardinality::One, Cardinality::Many] {
            tx = tx.define_attribute(
                AttributeDefinition::new(
                    attribute(index, cardinality),
                    ty,
                    cardinality,
                    Uniqueness::None,
                )
                .unwrap(),
            );
        }
    }
    for label in ["a", "b", "c", "d"] {
        tx = tx.new_entity(TempId::new(label).unwrap());
    }
    store.commit(tx.build().unwrap()).unwrap().created_entities
}

fn assert_parity(connection: &Connection, head: &SnapshotId) {
    for revision in 0..=head.revision().get() {
        let basis = SnapshotId::new(
            head.store().clone(),
            head.lineage().clone(),
            Revision::new(revision),
        );
        let replayed = log::replay(connection, &basis).unwrap();
        let indexed = index::model_at(connection, &basis).unwrap();
        assert!(
            index::same_state(&indexed, &replayed),
            "revision {revision}"
        );
    }
    index::verify(connection).unwrap();
}

/// Steps: (type, many?, entity, value, assert?). Retract-then-reassert cycles
/// create several intervals for one fact; one-valued asserts close intervals.
type Step = (usize, bool, u8, u8, bool);

fn run(steps: &[Step], batch: usize) -> (tempfile::TempDir, std::path::PathBuf, SnapshotId) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("index.sqlite");
    let store = Store::open(&path).unwrap();
    let entities = seed(&store);
    for chunk in steps.chunks(batch) {
        let mut tx = Tx::builder();
        for &(ty, many, entity, number, assert) in chunk {
            let cardinality = if many {
                Cardinality::Many
            } else {
                Cardinality::One
            };
            let entity = entities[entity as usize % entities.len()].clone();
            let value = value(ty, number, &entities);
            tx = if assert {
                tx.assert(entity, attribute(ty, cardinality), value)
            } else {
                tx.retract(entity, attribute(ty, cardinality), value)
            };
        }
        // Conflicting batches abort atomically; parity must hold either way.
        let _ = store.commit(tx.build().unwrap());
    }
    let head = store.head().unwrap();
    (directory, path, head)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn indexed_state_equals_replay_at_every_revision_and_after_reopen(
        steps in proptest::collection::vec((0_usize..6, any::<bool>(), any::<u8>(), any::<u8>(), any::<bool>()), 0..24),
    ) {
        let (_directory, path, head) = run(&steps, 3);
        let store = Store::open(&path).unwrap();
        assert_parity(&store.connection().unwrap(), &head);
    }
}

#[test]
fn retract_and_reassert_keeps_separate_intervals_and_historical_views() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("cycle.sqlite")).unwrap();
    let entities = seed(&store);
    let name = attribute(1, Cardinality::Many);
    let fact = |assert: bool| {
        let builder = Tx::builder();
        let builder = if assert {
            builder.assert(entities[0].clone(), name.clone(), Value::I64(7))
        } else {
            builder.retract(entities[0].clone(), name.clone(), Value::I64(7))
        };
        store.commit(builder.build().unwrap()).unwrap().after
    };
    let first = fact(true);
    let gone = fact(false);
    let again = fact(true);
    let intervals: Vec<(Vec<u8>, Option<Vec<u8>>)> = store
        .connection()
        .unwrap()
        .prepare("SELECT asserted,retracted FROM datoms ORDER BY asserted")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let bytes = |snapshot: &SnapshotId| snapshot.revision().get().to_be_bytes().to_vec();
    assert_eq!(
        intervals,
        vec![(bytes(&first), Some(bytes(&gone))), (bytes(&again), None)]
    );
    let values = |snapshot: &SnapshotId| {
        store
            .db_at(snapshot)
            .unwrap()
            .facts(crate::FactFilter {
                attribute: Some(name.clone()),
                ..Default::default()
            })
            .unwrap()
            .len()
    };
    assert_eq!((values(&first), values(&gone), values(&again)), (1, 0, 1));
    assert_parity(&store.connection().unwrap(), &again);
}

fn downgrade_to_v1(path: &std::path::Path) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch("DROP TABLE datoms; UPDATE store_info SET format=1; PRAGMA user_version=1;")
        .unwrap();
}

#[test]
fn format_one_upgrades_on_writable_open_and_preserves_every_view() {
    let steps: Vec<Step> = (0..6)
        .flat_map(|ty| {
            [
                (ty, false, 0, 1, true),
                (ty, true, 1, 2, true),
                (ty, true, 1, 2, false),
                (ty, false, 0, 3, true),
            ]
        })
        .collect();
    let (_directory, path, head) = run(&steps, 1);
    let before = Db::open_read_only_at(&path, &head)
        .unwrap()
        .facts(Default::default())
        .unwrap();
    downgrade_to_v1(&path);
    assert_eq!(
        Db::open_read_only(&path).err().unwrap().code(),
        ErrorCode::UnsupportedFormat
    );
    let store = Store::open(&path).unwrap();
    assert_eq!(store.head().unwrap(), head);
    {
        let connection = store.connection().unwrap();
        let version: i32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 3);
        assert_parity(&connection, &head);
    }
    assert_eq!(
        store.db().unwrap().facts(Default::default()).unwrap(),
        before
    );
    let next = store.commit(Tx::default()).unwrap();
    drop(store);
    assert_eq!(Db::open_read_only(&path).unwrap().snapshot(), &next.after);
}

#[test]
fn failed_upgrade_leaves_format_one_file_unchanged() {
    let (_directory, path, _) = run(&[(1, false, 0, 1, true)], 1);
    downgrade_to_v1(&path);
    let connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", false)
        .unwrap();
    connection.execute("DELETE FROM fact_events", []).unwrap();
    drop(connection);
    assert_eq!(Store::open(&path).err().unwrap().code(), ErrorCode::Storage);
    let connection = Connection::open(&path).unwrap();
    let version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name='datoms'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!((version, tables), (1, 0));
}

#[test]
fn tampered_missing_or_extra_intervals_fail_every_open() {
    for mutation in [
        "DELETE FROM datoms WHERE (entity,attribute,type_tag,value,asserted) IN (SELECT entity,attribute,type_tag,value,asserted FROM datoms LIMIT 1)",
        "UPDATE datoms SET retracted=NULL WHERE retracted IS NOT NULL",
        "UPDATE datoms SET retracted=X'0000000000000004' WHERE retracted IS NULL",
        "UPDATE datoms SET value=X'7FFFFFFFFFFFFFFF' WHERE type_tag=1",
        "UPDATE datoms SET asserted=X'0000000000000001'",
        "INSERT INTO datoms SELECT entity,attribute,type_tag,value,X'0000000000000001',X'0000000000000002' FROM datoms LIMIT 1",
    ] {
        let steps: Vec<Step> = vec![
            (1, false, 0, 1, true),
            (1, false, 0, 2, true),
            (1, true, 1, 3, true),
            (1, true, 1, 3, false),
        ];
        let (_directory, path, _) = run(&steps, 1);
        let connection = Connection::open(&path).unwrap();
        // Corruption fixture only: bypass FK protection to inject broken rows.
        connection
            .pragma_update(None, "foreign_keys", false)
            .unwrap();
        assert!(connection.execute(mutation, []).unwrap() > 0, "{mutation}");
        drop(connection);
        assert_eq!(
            Store::open(&path).err().unwrap().code(),
            ErrorCode::Storage,
            "{mutation}"
        );
        assert_eq!(
            Db::open_read_only(&path).err().unwrap().code(),
            ErrorCode::Storage,
            "{mutation}"
        );
    }
}

fn downgrade_to_v2(path: &std::path::Path) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "DROP INDEX datoms_avet; UPDATE store_info SET format=2; PRAGMA user_version=2;",
        )
        .unwrap();
}

#[test]
fn format_two_gains_the_attribute_value_index_on_writable_open() {
    let steps: Vec<Step> = (0..6).map(|ty| (ty, true, 1, 2, true)).collect();
    let (_directory, path, head) = run(&steps, 1);
    let before = Db::open_read_only_at(&path, &head)
        .unwrap()
        .facts(Default::default())
        .unwrap();
    downgrade_to_v2(&path);
    assert_eq!(
        Db::open_read_only(&path).err().unwrap().code(),
        ErrorCode::UnsupportedFormat
    );
    let store = Store::open(&path).unwrap();
    {
        let connection = store.connection().unwrap();
        let version: i32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let indexes: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='datoms_avet'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((version, indexes), (3, 1));
        assert_parity(&connection, &head);
    }
    assert_eq!(
        store.db().unwrap().facts(Default::default()).unwrap(),
        before
    );
}

#[test]
fn a_missing_datom_index_fails_every_open() {
    let (_directory, path, _) = run(&[(1, false, 0, 1, true)], 1);
    Connection::open(&path)
        .unwrap()
        .execute_batch("DROP INDEX datoms_avet")
        .unwrap();
    assert_eq!(Store::open(&path).err().unwrap().code(), ErrorCode::Storage);
    assert_eq!(
        Db::open_read_only(&path).err().unwrap().code(),
        ErrorCode::Storage
    );
}

/// SQL reads at `basis` against the authoritative replayed state.
fn assert_reads_match_replay(store: &Store, basis: &SnapshotId) {
    use crate::{FactFilter, PulledValue};
    let replayed = {
        let connection = store.connection().unwrap();
        log::replay(&connection, basis).unwrap()
    };
    let db = store.db_at(basis).unwrap();
    let schema = replayed.catalog.schema_at(basis.revision()).unwrap();
    assert_eq!(db.schema().unwrap(), schema);
    let mut entities: Vec<EntityId> = replayed.catalog.entities.keys().cloned().collect();
    // Never allocated, and allocated only in another store.
    entities.push(EntityId::allocated(basis.store().clone(), 0xfffff).unwrap());
    entities.push(
        EntityId::allocated("s1.00000000000000000000000000000009".parse().unwrap(), 1).unwrap(),
    );
    let attributes: Vec<AttrName> = schema.attributes().map(|d| d.name().clone()).collect();
    let mut values: Vec<Value> = replayed.facts().iter().map(|f| f.value.clone()).collect();
    values.push(Value::I64(12345));
    let reference = |filter: &FactFilter| {
        let mut facts: Vec<_> = replayed
            .facts()
            .iter()
            .filter(|fact| {
                filter.entity.as_ref().is_none_or(|id| id == &fact.entity)
                    && filter
                        .attribute
                        .as_ref()
                        .is_none_or(|a| a == &fact.attribute)
                    && filter.value.as_ref().is_none_or(|v| v == &fact.value)
            })
            .cloned()
            .collect();
        facts.sort_by(crate::Fact::canonical_cmp);
        facts
    };
    let mut filters = vec![FactFilter::default()];
    for entity in &entities {
        filters.push(FactFilter {
            entity: Some(entity.clone()),
            ..Default::default()
        });
        for attribute in &attributes {
            filters.push(FactFilter {
                entity: Some(entity.clone()),
                attribute: Some(attribute.clone()),
                ..Default::default()
            });
        }
    }
    for attribute in &attributes {
        filters.push(FactFilter {
            attribute: Some(attribute.clone()),
            ..Default::default()
        });
        for value in &values {
            if schema.get(attribute).unwrap().value_type() == value.value_type() {
                filters.push(FactFilter {
                    attribute: Some(attribute.clone()),
                    value: Some(value.clone()),
                    ..Default::default()
                });
            }
        }
    }
    for value in &values {
        filters.push(FactFilter {
            value: Some(value.clone()),
            ..Default::default()
        });
    }
    for filter in &filters {
        assert_eq!(
            db.facts(filter.clone()).unwrap(),
            reference(filter),
            "{filter:?} at {basis}"
        );
    }
    for entity in &entities {
        let pulled = db.pull(entity, &attributes).unwrap();
        let known = replayed.catalog.known_at(entity, basis.revision()).unwrap();
        assert_eq!(pulled.is_some(), known, "{entity} at {basis}");
        if let Some(pulled) = pulled {
            for attribute in &attributes {
                let mut expected: Vec<_> = replayed
                    .facts()
                    .iter()
                    .filter(|f| &f.entity == entity && &f.attribute == attribute)
                    .map(|f| f.value.clone())
                    .collect();
                expected.sort_by(Value::canonical_cmp);
                let actual = match pulled.attributes.get(attribute) {
                    None => Vec::new(),
                    Some(PulledValue::One(value)) => vec![value.clone()],
                    Some(PulledValue::Many(values)) => values.clone(),
                };
                assert_eq!(actual, expected, "{entity} {attribute} at {basis}");
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn sql_reads_equal_replayed_state_at_every_revision(
        steps in proptest::collection::vec((0_usize..6, any::<bool>(), any::<u8>(), any::<u8>(), any::<bool>()), 0..18),
    ) {
        let (_directory, path, head) = run(&steps, 2);
        let store = Store::open(&path).unwrap();
        for revision in 0..=head.revision().get() {
            let basis = SnapshotId::new(head.store().clone(), head.lineage().clone(), Revision::new(revision));
            assert_reads_match_replay(&store, &basis);
        }
    }
}

/// Commit evaluation on the request-touched state equals evaluation on the
/// full state at head, for successes and every error, and the store commits
/// exactly what the full evaluation predicts.
fn touched_trace(steps: &[crate::model::reference::tests::Step]) {
    use crate::model::reference::tests::{ATTRS, build, definition, same_error};
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("touched.sqlite")).unwrap();
    store
        .commit(Tx {
            schema: (0..ATTRS.len()).map(definition).collect(),
            new_entities: (0..3)
                .map(|n| TempId::new(format!("k{n}")).unwrap())
                .collect(),
            ..Tx::default()
        })
        .unwrap();
    for (index, step) in steps.iter().enumerate() {
        let (full, partial, tx) = {
            let connection = store.connection().unwrap();
            let head = crate::store::read_head(&connection).unwrap();
            let full = index::model_at(&connection, &head).unwrap();
            let known: Vec<_> = full.catalog.entities.keys().cloned().collect();
            let tx = build(&full, &known, step);
            let partial = index::model_for_request(&connection, &head, &tx).unwrap();
            (full, partial, tx)
        };
        let expected = full.evaluate_owned(&tx);
        let actual = partial.evaluate_owned(&tx);
        let committed = store.commit(tx.clone());
        match (expected, actual, committed) {
            (Ok((_, expected)), Ok((_, actual)), Ok(committed)) => {
                assert_eq!(actual, expected, "step {index}: {tx:?}");
                assert_eq!(committed, expected, "step {index}: {tx:?}");
            }
            (Err(expected), Err(actual), Err(committed)) => {
                assert!(
                    same_error(&actual, &expected),
                    "step {index}: {actual:?} vs {expected:?}"
                );
                assert!(
                    same_error(&committed, &expected),
                    "step {index}: {committed:?} vs {expected:?}"
                );
            }
            (expected, actual, committed) => panic!(
                "step {index} diverged: full {:?}, touched {:?}, commit {:?} for {tx:?}",
                expected.map(|(_, report)| report),
                actual.map(|(_, report)| report),
                committed
            ),
        }
    }
    // The committed history still replays and verifies.
    drop(store);
    Store::open(directory.path().join("touched.sqlite")).unwrap();
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn touched_commit_state_evaluates_like_full_state(
        steps in proptest::collection::vec(crate::model::reference::tests::step(), 1..14),
    ) {
        touched_trace(&steps);
    }
}

#[test]
fn next_allocation_follows_the_greatest_identity() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("alloc.sqlite")).unwrap();
    for count in [0, 1, 17, 3] {
        let tx = Tx {
            new_entities: (0..count)
                .map(|n| TempId::new(format!("n{n}")).unwrap())
                .collect(),
            ..Tx::default()
        };
        let connection = store.connection().unwrap();
        let head = crate::store::read_head(&connection).unwrap();
        let full = index::model_at(&connection, &head).unwrap();
        let partial = index::model_for_request(&connection, &head, &tx).unwrap();
        assert_eq!(
            partial.catalog.next_allocation(),
            full.catalog.next_allocation()
        );
        drop(connection);
        store.commit(tx).unwrap();
    }
}
