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
        assert_eq!(version, 2);
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
