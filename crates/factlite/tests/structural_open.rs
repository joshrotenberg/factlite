//! Read-only structural opens and explicit verification (plan item 12.1).

use factlite::{
    Aggregate, AggregateFn, AttributeDefinition, Cardinality, Db, ErrorCode, FactFilter, Params,
    Pattern, Query, QueryName, Revision, SnapshotId, Store, StoreOptions, TempId, Term, Tx,
    Uniqueness, Validation, Value, ValueType,
};
use proptest::prelude::*;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

fn structural() -> StoreOptions {
    StoreOptions::new().with_validation(Validation::Structural)
}

fn definition(name: &str, ty: ValueType, cardinality: Cardinality) -> AttributeDefinition {
    AttributeDefinition::new(name.parse().unwrap(), ty, cardinality, Uniqueness::None).unwrap()
}

/// A store with an i64 and a many-valued string attribute and `steps` later
/// commits that change them.
fn store_with(steps: &[(u8, i64, bool)]) -> (tempfile::TempDir, PathBuf, SnapshotId) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.sqlite");
    let store = Store::open(&path).unwrap();
    let mut tx = Tx::builder()
        .define_attribute(definition("x/n", ValueType::I64, Cardinality::One))
        .define_attribute(definition("x/tag", ValueType::String, Cardinality::Many));
    for n in 0..3 {
        tx = tx.new_entity(TempId::new(format!("e{n}")).unwrap());
    }
    let seed = store.commit(tx.build().unwrap()).unwrap();
    let ids: Vec<_> = seed.created_entities.clone();
    for &(entity, number, retract) in steps {
        let id = ids[usize::from(entity) % ids.len()].clone();
        let tag = Value::String(format!("t{}", number.rem_euclid(3)));
        let tx = if retract {
            Tx::builder().retract(id, "x/tag".parse().unwrap(), tag)
        } else {
            Tx::builder()
                .assert(id.clone(), "x/n".parse().unwrap(), Value::I64(number))
                .assert(id, "x/tag".parse().unwrap(), tag)
        };
        store.commit(tx.build().unwrap()).unwrap();
    }
    let head = store.head().unwrap();
    (dir, path, head)
}

fn at(head: &SnapshotId, revision: u64) -> SnapshotId {
    SnapshotId::new(
        head.store().clone(),
        head.lineage().clone(),
        Revision::new(revision),
    )
}

fn query() -> Query {
    let name = |s: &str| QueryName::new(s).unwrap();
    Query {
        patterns: vec![
            Pattern {
                entity: Term::Var(name("e")),
                attribute: "x/n".parse().unwrap(),
                value: Term::Var(name("n")),
            },
            Pattern {
                entity: Term::Var(name("e")),
                attribute: "x/tag".parse().unwrap(),
                value: Term::Var(name("t")),
            },
        ],
        select: vec![name("t")],
        // A total order, so results compare as sequences.
        order: vec![factlite::Order {
            field: name("t"),
            direction: factlite::Direction::Asc,
        }],
        with: vec![name("e")],
        aggregates: vec![Aggregate {
            function: AggregateFn::Sum,
            var: name("n"),
            name: name("total"),
        }],
        ..Query::default()
    }
}

fn tamper(path: &Path, sql: &str) {
    let connection = Connection::open(path).unwrap();
    // Corruption fixture only: bypass foreign keys to write inconsistent rows.
    connection
        .pragma_update(None, "foreign_keys", false)
        .unwrap();
    assert!(connection.execute(sql, []).unwrap() > 0, "{sql}");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn structural_reads_equal_full_reads_at_every_revision(
        steps in proptest::collection::vec((any::<u8>(), -5_i64..5, any::<bool>()), 0..10),
    ) {
        let (_dir, path, head) = store_with(&steps);
        let full = Db::open_read_only(&path).unwrap();
        let fast = Db::open_read_only_with(&path, &structural()).unwrap();
        prop_assert_eq!(full.snapshot(), fast.snapshot());
        for revision in 0..=head.revision().get() {
            let (a, b) = (full.at(&at(&head, revision)).unwrap(), fast.at(&at(&head, revision)).unwrap());
            prop_assert_eq!(a.schema().unwrap(), b.schema().unwrap());
            prop_assert_eq!(a.facts(FactFilter::default()).unwrap(), b.facts(FactFilter::default()).unwrap());
            prop_assert_eq!(a.history(FactFilter::default(), Revision::ZERO).unwrap(), b.history(FactFilter::default(), Revision::ZERO).unwrap());
            prop_assert_eq!(a.transactions(Revision::ZERO).unwrap(), b.transactions(Revision::ZERO).unwrap());
            if revision > 0 {
                prop_assert_eq!(a.query(&query(), &Params::new()).unwrap(), b.query(&query(), &Params::new()).unwrap());
            }
        }
        prop_assert_eq!(fast.verify().unwrap(), head);
    }
}

#[test]
fn tampering_that_full_validation_rejects_opens_structurally_and_fails_verify() {
    for sql in [
        // A changed interval value, same type.
        "UPDATE datoms SET value=X'8000000000000063' WHERE type_tag=1",
        // A changed stored report.
        "UPDATE transactions SET report=X'00' WHERE revision=X'0000000000000002'",
        // A changed authoritative event.
        "UPDATE fact_events SET value=X'8000000000000063' WHERE type_tag=1",
    ] {
        let (_dir, path, head) = store_with(&[(0, 1, false), (1, 2, false)]);
        tamper(&path, sql);
        assert_eq!(
            Db::open_read_only(&path).err().unwrap().code(),
            ErrorCode::Storage,
            "{sql}"
        );
        let db = Db::open_read_only_with(&path, &structural()).unwrap();
        assert_eq!(db.snapshot(), &head, "{sql}");
        assert_eq!(db.verify().unwrap_err().code(), ErrorCode::Storage, "{sql}");
    }
}

#[test]
fn mistyped_rows_fail_reads_without_panicking() {
    let (_dir, path, _) = store_with(&[(0, 1, false), (1, 2, false)]);
    // An i64 interval rewritten as a valid string value.
    tamper(
        &path,
        "UPDATE datoms SET type_tag=3, value=CAST('oops' AS BLOB) WHERE type_tag=1",
    );
    let db = Db::open_read_only_with(&path, &structural()).unwrap();
    assert_eq!(
        db.facts(FactFilter::default()).unwrap_err().code(),
        ErrorCode::Storage
    );
    // More than 32 patterns takes the reference evaluator, whose aggregation
    // relies on schema types.
    let mut large = query();
    let repeated = large.patterns[0].clone();
    large.patterns.extend(std::iter::repeat_n(repeated, 32));
    assert_eq!(
        db.query(&large, &Params::new()).unwrap_err().code(),
        ErrorCode::Storage
    );
}

#[test]
fn structural_open_still_rejects_wrong_formats_tables_indexes_and_heads() {
    let cases: [(&str, ErrorCode); 4] = [
        ("PRAGMA user_version=99", ErrorCode::UnsupportedFormat),
        ("DROP INDEX datoms_avet", ErrorCode::Storage),
        (
            "UPDATE store_info SET head=X'0000000000000009'",
            ErrorCode::Storage,
        ),
        ("CREATE TABLE extra(x)", ErrorCode::Storage),
    ];
    for (sql, code) in cases {
        let (_dir, path, _) = store_with(&[(0, 1, false)]);
        Connection::open(&path).unwrap().execute_batch(sql).unwrap();
        assert_eq!(
            Db::open_read_only_with(&path, &structural())
                .err()
                .unwrap()
                .code(),
            code,
            "{sql}"
        );
    }
}

#[test]
fn a_missing_interior_revision_is_not_readable() {
    let (_dir, path, head) = store_with(&[(0, 1, false), (1, 2, false)]);
    tamper(
        &path,
        "DELETE FROM fact_events WHERE revision=X'0000000000000002'",
    );
    tamper(
        &path,
        "DELETE FROM transactions WHERE revision=X'0000000000000002'",
    );
    let db = Db::open_read_only_with(&path, &structural()).unwrap();
    assert_eq!(db.snapshot(), &head);
    assert_eq!(
        db.at(&at(&head, 2)).err().unwrap().code(),
        ErrorCode::InvalidSnapshot
    );
    assert!(db.at(&at(&head, 1)).is_ok());
    assert_eq!(db.verify().unwrap_err().code(), ErrorCode::Storage);
}

#[test]
fn writable_structural_opens_are_refused_without_creating_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("never.sqlite");
    let error = Store::open_with(&path, &structural()).err().unwrap();
    assert_eq!(error.code(), ErrorCode::InvalidRequest);
    assert!(!path.exists());
    let (_dir, existing, _) = store_with(&[]);
    assert_eq!(
        Store::open_with(&existing, &structural())
            .err()
            .unwrap()
            .code(),
        ErrorCode::InvalidRequest
    );
}

#[test]
fn verify_reports_the_latest_head_and_catches_tampering_after_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("verify.sqlite");
    let store = Store::open(&path).unwrap();
    assert_eq!(store.verify().unwrap().revision(), Revision::ZERO);
    let pinned = store.db().unwrap();
    let later = store.commit(Tx::default()).unwrap().after;
    // Verification checks the latest head; the old view keeps its pin.
    assert_eq!(pinned.verify().unwrap(), later);
    assert_eq!(pinned.snapshot().revision(), Revision::ZERO);
    tamper(&path, "UPDATE transactions SET report=X'00'");
    assert_eq!(store.verify().unwrap_err().code(), ErrorCode::Storage);
    Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA user_version=99")
        .unwrap();
    assert_eq!(
        store.verify().unwrap_err().code(),
        ErrorCode::UnsupportedFormat
    );
}

#[test]
fn compiled_queries_never_reinterpret_mistyped_rows() {
    let (_dir, path, _) = store_with(&[(0, 1, false), (1, 2, false)]);
    // Same width as an i64, so decoding as the schema type would succeed.
    tamper(&path, "UPDATE datoms SET type_tag=7 WHERE type_tag=1");
    let db = Db::open_read_only_with(&path, &structural()).unwrap();
    // Two patterns take the SQL path: the mistyped rows do not match at all.
    assert!(db.query(&query(), &Params::new()).unwrap().rows.is_empty());
}

#[test]
fn structural_reads_of_reports_and_gaps_fail_instead_of_omitting() {
    let (_dir, path, _) = store_with(&[(0, 1, false), (1, 2, false)]);
    tamper(
        &path,
        "UPDATE transactions SET report=X'00' WHERE revision=X'0000000000000002'",
    );
    let db = Db::open_read_only_with(&path, &structural()).unwrap();
    assert_eq!(
        db.transactions(Revision::ZERO).unwrap_err().code(),
        ErrorCode::Storage
    );
    assert_eq!(
        db.history(FactFilter::default(), Revision::ZERO)
            .unwrap_err()
            .code(),
        ErrorCode::Storage
    );
    let (_dir, path, head) = store_with(&[(0, 1, false), (1, 2, false)]);
    tamper(
        &path,
        "DELETE FROM fact_events WHERE revision=X'0000000000000002'",
    );
    tamper(
        &path,
        "DELETE FROM transactions WHERE revision=X'0000000000000002'",
    );
    let db = Db::open_read_only_with(&path, &structural()).unwrap();
    assert_eq!(db.snapshot(), &head);
    assert_eq!(
        db.transactions(Revision::ZERO).unwrap_err().code(),
        ErrorCode::Storage
    );
    assert_eq!(
        db.history(FactFilter::default(), Revision::ZERO)
            .unwrap_err()
            .code(),
        ErrorCode::Storage
    );
}

#[test]
fn duplicated_one_valued_rows_fail_pull() {
    let (_dir, path, _) = store_with(&[(0, 1, false)]);
    tamper(
        &path,
        "INSERT INTO datoms SELECT entity,attribute,type_tag,X'8000000000000063',asserted,NULL FROM datoms WHERE type_tag=1",
    );
    let db = Db::open_read_only_with(&path, &structural()).unwrap();
    let id = db.facts(FactFilter::default()).unwrap()[0].entity.clone();
    assert_eq!(
        db.pull(&id, &["x/n".parse().unwrap()]).unwrap_err().code(),
        ErrorCode::Storage
    );
}
