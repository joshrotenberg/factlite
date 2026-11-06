//! Actual subprocess scenarios via assert_cmd; every command reopens the file.

use std::{path::Path, time::Duration};

use assert_cmd::{Command, cargo::cargo_bin_cmd};
use factlite::{Revision, Store, TempId, Tx, Value, wire};
use serde_json::{Value as Json, json};

fn binary() -> Command {
    let mut command = cargo_bin_cmd!("factlite");
    command.timeout(Duration::from_secs(20));
    command
}
fn db(path: &Path) -> Command {
    let mut command = binary();
    command.arg("--db").arg(path);
    command
}
fn success(command: &mut Command) -> Json {
    let assertion = command.assert().success().stderr("");
    let bytes = &assertion.get_output().stdout;
    assert_eq!(
        bytes.iter().filter(|byte| **byte == b'\n').count(),
        1,
        "one result line"
    );
    let result: Json = serde_json::from_slice(bytes).unwrap();
    assert_eq!(result["version"], 1);
    result
}
fn failure(command: &mut Command, exit: i32, code: &str) -> Json {
    let assertion = command.assert().code(exit).stdout("");
    let bytes = &assertion.get_output().stderr;
    assert_eq!(
        bytes.iter().filter(|byte| **byte == b'\n').count(),
        1,
        "one error line"
    );
    let result: Json = serde_json::from_slice(bytes).unwrap();
    assert_eq!(result["version"], 1);
    assert_eq!(result["error"]["code"], code);
    result
}
fn transact(path: &Path, request: &Json) -> Json {
    success(
        db(path)
            .args(["transact", "--file", "-"])
            .write_stdin(request.to_string()),
    )
}
fn projection(path: &Path, snapshot: &Path, task: &str) -> Json {
    success(db(path).arg("pull").arg("--at-file").arg(snapshot).args([
        "--entity",
        task,
        "--attrs",
        "task/status",
    ]))
}

#[test]
fn design_create_start_reopen_retry_fixture_matches_rust_reads_and_full_reports() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("demo file.sqlite");
    let zero = success(binary().arg("init").arg(&path));
    assert_eq!(zero["revision"], "0");
    let create_file = dir.path().join("create file.json");
    std::fs::write(&create_file, include_str!("../../../examples/create.json")).unwrap();
    let a = success(db(&path).arg("transact").arg("--file").arg(&create_file));
    assert_eq!(a["before"], zero);
    assert_eq!(a["after"]["revision"], "1");
    let task = a["tempids"]["task"].as_str().unwrap();
    let person = a["tempids"]["person"].as_str().unwrap();
    let a_file = dir.path().join("A snapshot.json");
    std::fs::write(&a_file, a["after"].to_string()).unwrap();
    let query_text = include_str!("../../../examples/query.json");
    let query_file = dir.path().join("joined query.json");
    std::fs::write(&query_file, query_text).unwrap();
    let query_at = |basis: &Path| {
        success(
            db(&path)
                .arg("query")
                .arg("--file")
                .arg(&query_file)
                .arg("--at-file")
                .arg(basis),
        )
    };
    let empty_join = json!({"version":1,"columns":["task","title","name"],"rows":[]});
    assert_eq!(query_at(&a_file), empty_join);
    let expected_a =
        json!({"version":1,"entity":{"id":task},"attributes":{"task/status":{"string":"todo"}}});
    assert_eq!(projection(&path, &a_file, task), expected_a);
    let request_b = json!({"version":1,"idempotency_key":"demo-start-v1","expect":[{"entity":{"id":task},"attr":"task/status","values":[{"string":"todo"}]}],"changes":[{"op":"assert","entity":{"id":task},"attr":"task/status","value":{"string":"doing"}}],"metadata":{"source":{"string":"design-example"}}});
    let b = transact(&path, &request_b);
    assert_eq!(b["before"], a["after"]);
    assert_eq!(b["after"]["revision"], "2");
    assert_eq!(
        b["effective_delta"],
        json!([
            {"op":"retract","entity":{"id":task},"attr":"task/status","value":{"string":"todo"}},
            {"op":"assert","entity":{"id":task},"attr":"task/status","value":{"string":"doing"}}
        ])
    );
    let b_file = dir.path().join("B.json");
    std::fs::write(&b_file, b["after"].to_string()).unwrap();
    let joined = json!({"version":1,"columns":["task","title","name"],"rows":[[{"ref":{"id":task}},{"string":"Design Factlite"},{"string":"Ada"}]]});
    assert_eq!(query_at(&a_file), empty_join);
    assert_eq!(query_at(&b_file), joined);
    assert_eq!(
        success(
            db(&path)
                .arg("query")
                .args(["--file", "-"])
                .arg("--at-file")
                .arg(&b_file)
                .write_stdin(query_text)
        ),
        joined
    );
    let (query, params) = wire::decode_query(query_text).unwrap();
    let rust_store = Store::open(&path).unwrap();
    let rust_b = rust_store
        .db_at(&wire::decode_snapshot(&b["after"].to_string()).unwrap())
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Json>(&wire::encode_query_result(
            &rust_b.query(&query, &params).unwrap()
        ))
        .unwrap(),
        joined
    );
    assert_eq!(projection(&path, &a_file, task), expected_a);
    assert_eq!(
        projection(&path, &b_file, task),
        json!({"version":1,"entity":{"id":task},"attributes":{"task/status":{"string":"doing"}}})
    );
    let owner = success(db(&path).arg("pull").arg("--at-file").arg(&a_file).args([
        "--entity",
        task,
        "--attrs",
        "task/owner,task/title",
    ]));
    assert_eq!(
        owner["attributes"]["task/owner"],
        json!({"ref":{"id":person}})
    );
    let person_pull = success(db(&path).arg("pull").arg("--at-file").arg(&a_file).args([
        "--entity",
        person,
        "--attrs",
        "person/name",
    ]));
    assert_eq!(
        person_pull["attributes"]["person/name"],
        json!({"string":"Ada"})
    );
    let history = success(
        db(&path)
            .arg("history")
            .arg("--at-file")
            .arg(&b_file)
            .args(["--entity", task, "--attr", "task/status", "--after", "0"]),
    );
    assert_eq!(history["events"].as_array().unwrap().len(), 3);
    assert_eq!(history["events"][1]["op"], "retract");
    assert_eq!(history["events"][1]["revision"], "2");
    assert_eq!(history["events"][2]["ordinal"], "1");
    assert_eq!(
        success(db(&path).arg("history").arg("--at-file").arg(&a_file))["events"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(transact(&path, &request_b), b);
    assert_eq!(
        success(db(&path).arg("transact").arg("--file").arg(&create_file)),
        a
    );
    let mut changed = request_b.clone();
    changed["metadata"]["source"] = json!({"string":"changed"});
    failure(
        db(&path)
            .args(["transact", "--file", "-"])
            .write_stdin(changed.to_string()),
        3,
        "IdempotencyConflict",
    );
    let mut fresh_stale = request_b.clone();
    fresh_stale["idempotency_key"] = json!("new-key");
    let error = failure(
        db(&path)
            .args(["transact", "--file", "-"])
            .write_stdin(fresh_stale.to_string()),
        3,
        "PreconditionFailed",
    );
    assert_eq!(
        error["error"]["context"]["expected"],
        json!([{"string":"todo"}])
    );
    assert_eq!(
        error["error"]["context"]["actual"],
        json!([{"string":"doing"}])
    );
    assert_eq!(error["error"]["commit_outcome"], "RolledBack");
    let noop = json!({"version":1,"idempotency_key":"noop","changes":[{"op":"assert","entity":{"id":task},"attr":"task/status","value":{"string":"doing"}}],"metadata":{"noop":{"bool":true}}});
    let c = transact(&path, &noop);
    assert_eq!(c["after"]["revision"], "3");
    assert_eq!(c["effective_delta"], json!([]));
    assert_eq!(transact(&path, &request_b), b);
    assert_eq!(query_at(&a_file), empty_join);
    assert_eq!(query_at(&b_file), joined);
    assert_eq!(projection(&path, &a_file, task), expected_a);
    let transactions = success(db(&path).arg("transactions"));
    assert_eq!(transactions["transactions"].as_array().unwrap().len(), 3);
    assert_eq!(transactions["transactions"][2]["report"], c);
    assert_eq!(
        success(
            db(&path)
                .arg("transactions")
                .arg("--at-file")
                .arg(&b_file)
                .args(["--after", "1"])
        )["transactions"],
        json!([{"revision":"2","report":b}])
    );
    let rust = Store::open(&path).unwrap();
    let reports = rust.db().unwrap().transactions(Revision::ZERO).unwrap();
    for (record, expected) in reports.iter().zip([&a, &b, &c]) {
        assert_eq!(
            serde_json::from_str::<Json>(&wire::encode_report(&record.report)).unwrap(),
            *expected
        );
    }
    let a_token = wire::decode_snapshot(&a["after"].to_string()).unwrap();
    let a_db = rust.db_at(&a_token).unwrap();
    let schema =
        serde_json::from_str::<Json>(&wire::encode_schema(&a_db.schema().unwrap())).unwrap();
    assert_eq!(
        success(db(&path).args(["schema", "--at", &a_token.to_string()])),
        schema
    );
    assert_eq!(
        success(db(&path).args(["facts", "--at", &a_token.to_string()])),
        serde_json::from_str::<Json>(&wire::encode_facts(
            &a_db.facts(factlite::FactFilter::default()).unwrap()
        ))
        .unwrap()
    );
    assert_eq!(success(binary().arg("init").arg(&path))["revision"], "3");
}

#[test]
fn invalid_arguments_wire_and_bounds_are_structured_and_do_not_create_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.sqlite");
    for args in [
        vec![],
        vec!["schema"],
        vec!["--help", "extra"],
        vec!["init"],
        vec!["init", "a", "b"],
    ] {
        failure(binary().args(args), 2, "InvalidRequest");
    }
    for args in [
        vec!["pull"],
        vec!["pull", "--entity", "bogus"],
        vec!["schema", "--entity", "x"],
        vec!["schema", "--at", "x", "--at-file", "y"],
        vec!["schema", "--at"],
        vec!["facts", "--after", "0"],
        vec!["transactions", "--after", "0", "--after", "1"],
        vec!["transact"],
        vec!["query"],
        vec!["facts", "--entity", "bogus"],
        vec!["transactions", "--after", "-1"],
    ] {
        failure(db(&path).args(args), 2, "InvalidRequest");
    }
    failure(
        db(&path)
            .args(["transact", "--file", "-"])
            .write_stdin(r#"{"version":1,"version":1}"#),
        2,
        "InvalidRequest",
    );
    failure(
        db(&path)
            .args(["transact", "--file", "-"])
            .write_stdin(vec![255]),
        2,
        "InvalidRequest",
    );
    assert!(!path.exists());
    failure(db(&path).arg("schema"), 5, "Storage");
    assert!(!path.exists());
    success(binary().arg("init").arg(&path));
    failure(
        db(&path).args(["history", "--after", "1"]),
        2,
        "InvalidRequest",
    );
    failure(
        db(&path).args(["transactions", "--after", "1"]),
        2,
        "InvalidRequest",
    );
    failure(
        db(&path).args(["facts", "--attr", "later/value"]),
        2,
        "UnknownAttribute",
    );
    failure(
        db(&path).args(["schema", "--at", "bogus"]),
        2,
        "InvalidSnapshot",
    );
    failure(
        db(&path)
            .args(["transact", "--file"])
            .arg(dir.path().join("missing.json")),
        5,
        "Storage",
    );
    let error = failure(db(&path).args(["transact","--file","-"]).write_stdin(r#"{"version":1,"new_entities":["e"],"changes":[{"op":"assert","entity":{"temp":"e"},"attr":"missing/value","value":{"i64":"1"}}]}"#),2,"UnknownAttribute");
    assert_eq!(error["error"]["commit_outcome"], "RolledBack");
    assert_eq!(success(binary().arg("init").arg(&path))["revision"], "0");
}

#[test]
fn busy_and_unsupported_storage_have_distinct_exit_codes_and_empty_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("busy.sqlite");
    success(binary().arg("init").arg(&path));
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    failure(
        db(&path)
            .args(["transact", "--file", "-"])
            .write_stdin(r#"{"version":1,"idempotency_key":"key"}"#),
        4,
        "Busy",
    );
    connection.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        transact(&path, &json!({"version":1,"idempotency_key":"key"}))["after"]["revision"],
        "1"
    );
    drop(connection);
    let future = dir.path().join("future.sqlite");
    success(binary().arg("init").arg(&future));
    let connection = rusqlite::Connection::open(&future).unwrap();
    connection.pragma_update(None, "user_version", 4).unwrap();
    drop(connection);
    failure(db(&future).arg("schema"), 5, "UnsupportedFormat");
    let corrupt = dir.path().join("corrupt.sqlite");
    std::fs::write(&corrupt, b"not SQLite").unwrap();
    failure(db(&corrupt).arg("schema"), 5, "Storage");
}

#[test]
fn projection_many_types_empty_unknown_entities_and_value_filters_are_lossless() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("types.sqlite");
    success(binary().arg("init").arg(&path));
    let store = Store::open(&path).unwrap();
    let label = TempId::new("e").unwrap();
    let empty = TempId::new("empty").unwrap();
    let mut tx = Tx::builder()
        .new_entity(label.clone())
        .new_entity(empty.clone());
    let cases = [
        vec![Value::Bool(false), Value::Bool(true)],
        vec![Value::I64(i64::MIN), Value::I64(i64::MAX)],
        vec![Value::f64(-0.0).unwrap(), Value::f64(f64::MAX).unwrap()],
        vec![Value::String("\0é".into())],
        vec![Value::Bytes(vec![0, 255])],
    ];
    for (index, values) in cases.iter().enumerate() {
        let name = format!("x/v{index}");
        tx = tx.define_attribute(
            factlite::AttributeDefinition::new(
                name.parse().unwrap(),
                values[0].value_type(),
                factlite::Cardinality::Many,
                factlite::Uniqueness::None,
            )
            .unwrap(),
        );
        for value in values.iter().rev() {
            tx = tx.assert(label.clone(), name.parse().unwrap(), value.clone());
        }
    }
    let report = store.commit(tx.build().unwrap()).unwrap();
    let id = report.tempids[&label].to_string();
    let empty = report.tempids[&empty].to_string();
    drop(store);
    for (index, values) in cases.iter().enumerate() {
        let name = format!("x/v{index}");
        let projected = success(db(&path).args(["pull", "--entity", &id, "--attrs", &name]));
        assert_eq!(
            projected["attributes"][&name],
            Json::Array(
                values
                    .iter()
                    .map(|value| serde_json::from_str(&wire::encode_value(value)).unwrap())
                    .collect()
            )
        );
        let selected = success(db(&path).args([
            "facts",
            "--entity",
            &id,
            "--attr",
            &name,
            "--value",
            &wire::encode_value(&values[0]),
        ]));
        assert_eq!(selected["facts"].as_array().unwrap().len(), 1);
    }
    assert_eq!(
        success(db(&path).args(["pull", "--entity", &empty, "--attrs", "x/v0"])),
        json!({"version":1,"entity":{"id":empty},"attributes":{}})
    );
    assert_eq!(
        success(db(&path).args(["pull", "--entity", &id, "--attrs", ""]))["attributes"],
        json!({})
    );
    let unknown = format!(
        "e1.{}.ffffffffffffffff",
        report
            .after
            .store()
            .to_string()
            .strip_prefix("s1.")
            .unwrap()
    );
    assert_eq!(
        success(db(&path).args(["pull", "--entity", &unknown, "--attrs", "x/v0"])),
        json!({"version":1,"entity":null})
    );
    failure(
        db(&path).args(["facts", "--attr", "x/v0", "--value", r#"{"i64":"0"}"#]),
        2,
        "TypeMismatch",
    );
    let encoded = wire::encode_snapshot(&report.after);
    assert_eq!(
        success(
            db(&path)
                .args(["schema", "--at-file", "-"])
                .write_stdin(encoded)
        )["schema"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
}

#[test]
fn help_version_and_non_utf8_paths_preserve_process_contracts() {
    binary().arg("--help").assert().success().stderr("");
    binary()
        .arg("--version")
        .assert()
        .success()
        .stderr("")
        .stdout("factlite 0.1.0\n");
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join(std::ffi::OsString::from_vec(vec![b'd', 255]));
        // Some filesystems (APFS on macOS) reject non-UTF-8 names outright, so
        // the path round trip is only meaningful where the name can exist.
        let probe = dir
            .path()
            .join(std::ffi::OsString::from_vec(vec![b'p', 255]));
        if std::fs::write(&probe, b"").is_ok() {
            success(binary().arg("init").arg(&path));
            success(db(&path).arg("schema"));
        }
        failure(
            binary().arg(std::ffi::OsString::from_vec(vec![255])),
            2,
            "InvalidRequest",
        );
    }
}

#[test]
fn write_constraints_and_head_fences_keep_typed_context_and_conflict_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("errors.sqlite");
    let zero = success(binary().arg("init").arg(&path));
    let created = transact(
        &path,
        &json!({"version":1,"schema":[{"name":"person/name","type":"string","cardinality":"one","unique":"unique_one"}],"new_entities":["a","b"],"changes":[{"op":"assert","entity":{"temp":"a"},"attr":"person/name","value":{"string":"Ada"}}]}),
    );
    let a = &created["tempids"]["a"];
    let b = &created["tempids"]["b"];
    let write = |entity: &Json, value: Json| json!({"op":"assert","entity":{"id":entity},"attr":"person/name","value":value});
    let head = failure(
        db(&path)
            .args(["transact", "--file", "-"])
            .write_stdin(json!({"version":1,"expected_head":zero}).to_string()),
        3,
        "HeadMismatch",
    );
    assert_eq!(head["error"]["context"]["expected"], zero);
    assert_eq!(head["error"]["context"]["actual"], created["after"]);
    let unique = failure(
        db(&path).args(["transact", "--file", "-"]).write_stdin(
            json!({"version":1,"changes":[write(b,json!({"string":"Ada"}))]}).to_string(),
        ),
        3,
        "UniqueViolation",
    );
    assert_eq!(unique["error"]["context"]["value"], json!({"string":"Ada"}));
    assert_eq!(unique["error"]["context"]["owners"], json!([a, b]));
    let mismatch = failure(
        db(&path)
            .args(["transact", "--file", "-"])
            .write_stdin(json!({"version":1,"changes":[write(a,json!({"i64":"1"}))]}).to_string()),
        2,
        "TypeMismatch",
    );
    assert_eq!(
        mismatch["error"]["context"],
        json!({"expected":"string","actual":"i64"})
    );
    let mut retraction = write(a, json!({"string":"Ada"}));
    retraction["op"] = json!("retract");
    let conflict = failure(
        db(&path).args(["transact", "--file", "-"]).write_stdin(
            json!({"version":1,"changes":[write(a,json!({"string":"Ada"})),retraction]})
                .to_string(),
        ),
        3,
        "ConflictingBatch",
    );
    assert_eq!(
        conflict["error"]["context"]["value"],
        json!({"string":"Ada"})
    );
    let conflict = failure(db(&path).args(["transact","--file","-"]).write_stdin(json!({"version":1,"changes":[write(a,json!({"string":"one"})),write(a,json!({"string":"two"}))]}).to_string()),3,"ConflictingBatch");
    assert_eq!(conflict["error"]["context"]["entity"], json!({"id":a}));
    let store = Store::open(&path).unwrap();
    let unknown = json!(format!(
        "e1.{}.ffffffffffffffff",
        store
            .head()
            .unwrap()
            .store()
            .to_string()
            .strip_prefix("s1.")
            .unwrap()
    ));
    drop(store);
    let error = failure(
        db(&path).args(["transact", "--file", "-"]).write_stdin(
            json!({"version":1,"changes":[write(&unknown,json!({"string":"unknown"}))]})
                .to_string(),
        ),
        2,
        "UnknownEntity",
    );
    assert_eq!(error["error"]["context"]["entity"], json!({"id":unknown}));
    assert_eq!(success(binary().arg("init").arg(&path)), created["after"]);
}

#[test]
fn query_validation_errors_empty_rows_and_historical_schema_use_shared_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("queries.sqlite");
    let zero = success(binary().arg("init").arg(&path));
    let a = transact(
        &path,
        &serde_json::from_str(include_str!("../../../examples/create.json")).unwrap(),
    );
    let base: Json = serde_json::from_str(include_str!("../../../examples/query.json")).unwrap();
    let mut cases = Vec::new();
    let mut q = base.clone();
    q["params"] = json!({});
    cases.push((q, "InvalidRequest"));
    let mut q = base.clone();
    q["params"]["extra"] = json!({"bool":true});
    cases.push((q, "InvalidRequest"));
    let mut q = base.clone();
    q["params"]["wanted"] = json!({"i64":"1"});
    cases.push((q, "TypeMismatch"));
    let mut q = base.clone();
    q["query"]["limit"] = json!(-1);
    cases.push((q, "InvalidRequest"));
    let mut q = base.clone();
    q["query"]["select"] = json!(["unbound"]);
    cases.push((q, "InvalidRequest"));
    let mut q = base.clone();
    q["query"]["select"] = json!(["task", "task"]);
    cases.push((q, "InvalidRequest"));
    let mut q = base.clone();
    q["query"]["order"] = json!([{"field":"person","direction":"asc"}]);
    cases.push((q, "InvalidRequest"));
    let mut q = base.clone();
    q["query"]["match"][0][1] = json!("task/missing");
    q["query"]["limit"] = json!(0);
    cases.push((q, "UnknownAttribute"));
    for (q, code) in cases {
        failure(
            db(&path)
                .args(["query", "--file", "-"])
                .write_stdin(q.to_string()),
            2,
            code,
        );
    }
    failure(
        db(&path).args(["query", "--file", "-", "--at", "invalid"]),
        2,
        "InvalidSnapshot",
    );
    failure(
        db(&path)
            .args(["query", "--file", "-", "--at-file", "-"])
            .write_stdin(base.to_string()),
        2,
        "InvalidRequest",
    );
    let at_zero = wire::decode_snapshot(&zero.to_string())
        .unwrap()
        .to_string();
    failure(
        db(&path)
            .args(["query", "--file", "-", "--at", &at_zero])
            .write_stdin(base.to_string()),
        2,
        "UnknownAttribute",
    );
    let unit = json!({"version":1,"query":{"match":[],"select":[]}});
    assert_eq!(
        success(
            db(&path)
                .args(["query", "--file", "-"])
                .write_stdin(unit.to_string())
        ),
        json!({"version":1,"columns":[],"rows":[[]]})
    );
    let mut q = base;
    q["params"]["wanted"] = json!({"string":"todo"});
    q["query"]["limit"] = json!(0);
    assert_eq!(
        success(
            db(&path)
                .args(["query", "--file", "-"])
                .write_stdin(q.to_string())
        )["rows"],
        json!([])
    );
    q["query"]["limit"] = json!(u64::MAX);
    assert_eq!(
        success(
            db(&path)
                .args(["query", "--file", "-"])
                .write_stdin(q.to_string())
        )["rows"][0][0],
        json!({"ref":{"id":a["tempids"]["task"]}})
    );
    assert_eq!(success(binary().arg("init").arg(&path)), a["after"]);
}

#[test]
fn concurrent_cli_processes_share_exact_retry_reports_and_serialize_conditions() {
    use std::sync::{Arc, Barrier};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("contenders.sqlite");
    success(binary().arg("init").arg(&path));
    let request = include_str!("../../../examples/create.json");
    let barrier = Arc::new(Barrier::new(8));
    let reports = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                let path = &path;
                scope.spawn(move || {
                    barrier.wait();
                    success(
                        db(path)
                            .args(["transact", "--file", "-"])
                            .write_stdin(request),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(reports.iter().all(|report| report == &reports[0]));
    let task = reports[0]["tempids"]["task"].as_str().unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let outputs = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8).map(|index| { let barrier = barrier.clone(); let path = &path; scope.spawn(move || {
            let request = json!({"version":1,"idempotency_key":format!("condition-{index}"),"expect":[{"entity":{"id":task},"attr":"task/status","values":[{"string":"todo"}]}],"changes":[{"op":"assert","entity":{"id":task},"attr":"task/status","value":{"string":format!("winner-{index}")}}]});
            barrier.wait();
            let mut command = db(path); command.args(["transact","--file","-"]).write_stdin(request.to_string());
            let output = command.output().unwrap();
            if output.status.success() {
                assert!(output.stderr.is_empty());
                let report: Json = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(report["after"]["revision"],"2");
                Some((request,report))
            } else {
                use assert_cmd::assert::OutputAssertExt;
                let assertion = output.assert().code(3).stdout("");
                let error: Json = serde_json::from_slice(&assertion.get_output().stderr).unwrap();
                assert_eq!(error["error"]["code"],"PreconditionFailed");
                assert_eq!(error["error"]["commit_outcome"],"RolledBack");
                None
            }
        }) }).collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(outputs.len(), 1);
    assert_eq!(transact(&path, &outputs[0].0), outputs[0].1);
    assert_eq!(
        transact(&path, &serde_json::from_str(request).unwrap()),
        reports[0]
    );
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.head().unwrap().revision(), Revision::new(2));
    let records = reopened.db().unwrap().transactions(Revision::ZERO).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records
            .iter()
            .map(|r| r.report.created_entities.len())
            .sum::<usize>(),
        2
    );
}

#[test]
fn cli_backup_is_standalone_and_read_commands_do_not_modify_the_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.sqlite");
    let backup = dir.path().join("copy with spaces.sqlite");
    success(binary().arg("init").arg(&path));
    let a = transact(
        &path,
        &serde_json::from_str(include_str!("../../../examples/create.json")).unwrap(),
    );
    failure(db(&path).args(["backup"]), 2, "InvalidRequest");
    failure(
        db(&path).args(["backup", "--to", "x", "--at", "x"]),
        2,
        "InvalidRequest",
    );
    let copied = success(db(&path).arg("backup").arg("--to").arg(&backup));
    assert_eq!(copied, a["after"]);
    let original_bytes = std::fs::read(&backup).unwrap();
    let query = include_str!("../../../examples/query.json");
    assert_eq!(
        success(
            db(&backup)
                .args(["query", "--file", "-"])
                .write_stdin(query)
        )["rows"],
        json!([])
    );
    assert_eq!(
        success(db(&backup).arg("transactions"))["transactions"][0]["report"],
        a
    );
    assert_eq!(
        success(db(&backup).arg("schema"))["schema"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        success(db(&backup).arg("facts"))["facts"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(std::fs::read(&backup).unwrap(), original_bytes);
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(
            !dir.path()
                .join(format!("copy with spaces.sqlite{suffix}"))
                .exists()
        );
    }
    failure(
        db(&path).arg("backup").arg("--to").arg(&backup),
        5,
        "Storage",
    );
    assert_eq!(std::fs::read(&backup).unwrap(), original_bytes);
    let noop = transact(&path, &json!({"version":1}));
    assert_eq!(noop["after"]["revision"], "2");
    assert_eq!(
        success(db(&backup).arg("transactions"))["transactions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(success(binary().arg("init").arg(&path)), noop["after"]);
}
