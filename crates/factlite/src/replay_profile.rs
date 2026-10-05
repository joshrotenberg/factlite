//! Manual, release-only stage measurements; never compiled into the library.
use std::time::Instant;

use super::*;
use crate::{
    AttributeDefinition, Cardinality, Store, TempId, Tx, Uniqueness, Value, ValueTerm, ValueType,
};

fn fixture(store: &Store, nodes: usize, revisions: usize) -> SnapshotId {
    let mut tx = Tx::builder().idempotency_key("profile-create");
    for (name, ty) in [
        ("profile/number", ValueType::I64),
        ("profile/title", ValueType::String),
        ("profile/link", ValueType::Ref),
    ] {
        tx = tx.define_attribute(
            AttributeDefinition::new(
                name.parse().unwrap(),
                ty,
                Cardinality::One,
                Uniqueness::None,
            )
            .unwrap(),
        );
    }
    let labels: Vec<_> = (0..nodes)
        .map(|n| TempId::new(format!("node-{n}")).unwrap())
        .collect();
    for (index, label) in labels.iter().enumerate() {
        tx = tx
            .new_entity(label.clone())
            .assert(
                label.clone(),
                "profile/number".parse().unwrap(),
                Value::I64(index as i64),
            )
            .assert(
                label.clone(),
                "profile/title".parse().unwrap(),
                Value::String(format!("Node {index}")),
            )
            .assert(
                label.clone(),
                "profile/link".parse().unwrap(),
                ValueTerm::TempRef(labels[(index + 1) % nodes].clone()),
            );
    }
    store.commit(tx.build().unwrap()).unwrap();
    for index in 1..revisions {
        store
            .commit(
                Tx::builder()
                    .idempotency_key(format!("profile-{index}"))
                    .metadata("step", Value::I64(index as i64))
                    .build()
                    .unwrap(),
            )
            .unwrap();
    }
    store.head().unwrap()
}

fn milliseconds(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

fn stages(connection: &Connection, basis: &SnapshotId, nodes: usize) -> [f64; 6] {
    let wall = Instant::now();
    let read = Instant::now();
    let mut statement=connection.prepare("SELECT revision,report,canonical_version,canonical_request FROM transactions WHERE revision<=?1 ORDER BY revision").unwrap();
    let rows = statement
        .query_map([revision_bytes(basis.revision()).as_slice()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let mut result = [0.0; 6];
    result[0] = milliseconds(read);
    let mut model = Model::empty(SnapshotId::new(
        basis.store().clone(),
        basis.lineage().clone(),
        Revision::ZERO,
    ))
    .unwrap();
    for (revision, bytes, version, canonical) in rows {
        let started = Instant::now();
        let report = codec::decode_report(&bytes).unwrap();
        let request = crate::canonical_decode::decode_request(version, &canonical).unwrap();
        result[1] += milliseconds(started);
        assert_eq!(decode_revision(&revision).unwrap(), report.after.revision());
        let started = Instant::now();
        check_events(connection, &revision, &report).unwrap();
        result[2] += milliseconds(started);
        let started = Instant::now();
        let (evaluated, expected) = model.evaluate(&request).unwrap();
        drop(evaluated); // Match the original replay's discarded next state.
        result[3] += milliseconds(started);
        assert_eq!(expected, report);
        let started = Instant::now();
        let applied = model.apply_report(&report).unwrap();
        result[4] += milliseconds(started);
        model = applied;
    }
    assert_eq!(model.catalog.snapshot, *basis);
    assert_eq!(model.facts.len(), 3 * nodes);
    assert_eq!(model.catalog.entities.len(), nodes);
    assert_eq!(model.catalog.schema.len(), 3);
    result[5] = milliseconds(wall);
    result
}

#[test]
#[ignore = "manual release profiling; run with --ignored --exact --nocapture"]
fn retained_replay_stage_profile() {
    let root = std::env::var_os("FACTLITE_PROFILE_DIRECTORY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    assert!(
        root.is_absolute(),
        "FACTLITE_PROFILE_DIRECTORY must be absolute"
    );
    for (nodes, revisions) in [(1000, 50), (100, 250)] {
        let directory = tempfile::tempdir_in(&root).unwrap();
        let store = Store::open(directory.path().join("profile.sqlite")).unwrap();
        let basis = fixture(&store, nodes, revisions);
        let mut connection = store.connection().unwrap();
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)
            .unwrap();
        stages(&transaction, &basis, nodes); // Warm up; setup excluded.
        let mut samples: Vec<_> = (0..15)
            .map(|_| stages(&transaction, &basis, nodes))
            .collect();
        let mut median = [0.0; 6];
        for index in 0..6 {
            samples.sort_by(|a, b| a[index].total_cmp(&b[index]));
            median[index] = samples[7][index];
        }
        println!(
            "{}",
            serde_json::json!({"nodes":nodes,"revisions":revisions,"samples":15,"unit":"ms","debug_assertions":cfg!(debug_assertions),
            "median":{"row_read":median[0],"decode":median[1],"events":median[2],"evaluate":median[3],"apply_report":median[4],"wall":median[5]},
            "method":"original double reconstruction; evaluated next-state cleanup included"})
        );
        transaction.commit().unwrap();
    }
}
