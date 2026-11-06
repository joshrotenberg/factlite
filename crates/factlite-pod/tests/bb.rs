//! Runs the pod under a real babashka. Skipped when `bb` is not on PATH,
//! unless `REQUIRE_BB` is set.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use factlite::{
    AttributeDefinition, Cardinality, FactFilter, Store, TempId, Tx, Uniqueness, Value, ValueType,
};

fn have_bb() -> bool {
    if Command::new("bb").arg("--version").output().is_ok() {
        return true;
    }
    assert!(
        std::env::var_os("REQUIRE_BB").is_none(),
        "bb not found on PATH and REQUIRE_BB is set"
    );
    eprintln!("bb not found on PATH, skipping");
    false
}

fn script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
}

fn bb(args: &[&str], envs: &[(&str, &Path)]) -> Command {
    let mut command = Command::new("bb");
    command
        .args(args)
        .env("FACTLITE_POD", env!("CARGO_BIN_EXE_pod-factlite"));
    for (key, value) in envs {
        command.env(key, value);
    }
    command
}

fn stdout(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "bb failed\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

fn end_to_end(transport: &str) {
    if !have_bb() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let test = script("test.bb");
    let output = bb(
        &[test.to_str().unwrap(), transport],
        &[("FACTLITE_DIR", dir.path())],
    )
    .output()
    .unwrap();
    let out = stdout(&output);
    assert!(out.contains(&format!("transport: :{transport}")), "{out}");
    assert!(out.contains("failures: 0") && out.contains("done"), "{out}");
    assert!(!out.contains("FAIL"), "{out}");
    // Guard against a script that silently skips checks; update with test.bb.
    assert_eq!(
        out.lines().filter(|line| line.starts_with("ok ")).count(),
        79,
        "{out}"
    );
}

#[test]
fn stdio_transport() {
    end_to_end("stdio");
}

#[test]
fn socket_transport() {
    end_to_end("socket");
}

fn definition(name: &str, ty: ValueType) -> AttributeDefinition {
    AttributeDefinition::new(
        name.parse().unwrap(),
        ty,
        Cardinality::One,
        Uniqueness::None,
    )
    .unwrap()
}

/// Several pod processes commit to one file at once. Keyed creations must all
/// land exactly once, and conditional increments must serialize: no lost,
/// merged or duplicated writes.
#[test]
fn concurrent_pod_processes_lose_and_merge_nothing() {
    if !have_bb() {
        return;
    }
    // Defaults keep the gate fast; raise them for a heavier manual run.
    let size = |key: &str, default: usize| {
        std::env::var(key).map_or(default, |value| value.parse().unwrap())
    };
    let workers = size("FACTLITE_POD_WORKERS", 4);
    let iterations_per_worker = size("FACTLITE_POD_ITERATIONS", 15);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.sqlite");
    let store = Store::open(&path).unwrap();
    let counter = TempId::new("counter").unwrap();
    let seed = store
        .commit(
            Tx::builder()
                .define_attribute(definition("count/value", ValueType::I64))
                .define_attribute(definition("run/worker", ValueType::I64))
                .define_attribute(definition("run/n", ValueType::I64))
                .new_entity(counter.clone())
                .assert(
                    counter.clone(),
                    "count/value".parse().unwrap(),
                    Value::I64(0),
                )
                .build()
                .unwrap(),
        )
        .unwrap();
    let counter = seed.tempids[&counter].to_string();
    drop(store);
    let contend = script("contend.bb");
    let iterations = iterations_per_worker.to_string();
    let children: Vec<_> = (0..workers)
        .map(|worker| {
            let worker = worker.to_string();
            let transport = if worker == "0" { "socket" } else { "stdio" };
            bb(
                &[
                    contend.to_str().unwrap(),
                    transport,
                    &worker,
                    &iterations,
                    &counter,
                ],
                &[("FACTLITE_DB", &path)],
            )
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
        })
        .collect();
    let mut retries = 0;
    for child in children {
        let out = stdout(&child.wait_with_output().unwrap());
        assert!(out.contains("done"), "{out}");
        retries += out
            .lines()
            .find_map(|line| line.strip_prefix("retries: "))
            .unwrap()
            .parse::<usize>()
            .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let db = store.db().unwrap();
    let total = (workers * iterations_per_worker) as i64;
    let counted = db
        .facts(FactFilter {
            attribute: Some("count/value".parse().unwrap()),
            ..FactFilter::default()
        })
        .unwrap();
    assert_eq!(counted.len(), 1);
    assert_eq!(counted[0].value, Value::I64(total));
    let mut runs: Vec<(i64, i64)> = Vec::new();
    for worker in db
        .facts(FactFilter {
            attribute: Some("run/worker".parse().unwrap()),
            ..FactFilter::default()
        })
        .unwrap()
    {
        let entity = db
            .pull(&worker.entity, &["run/n".parse().unwrap()])
            .unwrap()
            .unwrap();
        let Value::I64(w) = worker.value else {
            panic!()
        };
        let Some(factlite::PulledValue::One(Value::I64(n))) =
            entity.attributes.values().next().cloned()
        else {
            panic!()
        };
        runs.push((w, n));
    }
    runs.sort();
    let expected: Vec<_> = (0..workers as i64)
        .flat_map(|w| (0..iterations_per_worker as i64).map(move |n| (w, n)))
        .collect();
    assert_eq!(runs, expected, "every keyed creation exactly once");
    // One seed commit plus one creation and one increment per iteration.
    // Failed attempts never commit, so the revision count is exact.
    assert_eq!(db.snapshot().revision().get(), 1 + 2 * total as u64);
    eprintln!("conditional retries across workers: {retries}");
}
