//! Standalone safe copies, read-only history and faithful writer retirement.
use factlite::{Db, ErrorCode, FactFilter, Revision, Store, TempId, Tx, Value, wire};
use std::error::Error as _;

#[test]
fn online_backup_preserves_all_tokens_reports_values_and_read_only_queries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.sqlite");
    let backup = dir.path().join("backup file.sqlite");
    let store = Store::open(&path).unwrap();
    let create = wire::decode_tx(include_str!("../../../examples/create.json")).unwrap();
    let a = store.commit(create.clone()).unwrap();
    let db_a = store.db().unwrap();
    let task = a.tempids[&TempId::new("task").unwrap()].clone();
    let start = Tx::builder()
        .expect(
            task.clone(),
            "task/status".parse().unwrap(),
            vec![Value::String("todo".into())],
        )
        .assert(
            task,
            "task/status".parse().unwrap(),
            Value::String("doing".into()),
        )
        .idempotency_key("start")
        .build()
        .unwrap();
    let b = store.commit(start.clone()).unwrap();
    let c = store
        .commit(
            Tx::builder()
                .metadata("no-change", Value::Bytes(vec![0, 255]))
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(store.backup(&backup).unwrap(), c.after);
    assert_eq!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e
                .file_name()
                .to_string_lossy()
                .starts_with(".factlite-backup-"))
            .count(),
        0
    );
    let reader = Db::open_read_only(&backup).unwrap();
    assert_eq!(*reader.snapshot(), c.after);
    assert_eq!(
        reader
            .transactions(Revision::ZERO)
            .unwrap()
            .iter()
            .map(|r| r.report.clone())
            .collect::<Vec<_>>(),
        vec![a.clone(), b.clone(), c.clone()]
    );
    let (query, params) = wire::decode_query(include_str!("../../../examples/query.json")).unwrap();
    let old = Db::open_read_only_at(&backup, &a.after).unwrap();
    assert_eq!(old.schema().unwrap(), db_a.schema().unwrap());
    assert_eq!(
        old.facts(FactFilter::default()).unwrap(),
        db_a.facts(FactFilter::default()).unwrap()
    );
    assert_eq!(
        old.history(FactFilter::default(), Revision::ZERO).unwrap(),
        db_a.history(FactFilter::default(), Revision::ZERO).unwrap()
    );
    assert!(old.query(&query, &params).unwrap().rows.is_empty());
    assert_eq!(
        reader.query(&query, &params).unwrap(),
        store.db().unwrap().query(&query, &params).unwrap()
    );
    assert_eq!(
        Db::open_read_only_at(&backup, &b.after)
            .unwrap()
            .query(&query, &params)
            .unwrap()
            .rows
            .len(),
        1
    );
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(
            !dir.path()
                .join(format!("backup file.sqlite{suffix}"))
                .exists()
        );
    }
    // Retire every original handle without making a later commit. Only this
    // identical retained history can resume its lineage under the v1 rule.
    drop((old, reader, db_a, store));
    std::fs::remove_file(&path).unwrap();
    let restored = Store::open(&backup).unwrap();
    assert_eq!(restored.commit(create).unwrap(), a);
    assert_eq!(restored.commit(start).unwrap(), b);
    assert_eq!(restored.head().unwrap(), c.after);
    let d = restored.commit(Tx::default()).unwrap();
    assert_eq!(d.before, c.after);
    assert_eq!(d.after.revision(), Revision::new(4));
    assert!(
        restored
            .db_at(&a.after)
            .unwrap()
            .query(&query, &params)
            .unwrap()
            .rows
            .is_empty()
    );
}

#[test]
fn backup_never_overwrites_paths_sidecars_or_source_and_read_only_open_never_initializes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.sqlite");
    let store = Store::open(&path).unwrap();
    let head = store.head().unwrap();
    let existing = dir.path().join("existing");
    std::fs::write(&existing, b"keep").unwrap();
    let error = store.backup(&existing).unwrap_err();
    assert_eq!(error.code(), ErrorCode::Storage);
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .is_some()
    );
    assert_eq!(std::fs::read(&existing).unwrap(), b"keep");
    assert_eq!(store.backup(&path).unwrap_err().code(), ErrorCode::Storage);
    let destination = dir.path().join("destination.sqlite");
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = dir.path().join(format!("destination.sqlite{suffix}"));
        std::fs::write(&sidecar, b"keep").unwrap();
        assert_eq!(
            store.backup(&destination).unwrap_err().code(),
            ErrorCode::Storage
        );
        assert!(!destination.exists());
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"keep");
        std::fs::remove_file(sidecar).unwrap();
    }
    assert_eq!(
        store
            .backup(dir.path().join("missing/backup.sqlite"))
            .unwrap_err()
            .code(),
        ErrorCode::Storage
    );
    for invalid in ["", ":memory:"] {
        assert_eq!(
            store.backup(invalid).unwrap_err().code(),
            ErrorCode::InvalidRequest
        );
    }
    let missing = dir.path().join("missing.sqlite");
    assert_eq!(
        Db::open_read_only(&missing).err().unwrap().code(),
        ErrorCode::Storage
    );
    assert!(!missing.exists());
    let empty = dir.path().join("empty.sqlite");
    std::fs::write(&empty, []).unwrap();
    assert_eq!(
        Db::open_read_only(&empty).err().unwrap().code(),
        ErrorCode::UnsupportedFormat
    );
    assert_eq!(std::fs::metadata(&empty).unwrap().len(), 0);
    let zero_backup = dir.path().join("zero.sqlite");
    assert_eq!(store.backup(&zero_backup).unwrap(), head);
    assert_eq!(*Db::open_read_only(&zero_backup).unwrap().snapshot(), head);
    let foreign = Store::open(dir.path().join("foreign.sqlite"))
        .unwrap()
        .head()
        .unwrap();
    assert_eq!(
        Db::open_read_only_at(&zero_backup, &foreign)
            .err()
            .unwrap()
            .code(),
        ErrorCode::InvalidSnapshot
    );
    assert_eq!(store.head().unwrap(), head);
}

use proptest::prelude::*;
fn values() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::I64),
        any::<u64>().prop_filter_map("finite", |bits| Value::f64(f64::from_bits(bits)).ok()),
        any::<String>().prop_map(Value::String),
        proptest::collection::vec(any::<u8>(), 0..64).prop_map(Value::Bytes)
    ]
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn all_typed_history_and_old_tokens_survive_standalone_copy(values in proptest::collection::vec(values(),0..12),noops in 0_usize..4) {
        use factlite::{AttributeDefinition,Cardinality,Uniqueness,ValueType,Pattern,Term,Query,QueryName,Direction,Order};
        let dir = tempfile::tempdir().unwrap();let path = dir.path().join("source.sqlite");let copy = dir.path().join("copy.sqlite");let store = Store::open(&path).unwrap();
        let allocated = store.commit(Tx::builder().new_entity(TempId::new("data").unwrap()).new_entity(TempId::new("target").unwrap()).build().unwrap()).unwrap();
        let data = allocated.tempids[&TempId::new("data").unwrap()].clone();let target = allocated.tempids[&TempId::new("target").unwrap()].clone();
        let attr = |ty| match ty {ValueType::Bool => "x/bool",ValueType::I64 => "x/i64",ValueType::F64 => "x/f64",ValueType::String => "x/string",ValueType::Bytes => "x/bytes",ValueType::Ref => "x/ref"};
        let mut request = Tx::builder().idempotency_key("all-types");
        for ty in [ValueType::Bool,ValueType::I64,ValueType::F64,ValueType::String,ValueType::Bytes,ValueType::Ref] {request = request.define_attribute(AttributeDefinition::new(attr(ty).parse().unwrap(),ty,Cardinality::Many,Uniqueness::None).unwrap());}
        for value in values.into_iter().chain([Value::Bool(false),Value::I64(i64::MIN),Value::f64(f64::MAX).unwrap(),Value::String("\0e\u{301}雪".into()),Value::Bytes(vec![0,255]),Value::Ref(target)]) {let name=attr(value.value_type()).parse().unwrap();request=request.assert(data.clone(),name,value);}
        let request = request.build().unwrap();let a = store.commit(request).unwrap();let before = store.db().unwrap();
        for _ in 0..noops {store.commit(Tx::default()).unwrap();}
        let head = store.head().unwrap();prop_assert_eq!(store.backup(&copy).unwrap(),head.clone());
        let readonly = Db::open_read_only_at(&copy,&a.after).unwrap();
        prop_assert_eq!(readonly.facts(FactFilter::default()).unwrap(),before.facts(FactFilter::default()).unwrap());
        prop_assert_eq!(readonly.schema().unwrap(),before.schema().unwrap());
        prop_assert_eq!(readonly.history(FactFilter::default(),Revision::ZERO).unwrap(),before.history(FactFilter::default(),Revision::ZERO).unwrap());
        for ty in [ValueType::Bool,ValueType::I64,ValueType::F64,ValueType::String,ValueType::Bytes,ValueType::Ref] {
            let name=QueryName::new("v").unwrap();let q=Query {patterns:vec![Pattern {entity:Term::Const(Value::Ref(data.clone())),attribute:attr(ty).parse().unwrap(),value:Term::Var(name.clone())}],select:vec![name.clone()],order:vec![Order {field:name,direction:Direction::Asc}],..Query::default()};
            prop_assert_eq!(readonly.query(&q,&Default::default()).unwrap(),before.query(&q,&Default::default()).unwrap());
        }
        prop_assert_eq!(Db::open_read_only(&copy).unwrap().transactions(Revision::ZERO).unwrap(),store.db().unwrap().transactions(Revision::ZERO).unwrap());
        store.commit(Tx::default()).unwrap();prop_assert_eq!(Db::open_read_only(&copy).unwrap().snapshot().clone(),head);
    }
}
