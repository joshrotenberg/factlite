//! Stores behind handles, against the real binary over stdio: what a session
//! reads and writes, what handles allow, recovery after a kill, and several
//! server processes on one file.

mod common;

use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin;
use common::{Server, parse};
use factlite::{Db, FactFilter, Revision, SnapshotId, Store, wire};
use serde_json::{Value as Json, json};

fn server(args: &[&str]) -> Server {
    Server::start(&cargo_bin("factlite-rpc"), args)
}
fn store(directory: &Path) -> PathBuf {
    directory.join("store.sqlite")
}
fn revision(snapshot: &Json) -> u64 {
    snapshot["revision"].as_str().unwrap().parse().unwrap()
}
fn snapshot(json: &Json) -> SnapshotId {
    wire::decode_snapshot(&json.to_string()).unwrap()
}
fn library_error(error: &factlite::Error) -> Json {
    parse(&wire::encode_error(error))["error"].clone()
}

const SCHEMA: &str = r#"{"version":1,"idempotency_key":"schema","schema":[
  {"name":"task/id","type":"string","cardinality":"one","unique":"unique_one"},
  {"name":"task/title","type":"string","cardinality":"one","unique":"none"},
  {"name":"task/priority","type":"i64","cardinality":"one","unique":"none"},
  {"name":"task/tags","type":"keyword","cardinality":"many","unique":"none"},
  {"name":"task/blocks","type":"ref","cardinality":"one","unique":"none"}]}"#;

/// A new task, addressed afterwards by its `task/id`.
fn task(id: &str, title: &str, priority: i64) -> Json {
    json!({"version": 1, "idempotency_key": format!("create/{id}"), "new_entities": ["t"],
    "metadata": {"agent": {"string": "tester"}},
    "changes": [
        {"op": "assert", "entity": {"temp": "t"}, "attr": "task/id", "value": {"string": id}},
        {"op": "assert", "entity": {"temp": "t"}, "attr": "task/title", "value": {"string": title}},
        {"op": "assert", "entity": {"temp": "t"}, "attr": "task/priority", "value": {"i64": priority.to_string()}},
        {"op": "assert", "entity": {"temp": "t"}, "attr": "task/tags", "value": {"keyword": "tag/new"}},
    ]})
}
fn owner(id: &str) -> Json {
    json!({"lookup": {"attr": "task/id", "value": {"string": id}}})
}

#[test]
fn a_session_writes_and_reads_exactly_what_the_library_reads() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    let mut server = server(&[]);

    let opened = server.ok("open", json!({"path": path, "create": true}));
    let handle = opened["handle"].as_str().unwrap().to_owned();
    assert_eq!(revision(&opened["head"]), 0);

    server.ok("transact", json!({"handle": handle, "tx": parse(SCHEMA)}));
    let a = server.ok(
        "transact",
        json!({"handle": handle, "tx": task("a", "Write", 2)}),
    );
    let b = server.ok(
        "transact",
        json!({"handle": handle, "tx": task("b", "Review", 1)}),
    );
    let entity_a = a["tempids"]["t"].as_str().unwrap().to_owned();
    let middle = b["after"].clone();
    // A lookup reference in both positions, a retraction and a replacement.
    let edit = json!({"version": 1, "idempotency_key": "edit", "changes": [
        {"op": "assert", "entity": owner("a"), "attr": "task/blocks", "value": {"ref": owner("b")}},
        {"op": "assert", "entity": owner("a"), "attr": "task/priority", "value": {"i64": "5"}},
        {"op": "retract", "entity": owner("b"), "attr": "task/tags", "value": {"keyword": "tag/new"}},
    ], "expect": [{"entity": {"id": entity_a}, "attr": "task/priority", "values": [{"i64": "2"}]}]});
    let edited = server.ok("transact", json!({"handle": handle, "tx": edit}));
    assert_eq!(revision(&edited["after"]), 4);

    // The library, on the same file, through its own connection.
    let library = Db::open_read_only(&path).unwrap();
    let head = library.snapshot().clone();
    assert_eq!(
        snapshot(&server.ok("head", json!({"handle": handle}))),
        head
    );
    assert_eq!(
        snapshot(&server.ok("verify", json!({"handle": handle}))),
        head
    );
    let records = library.transactions(Revision::ZERO).unwrap();
    assert_eq!(edited, parse(&wire::encode_report(&records[3].report)));

    let id_a = entity_a.parse().unwrap();
    let query = json!({"version": 1,
        "query": {
            "inputs": {"floor": "i64"},
            "match": [
                [{"var": "t"}, "task/priority", {"var": "p"}],
                [{"var": "t"}, "task/title", {"var": "title"}]
            ],
            "where": [{"left": {"var": "p"}, "op": "ge", "right": {"param": "floor"}}],
            "not": [{"match": [[{"var": "t"}, "task/tags", {"keyword": "tag/none"}]], "where": []}],
            "select": ["title", "p"],
            "order": [{"field": "title", "direction": "asc"}],
            "limit": 10},
        "params": {"floor": {"i64": "1"}}});
    let (decoded, params) = wire::decode_query(&query.to_string()).unwrap();
    let priority = FactFilter {
        attribute: Some("task/priority".parse().unwrap()),
        ..FactFilter::default()
    };

    // Each read, at the head and at an earlier snapshot in both spellings.
    let earlier = snapshot(&middle);
    for (at, view) in [
        (None, library.clone()),
        (Some(middle.clone()), library.at(&earlier).unwrap()),
        (
            Some(json!(earlier.to_string())),
            library.at(&earlier).unwrap(),
        ),
    ] {
        let cases: Vec<(&str, Json, String)> = vec![
            (
                "schema",
                json!({}),
                wire::encode_schema(&view.schema().unwrap()),
            ),
            (
                "facts",
                json!({}),
                wire::encode_facts(&view.facts(FactFilter::default()).unwrap()),
            ),
            (
                "facts",
                json!({"filter": {"attr": "task/priority"}}),
                wire::encode_facts(&view.facts(priority.clone()).unwrap()),
            ),
            (
                "pull",
                json!({"entity": {"id": entity_a}, "attrs": ["task/title", "task/tags", "task/blocks"]}),
                wire::encode_entity(
                    view.pull(
                        &id_a,
                        &[
                            "task/title".parse().unwrap(),
                            "task/tags".parse().unwrap(),
                            "task/blocks".parse().unwrap(),
                        ],
                    )
                    .unwrap()
                    .as_ref(),
                ),
            ),
            (
                "lookup",
                json!({"attr": "task/id", "value": {"string": "b"}}),
                wire::encode_lookup(
                    view.lookup(
                        &"task/id".parse().unwrap(),
                        &factlite::Value::String("b".into()),
                    )
                    .unwrap()
                    .as_ref(),
                ),
            ),
            (
                "history",
                json!({"filter": {"attr": "task/priority"}, "after": "1"}),
                wire::encode_history(&view.history(priority.clone(), Revision::new(1)).unwrap()),
            ),
            (
                "transactions",
                json!({"after": "2"}),
                wire::encode_transactions(&view.transactions(Revision::new(2)).unwrap()),
            ),
            (
                "query",
                json!({"query": query}),
                wire::encode_query_result(&view.query(&decoded, &params).unwrap()),
            ),
        ];
        for (method, mut request, expected) in cases {
            request["handle"] = json!(handle);
            if let Some(at) = &at {
                request["at"] = at.clone();
            }
            let result = server.ok(method, request.clone());
            assert_eq!(
                snapshot(&result["basis"]),
                *view.snapshot(),
                "{method} {request}"
            );
            assert_eq!(result["value"], parse(&expected), "{method} {request}");
        }
    }
    // The two views differ, so the comparison above could tell them apart.
    let now = server.ok(
        "facts",
        json!({"handle": handle, "filter": {"attr": "task/priority"}}),
    );
    let then = server.ok(
        "facts",
        json!({"handle": handle, "filter": {"attr": "task/priority"}, "at": middle}),
    );
    assert_ne!(now["value"], then["value"]);
    assert_eq!(now["value"]["facts"].as_array().unwrap().len(), 2);

    // A backup is a copy that still answers the tokens of the original.
    let copy = directory.path().join("copy.sqlite");
    let copied = server.ok("backup", json!({"handle": handle, "to": copy}));
    assert_eq!(snapshot(&copied), head);
    let copy_handle = server.open(&copy, json!({"read_only": true}));
    let from_copy = server.ok(
        "facts",
        json!({"handle": copy_handle, "filter": {"attr": "task/priority"}, "at": middle}),
    );
    assert_eq!(from_copy, then);

    assert_eq!(server.ok("close", json!({"handle": handle})), json!({}));
    let closed = server.refused("head", json!({"handle": handle}), "InvalidRequest");
    assert_eq!(closed["context"], json!({"field": "handle"}));
    assert_eq!(closed["message"], format!("unknown handle: {handle}"));
    // The other handle is unaffected, and its name is never handed out again.
    server.ok("head", json!({"handle": copy_handle}));
    assert_eq!(server.open(&path, json!({})), "h3");
}

#[test]
fn every_library_refusal_arrives_as_the_same_error_object() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    let mut server = server(&[]);
    let handle = server.open(&path, json!({"create": true}));
    server.ok("transact", json!({"handle": handle, "tx": parse(SCHEMA)}));
    let a = server.ok(
        "transact",
        json!({"handle": handle, "tx": task("a", "Write", 2)}),
    );
    let entity = a["tempids"]["t"].clone();
    let stale = a["before"].clone();
    let library = Store::open(&path).unwrap();

    let other = tempfile::tempdir().unwrap();
    let foreign = wire::encode_snapshot(&Store::open(store(other.path())).unwrap().head().unwrap());
    let future = {
        let head = library.head().unwrap();
        SnapshotId::new(
            head.store().clone(),
            head.lineage().clone(),
            Revision::new(99),
        )
    };

    let mut changed = task("a", "Write", 2);
    changed["changes"][1]["value"] = json!({"string": "Rewrite"});
    let refusals = [
        (
            "InvalidRequest",
            json!({"version": 1, "changes": [
            {"op": "assert", "entity": {"temp": "undeclared"}, "attr": "task/title", "value": {"string": "x"}}]}),
        ),
        (
            "UnknownAttribute",
            json!({"version": 1, "changes": [
            {"op": "assert", "entity": {"id": entity}, "attr": "task/none", "value": {"string": "x"}}]}),
        ),
        (
            "TypeMismatch",
            json!({"version": 1, "changes": [
            {"op": "assert", "entity": {"id": entity}, "attr": "task/priority", "value": {"string": "high"}}]}),
        ),
        (
            "UnknownEntity",
            json!({"version": 1, "changes": [
            {"op": "assert", "entity": owner("missing"), "attr": "task/priority", "value": {"i64": "1"}}]}),
        ),
        (
            "ConflictingBatch",
            json!({"version": 1, "changes": [
            {"op": "assert", "entity": {"id": entity}, "attr": "task/priority", "value": {"i64": "1"}},
            {"op": "assert", "entity": {"id": entity}, "attr": "task/priority", "value": {"i64": "2"}}]}),
        ),
        (
            "UniqueViolation",
            task("a", "Again", 1)
                .as_object()
                .map(|request| {
                    let mut request = request.clone();
                    request.insert("idempotency_key".into(), json!("create/a-again"));
                    Json::Object(request)
                })
                .unwrap(),
        ),
        (
            "PreconditionFailed",
            json!({"version": 1,
            "expect": [{"entity": {"id": entity}, "attr": "task/priority", "values": [{"i64": "9"}]}]}),
        ),
        (
            "HeadMismatch",
            json!({"version": 1, "expected_head": stale}),
        ),
        ("IdempotencyConflict", changed),
    ];
    for (code, request) in refusals {
        let data = server.refused("transact", json!({"handle": handle, "tx": request}), code);
        // Nothing was committed, so the library refuses the same request in
        // the same words: when it is decoded, or when it is committed.
        let error = match wire::decode_tx(&request.to_string()) {
            Err(error) => error,
            Ok(request) => library.commit(request).unwrap_err(),
        };
        assert_eq!(data, library_error(&error), "{code}");
    }
    assert_eq!(revision(&server.ok("head", json!({"handle": handle}))), 2);

    // Reads.
    let view = library.db().unwrap();
    for (code, method, request, error) in [
        (
            "UnknownAttribute",
            "facts",
            json!({"filter": {"attr": "task/none"}}),
            view.facts(FactFilter {
                attribute: Some("task/none".parse().unwrap()),
                ..FactFilter::default()
            })
            .unwrap_err(),
        ),
        (
            "TypeMismatch",
            "lookup",
            json!({"attr": "task/id", "value": {"i64": "1"}}),
            view.lookup(&"task/id".parse().unwrap(), &factlite::Value::I64(1))
                .unwrap_err(),
        ),
        (
            "InvalidSnapshot",
            "schema",
            json!({"at": parse(&foreign)}),
            library
                .db_at(&wire::decode_snapshot(&foreign).unwrap())
                .err()
                .unwrap(),
        ),
        (
            "InvalidSnapshot",
            "schema",
            json!({"at": future.to_string()}),
            library.db_at(&future).err().unwrap(),
        ),
        (
            "InvalidRequest",
            "transactions",
            json!({"after": "3"}),
            view.transactions(Revision::new(3)).unwrap_err(),
        ),
    ] {
        let mut request = request;
        request["handle"] = json!(handle);
        let data = server.refused(method, request, code);
        assert_eq!(data, library_error(&error), "{method} {code}");
    }
}

#[test]
fn no_file_is_created_unless_create_is_passed() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    let mut server = server(&[]);
    let entries = || std::fs::read_dir(directory.path()).unwrap().count();

    // The CLI's error for the same case: the operating system's message,
    // with the operation in the context.
    let missing = server.refused("open", json!({"path": path}), "Storage");
    assert_eq!(missing["context"], json!({"operation": "open database"}));
    assert_eq!(
        missing["message"],
        std::fs::metadata(&path).unwrap_err().to_string()
    );
    // A writable structural open is the library's refusal, path or not.
    let structural = server.refused(
        "open",
        json!({"path": path, "validation": "structural"}),
        "InvalidRequest",
    );
    assert_eq!(structural["context"], json!({"field": "validation"}));
    server.refused("open", json!({"path": path, "create": false}), "Storage");
    server.refused("open", json!({"path": path, "read_only": true}), "Storage");
    let both = server.refused(
        "open",
        json!({"path": path, "create": true, "read_only": true}),
        "InvalidRequest",
    );
    assert_eq!(both["context"], json!({"field": "create"}));
    // A store cannot be made inside a directory that does not exist.
    server.refused(
        "open",
        json!({"path": directory.path().join("none").join("store.sqlite"), "create": true}),
        "Storage",
    );
    assert_eq!(entries(), 0, "a refused open leaves nothing behind");

    // No failed open took a handle name.
    assert_eq!(server.open(&path, json!({"create": true})), "h1");
    assert!(path.exists());
    // Creating is not exclusive: an existing store is opened as it is.
    server.ok("transact", json!({"handle": "h1", "tx": {"version": 1}}));
    let again = server.ok("open", json!({"path": path, "create": true}));
    assert_eq!(revision(&again["head"]), 1);

    // A file that is not a store is refused and left as it was.
    let other = directory.path().join("notes.txt");
    std::fs::write(&other, b"not SQLite at all, just some text").unwrap();
    for extra in [
        json!({}),
        json!({"create": true}),
        json!({"read_only": true}),
    ] {
        let mut params = json!({"path": other});
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        let error = server.err("open", params);
        assert_eq!(error["code"], -32000, "{error}");
    }
    assert_eq!(
        std::fs::read(&other).unwrap(),
        b"not SQLite at all, just some text"
    );
}

#[test]
fn a_read_only_handle_refuses_writes_and_follows_the_head() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    let mut server = server(&[]);
    let writer = server.open(&path, json!({"create": true}));
    server.ok("transact", json!({"handle": writer, "tx": parse(SCHEMA)}));

    let opened = server.ok("open", json!({"path": path, "read_only": true}));
    let reader = opened["handle"].as_str().unwrap().to_owned();
    assert_eq!(revision(&opened["head"]), 1);

    for (method, params) in [
        ("transact", json!({"handle": reader, "tx": {"version": 1}})),
        // Refused for the handle before the request is looked at.
        ("transact", json!({"handle": reader, "tx": {"version": 9}})),
        (
            "backup",
            json!({"handle": reader, "to": directory.path().join("copy.sqlite")}),
        ),
    ] {
        let refused = server.refused(method, params, "InvalidRequest");
        assert_eq!(refused["context"], json!({"field": "handle"}));
        assert_eq!(refused["message"], "the handle is read-only");
    }
    assert!(!directory.path().join("copy.sqlite").exists());

    // The writer moves on; the reader's next read with no snapshot sees it.
    server.ok(
        "transact",
        json!({"handle": writer, "tx": task("a", "Write", 2)}),
    );
    assert_eq!(revision(&server.ok("head", json!({"handle": reader}))), 2);
    let facts = server.ok(
        "facts",
        json!({"handle": reader, "filter": {"attr": "task/id"}}),
    );
    assert_eq!(revision(&facts["basis"]), 2);
    assert_eq!(facts["value"]["facts"].as_array().unwrap().len(), 1);
    // And it still reads the snapshot it was opened at.
    let before = server.ok(
        "facts",
        json!({"handle": reader, "filter": {"attr": "task/id"}, "at": opened["head"]}),
    );
    assert_eq!(before["value"]["facts"], json!([]));
    assert_eq!(
        snapshot(&server.ok("verify", json!({"handle": reader})))
            .revision()
            .get(),
        2
    );

    // Structural validation is for read-only opens.
    let structural = server.ok(
        "open",
        json!({"path": path, "read_only": true, "validation": "structural", "busy_timeout_ms": 0}),
    );
    assert_eq!(revision(&structural["head"]), 2);
    assert_eq!(
        revision(&server.ok("verify", json!({"handle": structural["handle"]}))),
        2
    );
    let refused = server.refused(
        "open",
        json!({"path": path, "validation": "structural"}),
        "InvalidRequest",
    );
    assert_eq!(
        refused,
        library_error(
            &Store::open_with(
                &path,
                &factlite::StoreOptions::new().with_validation(factlite::Validation::Structural)
            )
            .err()
            .unwrap()
        )
    );
    server.ok(
        "open",
        json!({"path": path, "validation": "full", "busy_timeout_ms": 250}),
    );
    // A timeout the library will not take is the library's refusal.
    server.refused(
        "open",
        json!({"path": path, "busy_timeout_ms": 4294967296u64}),
        "InvalidRequest",
    );
}

#[test]
fn every_method_fails_on_a_handle_that_is_not_open() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    let mut server = server(&[]);
    let handle = server.open(&path, json!({"create": true}));
    server.ok("close", json!({"handle": handle}));

    for name in [handle.as_str(), "h2", "", "H1", "h01"] {
        for (method, mut params) in [
            ("close", json!({})),
            ("head", json!({})),
            ("verify", json!({})),
            ("transact", json!({"tx": {"version": 1}})),
            (
                "backup",
                json!({"to": directory.path().join("copy.sqlite")}),
            ),
            ("schema", json!({})),
            ("facts", json!({})),
            (
                "pull",
                json!({"entity": {"id": "e1.00000000000000000000000000000001.0000000000000001"}, "attrs": []}),
            ),
            ("lookup", json!({"attr": "a/b", "value": {"i64": "1"}})),
            ("history", json!({})),
            ("transactions", json!({})),
            (
                "query",
                json!({"query": {"version": 1, "query": {"match": [], "select": []}, "params": {}}}),
            ),
        ] {
            params["handle"] = json!(name);
            let refused = server.refused(method, params, "InvalidRequest");
            assert_eq!(refused["context"], json!({"field": "handle"}), "{method}");
            assert_eq!(
                refused["message"],
                format!("unknown handle: {name}"),
                "{method}"
            );
        }
    }
    assert!(!directory.path().join("copy.sqlite").exists());
    assert_eq!(
        Store::open(&path).unwrap().head().unwrap().revision().get(),
        0
    );
}

#[test]
fn a_server_told_to_require_keys_refuses_a_transaction_without_one() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    let mut server = server(&["--require-idempotency-key"]);
    let handle = server.open(&path, json!({"create": true}));

    let refused = server.refused(
        "transact",
        json!({"handle": handle, "tx": {"version": 1}}),
        "InvalidRequest",
    );
    assert_eq!(refused["context"], json!({"field": "idempotency_key"}));
    assert_eq!(revision(&server.ok("head", json!({"handle": handle}))), 0);
    // A request that does not decode is still the wire's refusal.
    let wire = server.refused(
        "transact",
        json!({"handle": handle, "tx": {"version": 2}}),
        "InvalidRequest",
    );
    assert_eq!(wire["context"], json!({"field": "version"}));

    let keyed = json!({"handle": handle, "tx": {"version": 1, "idempotency_key": "only"}});
    let first = server.ok("transact", keyed.clone());
    assert_eq!(server.ok("transact", keyed), first);
    assert_eq!(revision(&server.ok("head", json!({"handle": handle}))), 1);
}

#[test]
fn a_busy_store_fails_busy_and_the_keyed_retry_then_commits_once() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    let mut server = server(&[]);
    let handle = server.open(&path, json!({"create": true, "busy_timeout_ms": 0}));

    let blocker = rusqlite::Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let request = json!({"handle": handle, "tx": {"version": 1, "idempotency_key": "once"}});
    let started = std::time::Instant::now();
    let busy = server.refused("transact", request.clone(), "Busy");
    // A zero timeout fails at once, well inside the two-second default.
    assert!(started.elapsed() < std::time::Duration::from_millis(1500));
    assert_eq!(busy["commit_outcome"], "RolledBack", "{busy}");
    // The library's own error for the same refusal.
    let library = Store::open_with(
        &path,
        &factlite::StoreOptions::new()
            .with_busy_timeout(std::time::Duration::ZERO)
            .unwrap(),
    );
    let library = match library {
        Ok(store) => {
            store.commit(wire::decode_tx(r#"{"version":1,"idempotency_key":"once"}"#).unwrap())
        }
        Err(error) => Err(error),
    }
    .unwrap_err();
    assert_eq!(busy, library_error(&library));
    // Reads are not blocked by a writer.
    assert_eq!(
        revision(&server.ok("facts", json!({"handle": handle}))["basis"]),
        0
    );
    // A handle opened with a timeout longer than the library's two-second
    // default waits that long before failing, so the timeout given is the
    // one applied. The lower bound is exact; no upper bound is assumed.
    let patient = server.open(&path, json!({"busy_timeout_ms": 2600}));
    let started = std::time::Instant::now();
    server.refused(
        "transact",
        json!({"handle": patient, "tx": {"version": 1}}),
        "Busy",
    );
    assert!(started.elapsed() >= std::time::Duration::from_millis(2600));
    blocker.execute_batch("ROLLBACK").unwrap();

    let report = server.ok("transact", request.clone());
    assert_eq!(revision(&report["after"]), 1);
    assert_eq!(server.ok("transact", request), report);
    assert_eq!(revision(&server.ok("head", json!({"handle": handle}))), 1);
}

#[test]
fn a_keyed_retry_after_the_server_is_killed_returns_the_original_report() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    let request = |handle: &str, key: &str| {
        json!({"handle": handle, "tx": {"version": 1, "idempotency_key": key, "new_entities": ["e"],
            "metadata": {"attempt": {"string": "same every time"}}}})
    };

    // The client saw the report, and then the server died.
    let mut first = server(&[]);
    let handle = first.open(&path, json!({"create": true}));
    let report = first.ok("transact", request(&handle, "seen"));
    first.kill();

    let mut second = server(&[]);
    let handle = second.open(&path, json!({}));
    assert_eq!(second.ok("transact", request(&handle, "seen")), report);
    assert_eq!(revision(&second.ok("head", json!({"handle": handle}))), 1);

    // The client sent the request and never saw an answer. Whether the
    // commit happened is unknown to it; the retry settles it either way.
    second.send(
        &json!({"jsonrpc":"2.0","id":"lost","method":"transact","params":request(&handle, "unseen")})
            .to_string(),
    );
    second.kill();

    let mut third = server(&[]);
    let handle = third.open(&path, json!({}));
    let recovered = third.ok("transact", request(&handle, "unseen"));
    assert_eq!(revision(&recovered["after"]), 2);
    assert_eq!(third.ok("transact", request(&handle, "unseen")), recovered);
    assert_eq!(revision(&third.ok("head", json!({"handle": handle}))), 2);
    // One entity per key, whichever process committed it.
    let log = third.ok("transactions", json!({"handle": handle}));
    let created: Vec<usize> = log["value"]["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            record["report"]["created_entities"]
                .as_array()
                .unwrap()
                .len()
        })
        .collect();
    assert_eq!(created, [1, 1]);

    // The commit happened, and its response was never read: the server is
    // killed only once the file shows the revision.
    let mut fourth = server(&[]);
    let handle = fourth.open(&path, json!({}));
    fourth.send(
        &json!({"jsonrpc":"2.0","id":"lost","method":"transact","params":request(&handle, "committed")})
            .to_string(),
    );
    let deadline = std::time::Instant::now() + common::PATIENCE;
    while Db::open_read_only(&path)
        .unwrap()
        .snapshot()
        .revision()
        .get()
        < 3
    {
        assert!(std::time::Instant::now() < deadline, "the commit landed");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    fourth.kill();
    let mut fifth = server(&[]);
    let handle = fifth.open(&path, json!({}));
    let report = fifth.ok("transact", request(&handle, "committed"));
    assert_eq!(revision(&report["after"]), 3);
    assert_eq!(revision(&fifth.ok("head", json!({"handle": handle}))), 3);
    drop(fifth);
}

#[test]
fn server_processes_on_one_file_lose_no_commit_and_repeat_none() {
    const PROCESSES: usize = 4;
    const COMMITS: usize = 25;
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    {
        let mut setup = server(&[]);
        let handle = setup.open(&path, json!({"create": true}));
        setup.ok("transact", json!({"handle": handle, "tx": parse(SCHEMA)}));
        setup.ok(
            "transact",
            json!({"handle": handle, "tx": task("shared", "Contended", 0)}),
        );
    }
    let shared = Db::open_read_only(&path)
        .unwrap()
        .lookup(
            &"task/id".parse().unwrap(),
            &factlite::Value::String("shared".into()),
        )
        .unwrap()
        .unwrap()
        .to_string();

    // Every process is started and has the store open before any may write.
    let barrier = std::sync::Barrier::new(PROCESSES);
    let winners: Vec<bool> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..PROCESSES)
            .map(|process| {
                let (path, barrier, shared) = (&path, &barrier, &shared);
                scope.spawn(move || {
                    let mut server = server(&[]);
                    let handle = server.open(path, json!({"busy_timeout_ms": 30000}));
                    barrier.wait();
                    // One change that only one process may make.
                    let claim = server.call(
                        "transact",
                        json!({"handle": handle, "tx": {"version": 1,
                            "changes": [{"op": "assert", "entity": {"id": shared}, "attr": "task/priority",
                                "value": {"i64": (process + 1).to_string()}}],
                            "expect": [{"entity": {"id": shared}, "attr": "task/priority", "values": [{"i64": "0"}]}]}}),
                    );
                    let won = claim.get("result").is_some();
                    if !won {
                        assert_eq!(claim["error"]["data"]["code"], "PreconditionFailed", "{claim}");
                    }
                    for commit in 0..COMMITS {
                        let request = json!({"handle": handle,
                            "tx": task(&format!("p{process}-{commit}"), "Parallel", commit as i64)});
                        let report = server.ok("transact", request.clone());
                        // Sending it again changes nothing and says the same.
                        assert_eq!(server.ok("transact", request), report);
                    }
                    won
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect()
    });
    assert_eq!(winners.iter().filter(|won| **won).count(), 1, "{winners:?}");

    let view = Db::open_read_only(&path).unwrap();
    assert_eq!(
        view.snapshot().revision().get() as usize,
        2 + 1 + PROCESSES * COMMITS,
        "one revision per distinct request, and one for the single winner"
    );
    let ids = view
        .facts(FactFilter {
            attribute: Some("task/id".parse().unwrap()),
            ..FactFilter::default()
        })
        .unwrap();
    assert_eq!(ids.len(), 1 + PROCESSES * COMMITS);
    view.verify().unwrap();
}

#[test]
fn structural_opens_skip_the_log_and_verify_checks_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    {
        let store = Store::open(&path).unwrap();
        store.commit(wire::decode_tx(SCHEMA).unwrap()).unwrap();
    }
    // Damage a retained report without touching the head or the index.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("UPDATE transactions SET report = X'00'")
        .unwrap();
    let mut server = server(&[]);
    // A full open replays the log and refuses the file.
    let full = server.refused("open", json!({"path": path}), "Storage");
    assert_eq!(full, library_error(&Store::open(&path).err().unwrap()));
    server.refused("open", json!({"path": path, "read_only": true}), "Storage");
    // A structural open does not, and verify then finds the damage.
    let handle = server.open(
        &path,
        json!({"read_only": true, "validation": "structural"}),
    );
    let verified = server.refused("verify", json!({"handle": handle}), "Storage");
    let library = Db::open_read_only_with(
        &path,
        &factlite::StoreOptions::new().with_validation(factlite::Validation::Structural),
    )
    .unwrap()
    .verify()
    .unwrap_err();
    assert_eq!(verified, library_error(&library));
}

#[test]
fn a_store_of_an_unknown_format_is_refused_as_the_library_refuses_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    Store::open(&path).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    let mut server = server(&[]);
    for extra in [json!({}), json!({"read_only": true})] {
        let mut params = json!({"path": path});
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        let refused = server.refused("open", params, "UnsupportedFormat");
        let library = if extra.as_object().unwrap().is_empty() {
            Store::open(&path).err().unwrap()
        } else {
            Db::open_read_only(&path).err().unwrap()
        };
        assert_eq!(refused, library_error(&library));
    }
}
