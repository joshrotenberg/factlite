//! Abrupt subprocess termination at deterministic COMMIT boundaries (test only).
use crate::{
    AttributeDefinition, Cardinality, FactFilter, Revision, Store, TempId, Tx, Uniqueness, Value,
    ValueTerm,
};
use std::{
    io::{BufRead, Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

fn request(store: &Store) -> Tx {
    let a = TempId::new("a").unwrap();
    let b = TempId::new("b").unwrap();
    Tx::builder()
        .define_attribute(
            AttributeDefinition::new(
                "x/link".parse().unwrap(),
                crate::ValueType::Ref,
                Cardinality::One,
                Uniqueness::None,
            )
            .unwrap(),
        )
        .define_attribute(
            AttributeDefinition::new(
                "x/key".parse().unwrap(),
                crate::ValueType::I64,
                Cardinality::One,
                Uniqueness::UniqueOne,
            )
            .unwrap(),
        )
        .new_entity(a.clone())
        .new_entity(b.clone())
        .assert(
            a.clone(),
            "x/link".parse().unwrap(),
            ValueTerm::TempRef(b.clone()),
        )
        .assert(b, "x/link".parse().unwrap(), ValueTerm::TempRef(a.clone()))
        .assert(a, "x/key".parse().unwrap(), Value::I64(i64::MAX))
        .expect_head(store.head().unwrap())
        .metadata("durable", Value::Bytes(vec![0, 255]))
        .metadata("spill", Value::Bytes(vec![42; 64 * 1024]))
        .idempotency_key("crash-response")
        .build()
        .unwrap()
}
fn checkpoint() {
    writeln!(std::io::stdout(), "FACTLITE_CHECKPOINT").unwrap();
    std::io::stdout().flush().unwrap();
    let mut byte = [0];
    std::io::stdin().read_exact(&mut byte).unwrap();
    panic!("parent must terminate the checkpointed child");
}
struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn killed_writer_before_commit_rolls_back_and_after_commit_recovers_exact_lost_report() {
    // Re-enter this exact test in its own process. No helper is exposed in the
    // production binary and no extra no-op test is counted as recovery evidence.
    if let Some(mode) = std::env::var_os("FACTLITE_CRASH_MODE") {
        let store = Store::open(std::env::var_os("FACTLITE_CRASH_DB").unwrap()).unwrap();
        store
            .connection()
            .unwrap()
            .pragma_update(None, "cache_size", 2)
            .unwrap();
        let tx = request(&store);
        if mode == "before" {
            store
                .connection()
                .unwrap()
                .commit_hook(Some(|| {
                    checkpoint();
                    false
                }))
                .unwrap();
        }
        store.commit(tx).unwrap();
        checkpoint();
        unreachable!();
    }
    for mode in ["before", "after"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crash.sqlite");
        let store = Store::open(&path).unwrap();
        let old = store
            .commit(
                Tx::builder()
                    .metadata("old", Value::Bool(true))
                    .build()
                    .unwrap(),
            )
            .unwrap();
        let tx = request(&store);
        let expected = {
            let connection = store.connection().unwrap();
            crate::log::replay(&connection, &old.after)
                .unwrap()
                .evaluate(&tx)
                .unwrap()
                .1
        };
        drop(store);
        let mut child = KillOnDrop(Command::new(std::env::current_exe().unwrap())
            .args(["--exact","process_recovery::killed_writer_before_commit_rolls_back_and_after_commit_recovers_exact_lost_report","--nocapture"])
            .env("FACTLITE_CRASH_MODE",mode).env("FACTLITE_CRASH_DB",&path)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap());
        let output = child.0.stdout.take().unwrap();
        let (ready, receive) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(output).lines() {
                if line.unwrap().ends_with("FACTLITE_CHECKPOINT") {
                    ready.send(()).unwrap();
                    return;
                }
            }
        });
        receive
            .recv_timeout(Duration::from_secs(20))
            .expect("child must reach the deterministic commit checkpoint");
        let mut wal_path = path.as_os_str().to_owned();
        wal_path.push("-wal");
        assert!(
            std::fs::metadata(std::path::PathBuf::from(wal_path))
                .unwrap()
                .len()
                > 32,
            "cache pressure must write WAL frames before termination"
        );
        child.0.kill().unwrap();
        assert!(!child.0.wait().unwrap().success());
        reader.join().unwrap();
        let recovered = Store::open(&path).unwrap();
        assert_eq!(
            recovered.head().unwrap(),
            if mode == "before" {
                old.after.clone()
            } else {
                expected.after.clone()
            }
        );
        let before_retry = recovered.db().unwrap();
        assert_eq!(
            before_retry.schema().unwrap().attributes().count(),
            if mode == "before" { 0 } else { 2 }
        );
        assert_eq!(
            before_retry.facts(FactFilter::default()).unwrap().len(),
            if mode == "before" { 0 } else { 3 }
        );
        let keys: i64 = recovered
            .connection()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM transactions WHERE idempotency_key='crash-response'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(keys, if mode == "before" { 0 } else { 1 });
        assert_eq!(recovered.commit(tx.clone()).unwrap(), expected);
        assert_eq!(recovered.commit(tx.clone()).unwrap(), expected);
        assert_eq!(recovered.head().unwrap(), expected.after);
        let records = recovered
            .db()
            .unwrap()
            .transactions(Revision::ZERO)
            .unwrap();
        assert_eq!(
            records.iter().map(|r| r.report.clone()).collect::<Vec<_>>(),
            vec![old, expected.clone()]
        );
        drop((before_retry, recovered));
        assert_eq!(Store::open(&path).unwrap().commit(tx).unwrap(), expected);
    }
}
