//! The JSON-RPC rules, against the real binary over stdio.

mod common;

use std::{path::Path, time::Duration};

use assert_cmd::{Command, cargo::cargo_bin};
use common::Server;
use serde_json::{Value as Json, json};

fn server(args: &[&str]) -> Server {
    Server::start(&cargo_bin("factlite-rpc"), args)
}
fn store(directory: &Path) -> std::path::PathBuf {
    directory.join("store.sqlite")
}
/// A request that commits one empty revision.
fn commit(handle: &str) -> Json {
    json!({"handle": handle, "tx": {"version": 1}})
}
fn revision(snapshot: &Json) -> &str {
    snapshot["revision"].as_str().unwrap()
}
/// The error of a response that must carry no result.
fn error_of(response: &Json, code: i64) -> &Json {
    assert_eq!(response["jsonrpc"], "2.0", "{response}");
    assert!(response.get("result").is_none(), "{response}");
    assert_eq!(response["error"]["code"], code, "{response}");
    assert!(response["error"]["message"].is_string(), "{response}");
    &response["error"]
}

#[test]
fn a_response_repeats_the_id_exactly_as_sent() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = server(&[]);
    let handle = server.open(&store(directory.path()), json!({"create": true}));
    for id in [
        "1",
        "0",
        "-0",
        "1.50",
        "1e3",
        "-12.5E-2",
        "184467440737095516161234567890",
        r#""a b""#,
        r#""""#,
        r#""a\n""#,
        r#""1""#,
        r#""a\u0020b""#,
        "null",
    ] {
        server.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"head","params":{{"handle":"{handle}"}}}}"#
        ));
        let line = server.line();
        assert!(
            line.starts_with(&format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{"#)),
            "{id}: {line}"
        );
        // An error repeats it the same way.
        server.send(&format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"none"}}"#));
        let line = server.line();
        assert!(
            line.starts_with(&format!(
                r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32601,"#
            )),
            "{id}: {line}"
        );
    }
    for id in [
        "true",
        "false",
        "{}",
        "[]",
        "[1]",
        r#"{"id":1}"#,
        r#""\ud800""#,
    ] {
        server.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"head","params":{{"handle":"{handle}"}}}}"#
        ));
        let response = server.response();
        error_of(&response, -32600);
        assert_eq!(response["id"], Json::Null, "{id}");
    }
}

#[test]
fn a_notification_is_not_executed_and_gets_no_reply() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    let mut server = server(&[]);
    let handle = server.open(&path, json!({"create": true}));
    assert_eq!(handle, "h1");

    // Each of these would change something if it ran.
    for (method, params) in [
        ("transact", commit(&handle)),
        ("open", json!({"path": path})),
        ("close", json!({"handle": handle})),
        (
            "backup",
            json!({"handle": handle, "to": directory.path().join("copy.sqlite")}),
        ),
        ("shutdown", json!({})),
        ("head", json!({"handle": handle})),
        ("no-such-method", json!({})),
    ] {
        server.send(&json!({"jsonrpc":"2.0","method":method,"params":params}).to_string());
    }
    // The next answer is to the next request: nothing was said in between,
    // the store did not move, the handle is still open, no handle was
    // handed out, and no copy was written.
    assert_eq!(revision(&server.ok("head", json!({"handle": handle}))), "0");
    assert_eq!(server.open(&path, json!({})), "h2");
    assert!(!directory.path().join("copy.sqlite").exists());

    let (status, rest, stderr) = server.finish();
    assert!(status.success());
    assert!(rest.is_empty(), "{rest:?}");
    assert_eq!(stderr.lines().count(), 7, "{stderr}");
    for method in ["transact", "open", "close", "backup", "shutdown", "head"] {
        assert!(
            stderr.contains(&format!(
                "ignored a notification, which cannot be answered: \"{method}\""
            )),
            "{stderr}"
        );
    }
}

#[test]
fn a_batch_answers_each_entry_and_refuses_methods_that_depend_on_order() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    let mut server = server(&[]);
    let handle = server.open(&path, json!({"create": true}));
    let head = json!({"handle": handle});

    let batch = json!([
        {"jsonrpc":"2.0","id":"a","method":"head","params":head},
        {"jsonrpc":"2.0","id":"b","method":"transact","params":commit(&handle)},
        {"jsonrpc":"2.0","id":"c","method":"open","params":{"path":path}},
        {"jsonrpc":"2.0","id":"d","method":"close","params":head},
        {"jsonrpc":"2.0","id":"e","method":"backup","params":{"handle":handle,"to":directory.path().join("copy.sqlite")}},
        {"jsonrpc":"2.0","id":"f","method":"shutdown"},
        {"jsonrpc":"2.0","method":"head","params":head},
        {"jsonrpc":"2.0","method":"transact","params":commit(&handle)},
        7,
        {"jsonrpc":"2.0","id":"g","method":"facts","params":head},
        {"jsonrpc":"2.0","id":"h","method":"none"},
        {"jsonrpc":"2.0","id":"i","method":"facts","params":{"handle":"h9"}},
    ]);
    server.send(&batch.to_string());
    let responses = server.response();
    let responses = responses
        .as_array()
        .expect("a batch is answered by an array");
    let ids: Vec<Json> = responses
        .iter()
        .map(|response| response["id"].clone())
        .collect();
    assert_eq!(
        Json::Array(ids),
        json!(["a", "b", "c", "d", "e", "f", null, "g", "h", "i"])
    );
    assert_eq!(revision(&responses[0]["result"]), "0");
    for (index, method) in [
        (1, "transact"),
        (2, "open"),
        (3, "close"),
        (4, "backup"),
        (5, "shutdown"),
    ] {
        let error = error_of(&responses[index], -32600);
        assert_eq!(
            error["message"],
            format!("{method} is not allowed in a batch")
        );
    }
    error_of(&responses[6], -32600);
    assert_eq!(revision(&responses[7]["result"]["basis"]), "0");
    error_of(&responses[8], -32601);
    assert_eq!(
        error_of(&responses[9], -32000)["data"]["code"],
        "InvalidRequest"
    );

    // Nothing in the batch ran that should not have.
    assert_eq!(revision(&server.ok("head", head.clone())), "0");
    assert_eq!(server.open(&path, json!({})), "h2");
    assert!(!directory.path().join("copy.sqlite").exists());

    // A duplicate member inside one entry fails that entry alone.
    server.send(&format!(
        r#"[{{"jsonrpc":"2.0","id":1,"id":2,"method":"head","params":{head}}},{{"jsonrpc":"2.0","id":3,"method":"head","params":{head}}}]"#
    ));
    let responses = server.response();
    error_of(&responses[0], -32600);
    assert_eq!(responses[0]["id"], Json::Null);
    assert_eq!(responses[1]["id"], 3);
    assert_eq!(revision(&responses[1]["result"]), "0");

    // An entry that is JSON but cannot be held, such as a number out of
    // range, is an invalid request; its neighbours are served.
    server.send(&format!(
        r#"[1e99999,{{"jsonrpc":"2.0","id":4,"method":"head","params":{head}}}]"#
    ));
    let responses = server.response();
    error_of(&responses[0], -32600);
    assert_eq!(responses[0]["id"], Json::Null);
    assert_eq!(revision(&responses[1]["result"]), "0");

    // An empty batch is one error, not an array.
    server.send("[]");
    let response = server.response();
    error_of(&response, -32600);
    assert_eq!(response["id"], Json::Null);

    // A batch of notifications is answered by nothing at all.
    server.send(&json!([{"jsonrpc":"2.0","method":"head","params":head}]).to_string());
    assert_eq!(revision(&server.ok("head", head)), "0");
}

#[test]
fn malformed_lines_fail_with_the_standard_codes_and_the_server_keeps_serving() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = server(&[]);
    let handle = server.open(&store(directory.path()), json!({"create": true}));
    let head = format!(r#"{{"handle":"{handle}"}}"#);

    for (line, code) in [
        // Not JSON.
        ("not json".to_owned(), -32700),
        ("{".to_owned(), -32700),
        (
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":"head","params":{head}}} trailing"#),
            -32700,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"head","params":{"handle":}}"#.to_owned(),
            -32700,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"head","params":{"handle":"h1"}"#.to_owned(),
            -32700,
        ),
        // JSON, but not a request.
        ("1".to_owned(), -32600),
        ("null".to_owned(), -32600),
        (r#""head""#.to_owned(), -32600),
        ("{}".to_owned(), -32600),
        (
            format!(r#"{{"id":1,"method":"head","params":{head}}}"#),
            -32600,
        ),
        (
            format!(r#"{{"jsonrpc":"1.0","id":1,"method":"head","params":{head}}}"#),
            -32600,
        ),
        (
            format!(r#"{{"jsonrpc":2.0,"id":1,"method":"head","params":{head}}}"#),
            -32600,
        ),
        (
            format!(r#"{{"jsonrpc":"2.0","id":1,"params":{head}}}"#),
            -32600,
        ),
        (
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":7,"params":{head}}}"#),
            -32600,
        ),
        (
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":["head"],"params":{head}}}"#),
            -32600,
        ),
        (
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":"head","params":{head},"extra":1}}"#),
            -32600,
        ),
        // A syntax error is one even after a repeated key.
        (r#"{"jsonrpc":"2.0","id":1,"id":}"#.to_owned(), -32700),
        (
            r#"{"jsonrpc":"2.0","id":1,"id":2} trailing"#.to_owned(),
            -32700,
        ),
        // JSON that is not an object, however large its number.
        ("1e400".to_owned(), -32600),
        // A repeated member, however it is spelled.
        (
            format!(r#"{{"jsonrpc":"2.0","id":1,"id":1,"method":"head","params":{head}}}"#),
            -32600,
        ),
        (
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"none","method":"head","params":{head}}}"#
            ),
            -32600,
        ),
        (
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":"head","params":{{}},"params":{head}}}"#),
            -32600,
        ),
        (
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"head","\u0070arams":{{}},"params":{head}}}"#
            ),
            -32600,
        ),
        (
            format!(
                r#"{{"jsonrpc":"2.0","jsonrpc":"2.0","id":1,"method":"head","params":{head}}}"#
            ),
            -32600,
        ),
    ] {
        server.send(&line);
        let response = server.response();
        error_of(&response, code);
        assert_eq!(response["id"], Json::Null, "{line}");
    }

    // Bytes that are not UTF-8.
    server.send_bytes(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"\xff\"}");
    let response = server.response();
    assert_eq!(error_of(&response, -32700)["message"], "line is not UTF-8");
    assert_eq!(response["id"], Json::Null);

    // Blank lines say nothing, with either line ending.
    server.send("");
    server.send("  \t ");
    server.send("\r");
    server.send(&format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"head\",\"params\":{head}}}\r"
    ));
    let response = server.response();
    assert_eq!(response["id"], 9);
    assert_eq!(revision(&response["result"]), "0");
}

#[test]
fn params_are_an_object_of_known_members_and_the_wire_decodes_its_own() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    let mut server = server(&[]);
    let handle = server.open(&path, json!({"create": true}));

    // Params of the wrong shape, with the id still answered.
    for params in ["null", "1", r#""h1""#, "[]", r#"["h1"]"#, "true"] {
        server.send(&format!(
            r#"{{"jsonrpc":"2.0","id":5,"method":"head","params":{params}}}"#
        ));
        let response = server.response();
        error_of(&response, -32602);
        assert_eq!(response["id"], 5, "{params}");
    }
    server.send(r#"{"jsonrpc":"2.0","id":6,"method":"head"}"#);
    let response = server.response();
    assert_eq!(
        error_of(&response, -32602)["message"],
        "invalid params: missing member: handle"
    );
    assert_eq!(response["id"], 6);
    // shutdown is the one method that needs none; it is checked elsewhere.

    let invalid = |server: &mut Server, method: &str, params: Json, message: &str| {
        let error = server.err(method, params.clone());
        assert_eq!(error["code"], -32602, "{method} {params}: {error}");
        assert_eq!(
            error["message"],
            format!("invalid params: {message}"),
            "{method} {params}"
        );
        assert!(error.get("data").is_none(), "{error}");
    };
    // Members this interface defines.
    invalid(&mut server, "head", json!({}), "missing member: handle");
    invalid(
        &mut server,
        "head",
        json!({"handle": 1}),
        "handle must be a string",
    );
    invalid(
        &mut server,
        "head",
        json!({"handle": null}),
        "handle must be a string",
    );
    invalid(
        &mut server,
        "head",
        json!({"handle": handle, "at": null}),
        "unknown member: at",
    );
    invalid(
        &mut server,
        "close",
        json!({"handle": handle, "force": true}),
        "unknown member: force",
    );
    invalid(
        &mut server,
        "verify",
        json!({"handle": [handle]}),
        "handle must be a string",
    );
    invalid(&mut server, "open", json!({}), "missing member: path");
    invalid(
        &mut server,
        "open",
        json!({"path": 1}),
        "path must be a string",
    );
    invalid(
        &mut server,
        "open",
        json!({"path": path, "create": "yes"}),
        "create must be a boolean",
    );
    invalid(
        &mut server,
        "open",
        json!({"path": path, "create": 1}),
        "create must be a boolean",
    );
    invalid(
        &mut server,
        "open",
        json!({"path": path, "read_only": null}),
        "read_only must be a boolean",
    );
    invalid(
        &mut server,
        "open",
        json!({"path": path, "readonly": true}),
        "unknown member: readonly",
    );
    for timeout in [
        json!(-1),
        json!(1.5),
        json!("5"),
        json!(null),
        json!(18446744073709551616.0),
    ] {
        invalid(
            &mut server,
            "open",
            json!({"path": path, "busy_timeout_ms": timeout}),
            "busy_timeout_ms must be an unsigned integer",
        );
    }
    for validation in [json!("none"), json!("Full"), json!(true), json!(null)] {
        invalid(
            &mut server,
            "open",
            json!({"path": path, "validation": validation}),
            "validation must be \"full\" or \"structural\"",
        );
    }
    invalid(
        &mut server,
        "backup",
        json!({"handle": handle}),
        "missing member: to",
    );
    invalid(
        &mut server,
        "backup",
        json!({"handle": handle, "to": 1}),
        "to must be a string",
    );
    invalid(
        &mut server,
        "transact",
        json!({"handle": handle}),
        "missing member: tx",
    );
    invalid(
        &mut server,
        "transact",
        json!({"handle": handle, "tx": {"version": 1}, "key": "k"}),
        "unknown member: key",
    );
    invalid(
        &mut server,
        "pull",
        json!({"handle": handle, "entity": {"id": "x"}}),
        "missing member: attrs",
    );
    invalid(
        &mut server,
        "pull",
        json!({"handle": handle, "attrs": []}),
        "missing member: entity",
    );
    invalid(
        &mut server,
        "lookup",
        json!({"handle": handle, "attr": "a/b"}),
        "missing member: value",
    );
    invalid(
        &mut server,
        "query",
        json!({"handle": handle}),
        "missing member: query",
    );
    invalid(
        &mut server,
        "schema",
        json!({"handle": handle, "filter": {}}),
        "unknown member: filter",
    );
    invalid(
        &mut server,
        "facts",
        json!({"handle": handle, "after": "0"}),
        "unknown member: after",
    );
    invalid(
        &mut server,
        "transactions",
        json!({"handle": handle, "filter": {}}),
        "unknown member: filter",
    );
    invalid(
        &mut server,
        "shutdown",
        json!({"now": true}),
        "unknown member: now",
    );
    // A member is unknown before a handle is.
    invalid(
        &mut server,
        "facts",
        json!({"handle": "h9", "extra": 1}),
        "unknown member: extra",
    );

    // A repeated member of params names the member and keeps the id.
    server.send(&format!(
        r#"{{"jsonrpc":"2.0","id":7,"method":"head","params":{{"handle":"{handle}","handle":"{handle}"}}}}"#
    ));
    let response = server.response();
    assert_eq!(
        error_of(&response, -32602)["message"],
        "invalid params: duplicate object key: handle"
    );
    assert_eq!(response["id"], 7);

    // Members the wire contract defines are decoded by the wire, and fail as
    // it fails: a Factlite error under the request's own id.
    let wire = |server: &mut Server, method: &str, params: Json| {
        server.refused(method, params, "InvalidRequest");
    };
    wire(
        &mut server,
        "facts",
        json!({"handle": handle, "filter": null}),
    );
    wire(
        &mut server,
        "facts",
        json!({"handle": handle, "filter": {"attr": 5}}),
    );
    wire(
        &mut server,
        "facts",
        json!({"handle": handle, "filter": {"unknown": 1}}),
    );
    wire(
        &mut server,
        "pull",
        json!({"handle": handle, "entity": "e1", "attrs": []}),
    );
    wire(
        &mut server,
        "pull",
        json!({"handle": handle, "entity": {"temp": "a"}, "attrs": []}),
    );
    wire(
        &mut server,
        "lookup",
        json!({"handle": handle, "attr": 5, "value": {"i64": "1"}}),
    );
    wire(
        &mut server,
        "lookup",
        json!({"handle": handle, "attr": "a/b", "value": 1}),
    );
    wire(
        &mut server,
        "history",
        json!({"handle": handle, "after": 0}),
    );
    wire(
        &mut server,
        "history",
        json!({"handle": handle, "after": "01"}),
    );
    wire(
        &mut server,
        "transactions",
        json!({"handle": handle, "after": null}),
    );
    wire(
        &mut server,
        "query",
        json!({"handle": handle, "query": {"version": 2}}),
    );
    wire(
        &mut server,
        "transact",
        json!({"handle": handle, "tx": {"version": 1, "extra": 1}}),
    );
    wire(
        &mut server,
        "transact",
        json!({"handle": handle, "tx": null}),
    );
    // Their exact error objects are compared with the decoders' own in
    // a_malformed_wire_member_fails_with_the_decoders_own_error.
    assert_eq!(revision(&server.ok("head", json!({"handle": handle}))), "0");
}

#[test]
fn an_oversized_line_is_refused_whole_and_one_at_the_limit_is_served() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = server(&["--max-line-bytes", "512"]);
    let handle = server.open(&store(directory.path()), json!({"create": true}));
    let request = |length: usize| {
        let core = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"head","params":{{"handle":"{handle}"}}}}"#
        );
        format!("{core}{}", " ".repeat(length - core.len()))
    };

    server.send(&request(512));
    assert_eq!(revision(&server.response()["result"]), "0");

    server.send(&request(513));
    let response = server.response();
    assert_eq!(
        error_of(&response, -32700)["message"],
        "line is longer than 512 bytes"
    );
    assert_eq!(response["id"], Json::Null);

    // Far past the limit, and past any pipe buffer: still one refusal, and
    // the line after it is read from its own start.
    server.send(&"x".repeat(4 * 1024 * 1024));
    error_of(&server.response(), -32700);
    server.send(&request(100));
    assert_eq!(revision(&server.response()["result"]), "0");

    let (status, rest, _) = server.finish();
    assert!(status.success());
    assert!(rest.is_empty(), "{rest:?}");
}

#[test]
fn end_of_input_ends_the_process_and_a_last_line_needs_no_newline() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    let mut server = server(&[]);
    let handle = server.open(&path, json!({"create": true}));
    server.send_unterminated(
        format!(
            r#"{{"jsonrpc":"2.0","id":"last","method":"transact","params":{}}}"#,
            commit(&handle)
        )
        .as_bytes(),
    );
    let (status, rest, stderr) = server.finish();
    assert!(status.success(), "{status:?}");
    assert_eq!(stderr, "");
    assert_eq!(rest.len(), 1, "{rest:?}");
    let response = common::parse(&rest[0]);
    assert_eq!(response["id"], "last");
    assert_eq!(revision(&response["result"]["after"]), "1");
    assert_eq!(
        factlite::Store::open(&path)
            .unwrap()
            .head()
            .unwrap()
            .revision()
            .get(),
        1
    );
}

#[test]
fn shutdown_answers_then_ends_the_process_without_reading_further() {
    let directory = tempfile::tempdir().unwrap();
    let path = store(directory.path());
    let mut server = server(&[]);
    let handle = server.open(&path, json!({"create": true}));
    server.ok("transact", commit(&handle));

    // Both lines are written before the server can act on the first.
    server.send_unterminated(
        format!(
            "{}\n{}\n",
            json!({"jsonrpc":"2.0","id":"bye","method":"shutdown"}),
            json!({"jsonrpc":"2.0","id":"late","method":"transact","params":commit(&handle)}),
        )
        .as_bytes(),
    );
    let response = server.response();
    assert_eq!(response["id"], "bye");
    assert_eq!(response["result"], json!({}));
    // stdin is still open: the process ends because it was asked to.
    assert!(server.wait().success());
    let (_, rest, stderr) = server.finish();
    assert!(rest.is_empty(), "{rest:?}");
    assert_eq!(stderr, "");
    assert_eq!(
        factlite::Store::open(&path)
            .unwrap()
            .head()
            .unwrap()
            .revision()
            .get(),
        1
    );
}

#[test]
fn arguments_are_checked_before_anything_is_served() {
    let run = |args: &[&str]| {
        let mut command = Command::new(cargo_bin("factlite-rpc"));
        command.args(args).timeout(Duration::from_secs(20));
        command
    };
    let help = run(&["--help"]).assert().success().stderr("");
    let help = String::from_utf8(help.get_output().stdout.clone()).unwrap();
    assert!(help.starts_with("Usage:\n  factlite-rpc "), "{help}");
    for word in [
        "--max-line-bytes",
        "--require-idempotency-key",
        "shutdown",
        "Exit codes",
    ] {
        assert!(help.contains(word), "{word}");
    }
    run(&["--version"])
        .assert()
        .success()
        .stderr("")
        .stdout(format!("factlite-rpc {}\n", env!("CARGO_PKG_VERSION")));

    for args in [
        &["--unknown"][..],
        &["serve"],
        &["--help", "--version"],
        &["--max-line-bytes"],
        &["--max-line-bytes", ""],
        &["--max-line-bytes", "-1"],
        &["--max-line-bytes", "1k"],
        &["--max-line-bytes", "99999999999999999999999"],
        &["--max-line-bytes", "1", "--max-line-bytes", "2"],
        &["--require-idempotency-key", "--require-idempotency-key"],
    ] {
        let failed = run(args).write_stdin("").assert().code(2).stdout("");
        let stderr = String::from_utf8(failed.get_output().stderr.clone()).unwrap();
        assert!(stderr.starts_with("factlite-rpc: "), "{args:?}: {stderr}");
        assert_eq!(stderr.lines().count(), 1, "{args:?}: {stderr}");
    }

    // Help and version on a pipe nobody reads fail with exit code 5.
    for flag in ["--help", "--version"] {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let output = std::process::Command::new(cargo_bin("factlite-rpc"))
            .arg(flag)
            .stdout(writer)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(5), "{flag}");
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .starts_with("factlite-rpc: ")
        );
    }

    // With no input at all the server ends cleanly and says nothing.
    run(&[])
        .write_stdin("")
        .assert()
        .success()
        .stdout("")
        .stderr("");
    run(&["--max-line-bytes", "0"])
        .write_stdin("\n\n")
        .assert()
        .success()
        .stdout("")
        .stderr("");
}

#[test]
fn a_malformed_wire_member_fails_with_the_decoders_own_error() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = server(&[]);
    let handle = server.open(&store(directory.path()), json!({"create": true}));
    const ENTITY: &str = r#"{"id":"e1.00000000000000000000000000000001.0000000000000001"}"#;
    const QUERY: &str = r#"{"version":1,"query":{"match":[],"select":[]},"params":{}}"#;
    let library = |error: factlite::Error| -> Json {
        common::parse(&factlite::wire::encode_error(&error))["error"].clone()
    };
    use factlite::wire;
    // (method, the other members, the member under test, its text, the
    // library's own error for that text)
    let cases: Vec<(&str, String, &str, &str, factlite::Error)> = vec![
        (
            "facts",
            String::new(),
            "filter",
            "null",
            wire::decode_filter("null").unwrap_err(),
        ),
        (
            "facts",
            String::new(),
            "filter",
            r#"{"attr":5}"#,
            wire::decode_filter(r#"{"attr":5}"#).unwrap_err(),
        ),
        (
            "facts",
            String::new(),
            "filter",
            r#"{"attr":"a/b","attr":"a/b"}"#,
            wire::decode_filter(r#"{"attr":"a/b","attr":"a/b"}"#).unwrap_err(),
        ),
        (
            "facts",
            String::new(),
            "filter",
            r#"{"value":{"i64":"3","i64":"4"}}"#,
            wire::decode_filter(r#"{"value":{"i64":"3","i64":"4"}}"#).unwrap_err(),
        ),
        (
            "pull",
            r#","attrs":[]"#.into(),
            "entity",
            r#""e1""#,
            wire::decode_entity_id(r#""e1""#).unwrap_err(),
        ),
        (
            "pull",
            r#","attrs":[]"#.into(),
            "entity",
            r#"{"temp":"a"}"#,
            wire::decode_entity_id(r#"{"temp":"a"}"#).unwrap_err(),
        ),
        (
            "pull",
            format!(r#","entity":{ENTITY}"#),
            "attrs",
            r#"["a/b",1]"#,
            wire::decode_attrs(r#"["a/b",1]"#).unwrap_err(),
        ),
        (
            "pull",
            format!(r#","entity":{ENTITY}"#),
            "attrs",
            r#""a/b,c/d""#,
            wire::decode_attrs(r#""a/b,c/d""#).unwrap_err(),
        ),
        (
            "lookup",
            r#","value":{"i64":"1"}"#.into(),
            "attr",
            "5",
            wire::decode_attr("5").unwrap_err(),
        ),
        (
            "lookup",
            r#","value":{"i64":"1"}"#.into(),
            "attr",
            "\"nope\"",
            wire::decode_attr("\"nope\"").unwrap_err(),
        ),
        (
            "lookup",
            r#","attr":"a/b""#.into(),
            "value",
            "1",
            wire::decode_value("1").unwrap_err(),
        ),
        (
            "lookup",
            r#","attr":"a/b""#.into(),
            "value",
            r#"{"ref":{"temp":"a"}}"#,
            wire::decode_value(r#"{"ref":{"temp":"a"}}"#).unwrap_err(),
        ),
        (
            "history",
            String::new(),
            "after",
            "0",
            wire::decode_revision("0").unwrap_err(),
        ),
        (
            "history",
            String::new(),
            "after",
            r#""01""#,
            wire::decode_revision(r#""01""#).unwrap_err(),
        ),
        (
            "transactions",
            String::new(),
            "after",
            "null",
            wire::decode_revision("null").unwrap_err(),
        ),
        (
            "query",
            String::new(),
            "query",
            r#"{"version":2}"#,
            wire::decode_query(r#"{"version":2}"#).unwrap_err(),
        ),
        (
            "query",
            String::new(),
            "query",
            r#"{"version":1,"version":1}"#,
            wire::decode_query(r#"{"version":1,"version":1}"#).unwrap_err(),
        ),
        (
            "transact",
            String::new(),
            "tx",
            r#"{"version":1,"extra":1}"#,
            wire::decode_tx(r#"{"version":1,"extra":1}"#).unwrap_err(),
        ),
        (
            "transact",
            String::new(),
            "tx",
            "null",
            wire::decode_tx("null").unwrap_err(),
        ),
        (
            "schema",
            String::new(),
            "at",
            r#"{"version":1}"#,
            wire::decode_snapshot(r#"{"version":1}"#).unwrap_err(),
        ),
        (
            "schema",
            String::new(),
            "at",
            "3",
            wire::decode_snapshot("3").unwrap_err(),
        ),
        (
            "schema",
            String::new(),
            "at",
            r#""d1.nope""#,
            "d1.nope".parse::<factlite::SnapshotId>().unwrap_err(),
        ),
        (
            "query",
            format!(r#","query":{QUERY}"#),
            "at",
            r#"{"version":1,"version":1}"#,
            wire::decode_snapshot(r#"{"version":1,"version":1}"#).unwrap_err(),
        ),
    ];
    for (method, others, member, text, expected) in cases {
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":"w","method":"{method}","params":{{"handle":"{handle}"{others},"{member}":{text}}}}}"#
        );
        server.send(&line);
        let response = server.response();
        assert_eq!(response["id"], "w", "{line}");
        let error = error_of(&response, -32000);
        assert_eq!(error["data"], library(expected), "{line}");
        assert_eq!(error["message"], error["data"]["message"], "{line}");
    }
}

#[test]
fn a_closed_output_ends_the_server_with_exit_code_5() {
    use std::{io::Write, process::Stdio};
    let mut child = std::process::Command::new(cargo_bin("factlite-rpc"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Nobody will read the answer.
    drop(child.stdout.take());
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"none\"}\n");
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(5));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("factlite-rpc: "), "{stderr}");
}
