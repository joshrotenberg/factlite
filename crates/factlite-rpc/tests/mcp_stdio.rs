//! The MCP front end against the real binary over stdio.

mod common;

use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin;
use common::{Server, parse};
use factlite::{Db, FactFilter, Revision, Store, wire};
use serde_json::{Value as Json, json};

const SCHEMA: &str = r#"{"version":1,"idempotency_key":"schema","schema":[
  {"name":"note/id","type":"string","cardinality":"one","unique":"unique_one"},
  {"name":"note/text","type":"string","cardinality":"one","unique":"none"},
  {"name":"note/rank","type":"i64","cardinality":"one","unique":"none"}]}"#;

fn binary() -> PathBuf {
    cargo_bin("factlite-mcp")
}
fn start(args: &[String]) -> Server {
    Server::start(
        &binary(),
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}
fn bind(flag: &str, name: &str, path: &Path) -> [String; 2] {
    [flag.to_owned(), format!("{name}={}", path.display())]
}
/// A store with the schema and `notes` notes, made through the library.
fn store_with(path: &Path, notes: usize) -> Store {
    let store = Store::open(path).unwrap();
    store.commit(wire::decode_tx(SCHEMA).unwrap()).unwrap();
    for index in 0..notes {
        store.commit(note(index)).unwrap();
    }
    store
}
fn note(index: usize) -> factlite::Tx {
    wire::decode_tx(
        &json!({"version": 1, "idempotency_key": format!("note-{index}"), "new_entities": ["n"],
            "changes": [
                {"op": "assert", "entity": {"temp": "n"}, "attr": "note/id", "value": {"string": format!("n{index:03}")}},
                {"op": "assert", "entity": {"temp": "n"}, "attr": "note/text", "value": {"string": format!("text {index}")}},
                {"op": "assert", "entity": {"temp": "n"}, "attr": "note/rank", "value": {"i64": (index % 7).to_string()}},
            ]})
        .to_string(),
    )
    .unwrap()
}

/// Opens the handshake and returns its result.
fn initialize(server: &mut Server, version: &str) -> Json {
    let result = server.ok(
        "initialize",
        json!({"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
    );
    server.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    result
}
/// Calls a tool that must succeed and returns its parsed text.
fn tool(server: &mut Server, name: &str, arguments: Json) -> Json {
    let result = server.ok(
        "tools/call",
        json!({"name": name, "arguments": arguments.clone()}),
    );
    assert_eq!(result["isError"], false, "{name} {arguments}: {result}");
    text_of(&result)
}
/// Calls a tool that must fail and returns its Factlite error object.
fn tool_error(server: &mut Server, name: &str, arguments: Json, code: &str) -> Json {
    let result = server.ok("tools/call", json!({"name": name, "arguments": arguments}));
    assert_eq!(result["isError"], true, "{name}: {result}");
    let mut text = text_of(&result);
    assert_eq!(text["error"]["code"], code, "{name}: {text}");
    text["error"].take()
}
fn text_of(result: &Json) -> Json {
    let content = result["content"].as_array().expect("content is an array");
    assert_eq!(content.len(), 1, "{result}");
    assert_eq!(content[0]["type"], "text");
    assert!(result.get("structuredContent").is_none());
    parse(content[0]["text"].as_str().unwrap())
}
fn meta() -> Json {
    json!({"io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}})
}

#[test]
fn every_handshake_revision_is_answered_and_the_rest_fall_back_to_the_newest() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notes.sqlite");
    store_with(&path, 1);
    for (requested, answered) in [
        ("2024-11-05", "2024-11-05"),
        ("2025-03-26", "2025-03-26"),
        ("2025-06-18", "2025-06-18"),
        ("2025-11-25", "2025-11-25"),
        ("2026-07-28", "2025-11-25"),
        ("1999-01-01", "2025-11-25"),
    ] {
        let mut server = start(&bind("--store", "notes", &path));
        let result = initialize(&mut server, requested);
        assert_eq!(result["protocolVersion"], answered, "{requested}");
        assert_eq!(
            result["capabilities"],
            json!({"tools": {}, "resources": {}})
        );
        assert_eq!(result["serverInfo"]["name"], "factlite-mcp");
        assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        let instructions = result["instructions"].as_str().unwrap();
        assert!(instructions.contains("notes (read-only)"), "{instructions}");
        assert!(!instructions.contains("idempotency_key"), "{instructions}");
        assert!(result.get("resultType").is_none() && result.get("_meta").is_none());
        // The notification above got no answer: the next line answers ping.
        assert_eq!(server.ok("ping", json!({})), json!({}));
        let (status, rest, stderr) = server.finish();
        assert!(status.success() && rest.is_empty(), "{rest:?}");
        assert_eq!(stderr, "");
    }
}

#[test]
fn the_stateless_revision_is_served_per_request() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notes.sqlite");
    store_with(&path, 1);
    let mut server = start(&bind("--writable-store", "notes", &path));

    let discovered = server.ok("server/discover", json!({"_meta": meta()}));
    assert_eq!(discovered["resultType"], "complete");
    assert_eq!(
        discovered["supportedVersions"],
        json!([
            "2026-07-28",
            "2025-11-25",
            "2025-06-18",
            "2025-03-26",
            "2024-11-05"
        ])
    );
    assert_eq!(
        discovered["capabilities"],
        json!({"tools": {}, "resources": {}})
    );
    assert_eq!(
        discovered["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "factlite-mcp"
    );
    assert!(
        discovered["instructions"]
            .as_str()
            .unwrap()
            .contains("notes (writable)")
    );
    assert_eq!(discovered["cacheScope"], "private");
    assert!(discovered["ttlMs"].as_u64().is_some());

    // No initialize came first, and none is needed.
    let listed = server.ok("tools/list", json!({"_meta": meta()}));
    assert_eq!(listed["resultType"], "complete");
    assert_eq!(listed["cacheScope"], "private");
    assert_eq!(listed["tools"].as_array().unwrap().len(), 9);
    let called = server.ok(
        "tools/call",
        json!({"_meta": meta(), "name": "head", "arguments": {}}),
    );
    assert_eq!(called["resultType"], "complete");
    assert!(
        called.get("ttlMs").is_none(),
        "a tool call carries no cache hint"
    );
    assert_eq!(
        called["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "factlite-mcp"
    );
    let read = server.ok(
        "resources/read",
        json!({"_meta": meta(), "uri": "factlite://stores/notes/schema"}),
    );
    assert_eq!(read["ttlMs"], 0);

    // The same methods without _meta are answered in the handshake form.
    let plain = server.ok("tools/list", json!({}));
    assert!(plain.get("resultType").is_none() && plain.get("ttlMs").is_none());
    assert_eq!(plain["tools"], listed["tools"]);

    // A version this server does not know.
    let mut old = meta();
    old["io.modelcontextprotocol/protocolVersion"] = json!("2030-01-01");
    let error = server.err("tools/list", json!({"_meta": old}));
    assert_eq!(error["code"], -32022);
    assert_eq!(error["message"], "Unsupported protocol version");
    assert_eq!(error["data"]["requested"], "2030-01-01");
    assert_eq!(error["data"]["supported"][0], "2026-07-28");
    // The stateless revision requires the client's capabilities.
    let error = server.err(
        "tools/list",
        json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}}),
    );
    assert_eq!(error["code"], -32602);
    // discover belongs to the stateless revision.
    assert_eq!(server.err("server/discover", json!({}))["code"], -32602);
    // A request naming an older revision in _meta is answered in its form.
    let mut older = meta();
    older["io.modelcontextprotocol/protocolVersion"] = json!("2025-06-18");
    let result = server.ok("tools/list", json!({"_meta": older}));
    assert!(result.get("resultType").is_none());
}

#[test]
fn protocol_errors_use_the_standard_codes_and_notifications_are_never_answered() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notes.sqlite");
    store_with(&path, 1);
    let mut server = start(&bind("--store", "notes", &path));
    initialize(&mut server, "2025-11-25");

    for (line, code) in [
        ("not json", -32700),
        ("[]", -32600),
        (r#"{"jsonrpc":"2.0","id":1.5,"method":"ping"}"#, -32600),
        (r#"{"jsonrpc":"2.0","id":1e3,"method":"ping"}"#, -32600),
        (r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#, -32600),
        (r#"{"jsonrpc":"2.0","id":true,"method":"ping"}"#, -32600),
        (r#"{"jsonrpc":"2.0","id":1,"id":2,"method":"ping"}"#, -32600),
        (r#"{"id":1,"method":"ping"}"#, -32600),
    ] {
        server.send(line);
        let response = server.response();
        assert_eq!(response["error"]["code"], code, "{line}");
        assert_eq!(response["id"], Json::Null, "{line}");
    }
    for notification in [
        r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/progress","params":{}}"#,
        r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"head","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","method":"anything"}"#,
    ] {
        server.send(notification);
    }
    assert_eq!(server.err("resources/subscribe", json!({}))["code"], -32601);
    assert_eq!(server.err("prompts/list", json!({}))["code"], -32601);
    assert_eq!(server.err("tools/list", json!([]))["code"], -32602);
    // Unknown members of MCP params are ignored; the protocol adds them.
    server.ok(
        "tools/list",
        json!({"future": 1, "_meta": {"progressToken": 3}}),
    );
    assert_eq!(
        server.err("tools/call", json!({"arguments": {}}))["code"],
        -32602
    );
    let unknown = server.err("tools/call", json!({"name": "drop_table", "arguments": {}}));
    assert_eq!(unknown["code"], -32602);
    assert_eq!(unknown["message"], "Unknown tool: drop_table");
    // transact does not exist on a server with no writable store.
    assert_eq!(
        server.err("tools/call", json!({"name": "transact", "arguments": {}}))["code"],
        -32602
    );
    assert_eq!(
        server.err("tools/call", json!({"name": "head", "arguments": []}))["code"],
        -32602
    );
    // A tool with no arguments member gets none.
    tool(&mut server, "head", json!({}));
    let call = server.ok("tools/call", json!({"name": "head"}));
    assert_eq!(call["isError"], false);
    let (status, rest, _) = server.finish();
    assert!(status.success() && rest.is_empty(), "{rest:?}");
}

#[test]
fn tools_return_the_library_output_for_the_same_read() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notes.sqlite");
    let store = store_with(&path, 5);
    let mut server = start(&bind("--store", "notes", &path));
    initialize(&mut server, "2025-11-25");

    let view = Db::open_read_only(&path).unwrap();
    let basis = parse(&wire::encode_snapshot(view.snapshot()));
    let entity = view
        .lookup(
            &"note/id".parse().unwrap(),
            &factlite::Value::String("n002".into()),
        )
        .unwrap()
        .unwrap();
    let rank = FactFilter {
        attribute: Some("note/rank".parse().unwrap()),
        ..FactFilter::default()
    };
    let query = json!({"version": 1, "query": {
        "match": [[{"var": "n"}, "note/rank", {"var": "r"}], [{"var": "n"}, "note/id", {"var": "id"}]],
        "where": [{"left": {"var": "r"}, "op": "ge", "right": {"i64": "2"}}],
        "select": ["id", "r"], "order": [{"field": "id", "direction": "asc"}]}, "params": {}});
    let (decoded, params) = wire::decode_query(&query.to_string()).unwrap();

    let head = tool(&mut server, "head", json!({"store": "notes"}));
    assert_eq!(head["snapshot"], basis);
    let now: i64 = head["now"]["inst"].as_str().unwrap().parse().unwrap();
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!(
        (wall - now).abs() < 60_000,
        "now is the current time: {now} {wall}"
    );

    let paged: Vec<(&str, Json, String, &str)> = vec![
        (
            "schema",
            json!({}),
            wire::encode_schema(&view.schema().unwrap()),
            "schema",
        ),
        (
            "facts",
            json!({"filter": {"attr": "note/rank"}}),
            wire::encode_facts(&view.facts(rank.clone()).unwrap()),
            "facts",
        ),
        (
            "history",
            json!({"filter": {"attr": "note/rank"}, "after": "2"}),
            wire::encode_history(&view.history(rank.clone(), Revision::new(2)).unwrap()),
            "events",
        ),
        (
            "transactions",
            json!({"after": "3"}),
            wire::encode_transactions(&view.transactions(Revision::new(3)).unwrap()),
            "transactions",
        ),
        (
            "query",
            json!({"query": query}),
            wire::encode_query_result(&view.query(&decoded, &params).unwrap()),
            "rows",
        ),
    ];
    for (name, arguments, expected, items) in paged {
        let result = tool(&mut server, name, arguments.clone());
        assert_eq!(result["basis"], basis, "{name}");
        assert_eq!(result["value"], parse(&expected), "{name}");
        let total = result["value"][items].as_array().unwrap().len();
        assert_eq!(
            result["page"],
            json!({"offset": 0, "count": total, "total": total}),
            "{name}"
        );
    }
    let pulled = tool(
        &mut server,
        "pull",
        json!({"entity": {"id": entity.to_string()}, "attrs": ["note/text", "note/rank"]}),
    );
    assert_eq!(
        pulled,
        json!({"basis": basis, "value": parse(&wire::encode_entity(
            view.pull(&entity, &["note/text".parse().unwrap(), "note/rank".parse().unwrap()]).unwrap().as_ref()
        ))})
    );
    let looked = tool(
        &mut server,
        "lookup",
        json!({"attr": "note/id", "value": {"string": "n002"}}),
    );
    assert_eq!(looked["value"]["entity"]["id"], entity.to_string());

    // A later commit, and a read at the earlier basis, in both spellings.
    store.commit(note(9)).unwrap();
    let token = view.snapshot().to_string();
    for at in [basis.clone(), json!(token)] {
        let then = tool(
            &mut server,
            "facts",
            json!({"filter": {"attr": "note/rank"}, "at": at}),
        );
        assert_eq!(then["value"]["facts"].as_array().unwrap().len(), 5);
    }
    let now = tool(
        &mut server,
        "facts",
        json!({"filter": {"attr": "note/rank"}}),
    );
    assert_eq!(now["value"]["facts"].as_array().unwrap().len(), 6);

    // Errors are results the model can read, with the library's object.
    let unknown = tool_error(
        &mut server,
        "facts",
        json!({"filter": {"attr": "note/none"}}),
        "UnknownAttribute",
    );
    let library = view
        .facts(FactFilter {
            attribute: Some("note/none".parse().unwrap()),
            ..FactFilter::default()
        })
        .unwrap_err();
    assert_eq!(unknown, parse(&wire::encode_error(&library))["error"]);
    for (name, arguments, field) in [
        ("facts", json!({"filter": {}, "handle": "h1"}), "arguments"),
        ("facts", json!({"filters": {}}), "arguments"),
        ("facts", json!({"store": "other"}), "store"),
        ("facts", json!({"store": 1}), "arguments"),
        ("facts", json!({"limit": 0}), "limit"),
        ("facts", json!({"limit": -1}), "arguments"),
        ("facts", json!({"offset": 2}), "offset"),
        (
            "pull",
            json!({"entity": {"id": entity.to_string()}}),
            "arguments",
        ),
        ("head", json!({"at": basis}), "arguments"),
        ("head", json!({"offset": 0}), "arguments"),
    ] {
        let error = tool_error(&mut server, name, arguments.clone(), "InvalidRequest");
        assert_eq!(
            error["context"]["field"], field,
            "{name} {arguments}: {error}"
        );
    }
    // A repeated argument is also the model's to read.
    server.send(
        r#"{"jsonrpc":"2.0","id":"dup","method":"tools/call","params":{"name":"facts","arguments":{"store":"notes","store":"notes"}}}"#,
    );
    let response = server.response();
    assert_eq!(response["id"], "dup");
    assert_eq!(response["result"]["isError"], true);
    tool_error(
        &mut server,
        "lookup",
        json!({"attr": "note/id", "value": 3}),
        "InvalidRequest",
    );
    tool_error(
        &mut server,
        "facts",
        json!({"at": "d1.nope"}),
        "InvalidSnapshot",
    );
}

#[test]
fn pages_of_one_read_tile_its_result_at_the_first_basis_while_others_commit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notes.sqlite");
    let store = store_with(&path, 23);
    let mut args: Vec<String> = bind("--store", "notes", &path).into();
    args.extend(["--page-items".into(), "4".into()]);
    let mut server = start(&args);
    initialize(&mut server, "2025-11-25");

    let query = json!({"version": 1, "query": {
        "match": [[{"var": "n"}, "note/id", {"var": "id"}], [{"var": "n"}, "note/rank", {"var": "r"}]],
        "select": ["id", "r"], "order": [{"field": "r", "direction": "desc"}, {"field": "id", "direction": "asc"}]},
        "params": {}});
    let (decoded, params) = wire::decode_query(&query.to_string()).unwrap();
    let ids = FactFilter {
        attribute: Some("note/id".parse().unwrap()),
        ..FactFilter::default()
    };
    type Whole = Box<dyn Fn(&Db) -> Json>;
    let cases: Vec<(&str, Json, &str, Whole, Option<u64>)> = vec![
        (
            "schema",
            json!({}),
            "schema",
            Box::new(|db: &Db| {
                parse(&wire::encode_schema(&db.schema().unwrap()))["schema"].clone()
            }),
            Some(1),
        ),
        (
            "facts",
            json!({"filter": {"attr": "note/id"}}),
            "facts",
            Box::new(move |db: &Db| {
                parse(&wire::encode_facts(&db.facts(ids.clone()).unwrap()))["facts"].clone()
            }),
            None,
        ),
        (
            "history",
            json!({}),
            "events",
            Box::new(|db: &Db| {
                parse(&wire::encode_history(
                    &db.history(FactFilter::default(), Revision::ZERO).unwrap(),
                ))["events"]
                    .clone()
            }),
            Some(3),
        ),
        (
            "transactions",
            json!({"after": "5"}),
            "transactions",
            Box::new(|db: &Db| {
                parse(&wire::encode_transactions(
                    &db.transactions(Revision::new(5)).unwrap(),
                ))["transactions"]
                    .clone()
            }),
            Some(10),
        ),
        (
            "query",
            json!({"query": query}),
            "rows",
            Box::new(move |db: &Db| {
                parse(&wire::encode_query_result(
                    &db.query(&decoded, &params).unwrap(),
                ))["rows"]
                    .clone()
            }),
            None,
        ),
    ];
    let mut commits = 100;
    for (name, mut arguments, items, whole, limit) in cases {
        if let Some(limit) = limit {
            arguments["limit"] = json!(limit);
        }
        let size = limit.unwrap_or(4).min(4) as usize;
        // The whole result, read through the library at the first page's
        // basis once it is known.
        let mut expected = Json::Null;
        let mut total = 0;
        let mut seen = Vec::new();
        let mut basis = Json::Null;
        let mut offset = 0;
        loop {
            let result = tool(&mut server, name, arguments.clone());
            if basis.is_null() {
                basis = result["basis"].clone();
                let view = store
                    .db_at(&wire::decode_snapshot(&basis.to_string()).unwrap())
                    .unwrap();
                expected = whole(&view);
                total = expected.as_array().unwrap().len();
                assert!(total > 2 * size, "{name} needs several pages");
            }
            assert_eq!(result["basis"], basis, "{name}");
            let page = &result["page"];
            let count = page["count"].as_u64().unwrap() as usize;
            assert_eq!(page["offset"], offset);
            assert_eq!(page["total"], total);
            assert_eq!(count, size.min(total - offset), "{name}");
            seen.extend(result["value"][items].as_array().unwrap().iter().cloned());
            // Another writer moves the head between every two pages.
            store.commit(note(commits)).unwrap();
            commits += 1;
            match page.get("next") {
                Some(next) => {
                    offset = next.as_u64().unwrap() as usize;
                    assert_eq!(offset, seen.len());
                    arguments["at"] = basis.clone();
                    arguments["offset"] = json!(offset);
                }
                None => break,
            }
        }
        assert_eq!(Json::Array(seen), expected, "{name}");
        // Past the end is an empty last page.
        arguments["offset"] = json!(total + 5);
        let past = tool(&mut server, name, arguments);
        assert_eq!(
            past["page"],
            json!({"offset": total, "count": 0, "total": total})
        );
    }
}

#[test]
fn a_page_is_cut_by_bytes_but_always_holds_one_item() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notes.sqlite");
    store_with(&path, 12);
    let mut args: Vec<String> = bind("--store", "notes", &path).into();
    args.extend(["--page-bytes".into(), "300".into()]);
    let mut server = start(&args);
    initialize(&mut server, "2025-11-25");

    let mut arguments = json!({"filter": {"attr": "note/text"}});
    let mut pages: Vec<Vec<usize>> = Vec::new();
    loop {
        let result = tool(&mut server, "facts", arguments.clone());
        let items = result["value"]["facts"].as_array().unwrap();
        pages.push(items.iter().map(|item| item.to_string().len()).collect());
        let Some(next) = result["page"].get("next") else {
            break;
        };
        arguments["at"] = result["basis"].clone();
        arguments["offset"] = next.clone();
    }
    assert_eq!(pages.iter().map(Vec::len).sum::<usize>(), 12);
    // Each page holds as many items as fit in 300 bytes, and at least one:
    // a page under the bound would have had room for the next page's first
    // item otherwise.
    for (index, page) in pages.iter().enumerate() {
        let bytes: usize = page.iter().sum();
        assert!(!page.is_empty());
        assert!(page.len() == 1 || bytes <= 300, "{bytes} bytes in {page:?}");
        if let Some(next) = pages.get(index + 1) {
            assert!(
                bytes + next[0] > 300,
                "page {index} had room: {page:?} then {next:?}"
            );
        }
    }
    assert!(
        pages.iter().any(|page| page.len() > 1),
        "the bound allows several items"
    );

    let mut args: Vec<String> = bind("--store", "notes", &path).into();
    args.extend(["--page-bytes".into(), "0".into()]);
    let mut tiny = start(&args);
    let result = tool(&mut tiny, "facts", json!({"filter": {"attr": "note/text"}}));
    assert_eq!(
        result["page"]["count"], 1,
        "one item even when it alone is over"
    );
}

#[test]
fn a_read_only_binding_cannot_write_or_create_a_file() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.sqlite");
    for flags in [&["--store"][..], &["--writable-store"]] {
        let mut args: Vec<String> = bind(flags[0], "notes", &missing).into();
        if flags[0] == "--store" {
            args.push("--validation".into());
            args.push("structural".into());
        }
        let server = start(&args);
        let (status, rest, stderr) = server.finish();
        assert_eq!(status.code(), Some(2), "{flags:?}");
        assert!(rest.is_empty());
        assert!(
            stderr.starts_with("factlite-mcp: cannot open store notes at "),
            "{stderr}"
        );
        assert!(!missing.exists(), "{flags:?} created a file");
    }
    let mut created: Vec<String> = bind("--writable-store", "notes", &missing).into();
    created.push("--create".into());
    let mut server = start(&created);
    tool(&mut server, "head", json!({}));
    drop(server);
    assert!(missing.exists());

    // One read-only and one writable store in one server.
    let path = directory.path().join("notes.sqlite");
    store_with(&path, 1);
    let mut args: Vec<String> = bind("--store", "archive", &path).into();
    args.extend(bind("--writable-store", "work", &missing));
    let mut server = start(&args);
    initialize(&mut server, "2025-11-25");
    let tools = server.ok("tools/list", json!({}))["tools"].clone();
    let transact = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "transact")
        .unwrap();
    assert_eq!(
        transact["inputSchema"]["properties"]["store"]["enum"],
        json!(["work"])
    );
    assert_eq!(transact["inputSchema"]["required"], json!(["tx", "store"]));
    let refused = tool_error(
        &mut server,
        "transact",
        json!({"store": "archive", "tx": {"version": 1, "idempotency_key": "k"}}),
        "InvalidRequest",
    );
    assert_eq!(refused["message"], "store archive is bound read-only");
    let unnamed = tool_error(&mut server, "head", json!({}), "InvalidRequest");
    assert_eq!(unnamed["message"], "name a store: archive, work");
    assert_eq!(
        Store::open(&path).unwrap().head().unwrap().revision().get(),
        2,
        "the read-only store is unchanged"
    );
    let report = tool(
        &mut server,
        "transact",
        json!({"store": "work", "tx": {"version": 1, "idempotency_key": "k"}}),
    );
    assert_eq!(report["after"]["revision"], "1");
}

#[test]
fn writes_need_a_key_carry_the_server_metadata_and_replay_exactly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notes.sqlite");
    store_with(&path, 0);
    let mut args: Vec<String> = bind("--writable-store", "notes", &path).into();
    args.extend([
        "--metadata".into(),
        "agent=writer-1".into(),
        "--metadata".into(),
        "session=s-9".into(),
    ]);
    let mut server = start(&args);
    let init = initialize(&mut server, "2025-06-18");
    let instructions = init["instructions"].as_str().unwrap();
    for phrase in [
        "idempotency_key",
        "Never write secrets",
        "expect",
        "UniqueViolation",
        "now",
    ] {
        assert!(instructions.contains(phrase), "{phrase}");
    }

    let unkeyed = tool_error(
        &mut server,
        "transact",
        json!({"tx": {"version": 1}}),
        "InvalidRequest",
    );
    assert_eq!(unkeyed["context"]["field"], "idempotency_key");
    let forged = tool_error(
        &mut server,
        "transact",
        json!({"tx": {"version": 1, "idempotency_key": "f", "metadata": {"agent": {"string": "someone-else"}}}}),
        "InvalidRequest",
    );
    assert_eq!(forged["context"]["field"], "metadata");
    // A request that does not decode is refused by the wire, not reshaped.
    let wire = tool_error(
        &mut server,
        "transact",
        json!({"tx": {"version": 1, "idempotency_key": "w", "metadata": []}}),
        "InvalidRequest",
    );
    assert_eq!(wire["context"]["field"], "metadata");
    tool_error(
        &mut server,
        "transact",
        json!({"tx": "text"}),
        "InvalidRequest",
    );

    let request = json!({"tx": {"version": 1, "idempotency_key": "add n1", "new_entities": ["n"],
        "metadata": {"step": {"i64": "4"}},
        "changes": [{"op": "assert", "entity": {"temp": "n"}, "attr": "note/id", "value": {"string": "n1"}}]}});
    let report = tool(&mut server, "transact", request.clone());
    assert_eq!(
        report["metadata"],
        json!({"agent": {"string": "writer-1"}, "session": {"string": "s-9"}, "step": {"i64": "4"}})
    );
    assert_eq!(tool(&mut server, "transact", request.clone()), report);
    let mut changed = request.clone();
    changed["tx"]["metadata"]["step"] = json!({"i64": "5"});
    tool_error(&mut server, "transact", changed, "IdempotencyConflict");

    // The log holds what the model was told.
    let log = Db::open_read_only(&path)
        .unwrap()
        .transactions(Revision::new(1))
        .unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(parse(&wire::encode_report(&log[0].report)), report);

    // A server started with other metadata cannot replay the request.
    drop(server);
    let mut other = start(&bind("--writable-store", "notes", &path));
    tool_error(&mut other, "transact", request, "IdempotencyConflict");
}

#[test]
fn each_store_schema_is_a_resource() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("a.sqlite");
    let second = directory.path().join("b.sqlite");
    store_with(&first, 1);
    Store::open(&second).unwrap();
    let mut args: Vec<String> = bind("--store", "alpha", &first).into();
    args.extend(bind("--store", "beta", &second));
    let mut server = start(&args);
    initialize(&mut server, "2025-11-25");

    let listed = server.ok("resources/list", json!({}));
    let uris: Vec<&str> = listed["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|resource| resource["uri"].as_str().unwrap())
        .collect();
    assert_eq!(
        uris,
        [
            "factlite://stores/alpha/schema",
            "factlite://stores/beta/schema"
        ]
    );
    assert_eq!(
        server.ok("resources/templates/list", json!({})),
        json!({"resourceTemplates": []})
    );

    let read = server.ok(
        "resources/read",
        json!({"uri": "factlite://stores/alpha/schema"}),
    );
    let contents = read["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["uri"], "factlite://stores/alpha/schema");
    assert_eq!(contents[0]["mimeType"], "application/json");
    let schema = parse(contents[0]["text"].as_str().unwrap());
    let view = Db::open_read_only(&first).unwrap();
    assert_eq!(
        schema["value"],
        parse(&wire::encode_schema(&view.schema().unwrap()))
    );
    let empty = server.ok(
        "resources/read",
        json!({"uri": "factlite://stores/beta/schema"}),
    );
    assert_eq!(
        parse(empty["contents"][0]["text"].as_str().unwrap())["value"]["schema"],
        json!([])
    );

    let missing = server.err(
        "resources/read",
        json!({"uri": "factlite://stores/gamma/schema"}),
    );
    assert_eq!(missing["code"], -32002);
    assert_eq!(
        missing["data"],
        json!({"uri": "factlite://stores/gamma/schema"})
    );
    let missing = server.err(
        "resources/read",
        json!({"_meta": meta(), "uri": "factlite://stores/alpha/facts"}),
    );
    assert_eq!(missing["code"], -32602);
    assert_eq!(server.err("resources/read", json!({}))["code"], -32602);
}

#[test]
fn start_up_refuses_bad_arguments_before_reading_anything() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notes.sqlite");
    store_with(&path, 0);
    let good = format!("notes={}", path.display());
    let run = |args: &[&str]| {
        let mut command = assert_cmd::Command::new(binary());
        command
            .args(args)
            .timeout(std::time::Duration::from_secs(20));
        command
    };
    let help = run(&["--help"]).assert().success().stderr("");
    let help = String::from_utf8(help.get_output().stdout.clone()).unwrap();
    for word in [
        "--store",
        "--writable-store",
        "--create",
        "--metadata",
        "--page-items",
        "Exit codes",
    ] {
        assert!(help.contains(word), "{word}");
    }
    run(&["--version"])
        .assert()
        .success()
        .stdout(format!("factlite-mcp {}\n", env!("CARGO_PKG_VERSION")));
    for args in [
        &[][..],
        &["--store"],
        &["--store", "notes"],
        &["--store", "=x.sqlite"],
        &["--store", "notes="],
        &["--store", "no tes=x.sqlite"],
        &["--store", "notes/x=x.sqlite"],
        &["--store", &good, "--store", &good],
        &["--store", &good, "--create"],
        &["--store", &good, "--validation", "fast"],
        &["--store", &good, "--page-items", "0"],
        &["--store", &good, "--page-items", "-1"],
        &["--store", &good, "--busy-timeout-ms", "x"],
        &["--store", &good, "--metadata", "noequals"],
        &["--store", &good, "--metadata", "=v"],
        &["--store", &good, "--metadata", "a=1", "--metadata", "a=2"],
        &["--store", &good, "--page-bytes", "1", "--page-bytes", "2"],
        &["--store", &good, "--unknown"],
    ] {
        let failed = run(args).write_stdin("").assert().code(2).stdout("");
        let stderr = String::from_utf8(failed.get_output().stderr.clone()).unwrap();
        assert!(stderr.starts_with("factlite-mcp: "), "{args:?}: {stderr}");
        assert_eq!(stderr.lines().count(), 1, "{args:?}: {stderr}");
    }
    run(&["--store", &good])
        .write_stdin("")
        .assert()
        .success()
        .stdout("")
        .stderr("");
}

/// Checks a value against the subset of JSON Schema the tool definitions use.
/// Returns the path of the first violation.
fn violation(schema: &Json, value: &Json, at: &str) -> Option<String> {
    let fail = || Some(at.to_owned());
    if schema == &json!(false) {
        return fail();
    }
    if let Some(types) = schema.get("type") {
        let kinds: Vec<&str> = match types {
            Json::String(kind) => vec![kind.as_str()],
            Json::Array(kinds) => kinds.iter().map(|kind| kind.as_str().unwrap()).collect(),
            _ => panic!("type at {at}"),
        };
        let matches = kinds.iter().any(|kind| match *kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            // JSON Schema counts 1.0 as an integer.
            "integer" => value.as_f64().is_some_and(|number| number.fract() == 0.0),
            "boolean" => value.is_boolean(),
            other => panic!("type {other} at {at}"),
        });
        if !matches {
            return fail();
        }
    }
    if let Some(expected) = schema.get("const")
        && value != expected
    {
        return fail();
    }
    if let Some(options) = schema.get("enum")
        && !options.as_array().unwrap().contains(value)
    {
        return fail();
    }
    if let Some(inner) = schema.get("not")
        && violation(inner, value, at).is_none()
    {
        return fail();
    }
    if let Some(options) = schema.get("oneOf") {
        let matching = options
            .as_array()
            .unwrap()
            .iter()
            .filter(|option| violation(option, value, at).is_none())
            .count();
        if matching != 1 {
            return fail();
        }
    }
    if let Some(minimum) = schema.get("minimum")
        && value
            .as_f64()
            .is_some_and(|number| number < minimum.as_f64().unwrap())
    {
        return fail();
    }
    if let Some(text) = value.as_str() {
        if schema
            .get("minLength")
            .is_some_and(|min| (text.chars().count() as u64) < min.as_u64().unwrap())
        {
            return fail();
        }
        // The two patterns the schemas use, checked without a regex engine.
        let matches = match schema.get("pattern").and_then(Json::as_str) {
            None => true,
            Some("^(0|[1-9][0-9]*)$") => text.parse::<Revision>().is_ok(),
            Some("^[+-]?[0-9]+$") => {
                let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
                !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
            }
            Some(other) => panic!("pattern {other} at {at}"),
        };
        if !matches {
            return fail();
        }
    }
    if let Json::Array(items) = value {
        let count = items.len() as u64;
        if schema
            .get("minItems")
            .is_some_and(|min| count < min.as_u64().unwrap())
            || schema
                .get("maxItems")
                .is_some_and(|max| count > max.as_u64().unwrap())
        {
            return fail();
        }
        let prefix = schema.get("prefixItems").and_then(Json::as_array);
        for (index, element) in items.iter().enumerate() {
            let item = prefix
                .and_then(|prefix| prefix.get(index))
                .or_else(|| schema.get("items"));
            if let Some(item) = item
                && let Some(path) = violation(item, element, &format!("{at}[{index}]"))
            {
                return Some(path);
            }
        }
    }
    if let Json::Object(members) = value {
        let count = members.len() as u64;
        if schema
            .get("minProperties")
            .is_some_and(|min| count < min.as_u64().unwrap())
            || schema
                .get("maxProperties")
                .is_some_and(|max| count > max.as_u64().unwrap())
        {
            return fail();
        }
        let properties = schema.get("properties").and_then(Json::as_object);
        for required in schema
            .get("required")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            if !members.contains_key(required.as_str().unwrap()) {
                return Some(format!("{at}.{}", required.as_str().unwrap()));
            }
        }
        if let Some(dependent) = schema.get("dependentRequired").and_then(Json::as_object) {
            for (key, needs) in dependent {
                if members.contains_key(key) {
                    for need in needs.as_array().unwrap() {
                        if !members.contains_key(need.as_str().unwrap()) {
                            return Some(format!("{at}.{}", need.as_str().unwrap()));
                        }
                    }
                }
            }
        }
        for (key, member) in members {
            if let Some(names) = schema.get("propertyNames")
                && violation(names, &json!(key), &format!("{at}.{key}")).is_some()
            {
                return Some(format!("{at}.{key}"));
            }
            let rule = properties
                .and_then(|properties| properties.get(key))
                .or_else(|| schema.get("additionalProperties"));
            if let Some(rule) = rule
                && let Some(path) = violation(rule, member, &format!("{at}.{key}"))
            {
                return Some(path);
            }
        }
    }
    None
}

#[test]
fn published_input_schemas_accept_what_the_tools_accept_and_name_every_argument() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notes.sqlite");
    store_with(&path, 3);
    let mut args: Vec<String> = bind("--writable-store", "notes", &path).into();
    args.extend(bind("--store", "copy", &path));
    args.extend(["--metadata".into(), "agent=tester".into()]);
    let mut server = start(&args);
    initialize(&mut server, "2025-11-25");
    let tools: Vec<Json> = server.ok("tools/list", json!({}))["tools"]
        .as_array()
        .unwrap()
        .clone();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "head",
            "schema",
            "facts",
            "pull",
            "lookup",
            "history",
            "transactions",
            "query",
            "transact"
        ]
    );
    for tool in &tools {
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert!(tool["description"].as_str().unwrap().len() > 20);
        let writes = tool["name"] == "transact";
        assert_eq!(
            tool["annotations"],
            json!({"readOnlyHint": !writes, "destructiveHint": writes, "idempotentHint": true, "openWorldHint": false})
        );
    }
    let schema_of =
        |name: &str| tools.iter().find(|tool| tool["name"] == name).unwrap()["inputSchema"].clone();

    let entity = Db::open_read_only(&path)
        .unwrap()
        .lookup(
            &"note/id".parse().unwrap(),
            &factlite::Value::String("n001".into()),
        )
        .unwrap()
        .unwrap()
        .to_string();
    let head = tool(&mut server, "head", json!({"store": "copy"}))["snapshot"].clone();
    // Arguments the tools accept, which each schema must accept too.
    let accepted = [
        ("head", json!({"store": "notes"})),
        ("schema", json!({"store": "copy", "at": head})),
        (
            "facts",
            json!({"store": "notes", "filter": {"entity": {"id": entity}, "attr": "note/rank", "value": {"i64": "1"}}, "limit": 2}),
        ),
        (
            "facts",
            json!({"store": "notes", "at": head.clone(), "offset": 1}),
        ),
        (
            "pull",
            json!({"store": "notes", "entity": {"id": entity}, "attrs": []}),
        ),
        (
            "lookup",
            json!({"store": "notes", "attr": "note/id", "value": {"string": "n001"}}),
        ),
        (
            "history",
            json!({"store": "notes", "filter": {}, "after": "0"}),
        ),
        (
            "transactions",
            json!({"store": "notes", "after": "2", "at": wire::decode_snapshot(&head.to_string()).unwrap().to_string()}),
        ),
        (
            "query",
            json!({"store": "notes", "query": {"version": 1, "query": {
            "inputs": {"min": "i64"},
            "match": [[{"var": "n"}, "note/rank", {"var": "r"}]],
            "where": [{"left": {"var": "r"}, "op": "ge", "right": {"param": "min"}}],
            "not": [{"match": [[{"var": "n"}, "note/id", {"string": "n000"}]], "where": []}],
            "select": [], "with": ["n"], "order": [{"field": "total", "direction": "asc"}], "limit": 5,
            "aggregates": [{"fn": "count", "var": "n", "name": "total"}],
            "having": [{"left": {"var": "total"}, "op": "ge", "right": {"i64": "0"}}]},
            "params": {"min": {"i64": "1"}}}}),
        ),
        (
            "transact",
            json!({"store": "notes", "tx": {"version": 1, "idempotency_key": "full",
            "schema": [{"name": "note/tag", "type": "keyword", "cardinality": "many", "unique": "none"}],
            "new_entities": ["n"],
            "changes": [
                {"op": "assert", "entity": {"temp": "n"}, "attr": "note/id", "value": {"string": "n100"}},
                {"op": "assert", "entity": {"lookup": {"attr": "note/id", "value": {"string": "n001"}}}, "attr": "note/rank", "value": {"i64": "6"}},
                {"op": "retract", "entity": {"id": entity}, "attr": "note/text", "value": {"string": "text 1"}}],
            "expect": [{"entity": {"id": entity}, "attr": "note/rank", "values": [{"i64": "1"}]}],
            "metadata": {"why": {"string": "test"}}}}),
        ),
    ];
    for (name, arguments) in accepted {
        assert_eq!(
            violation(&schema_of(name), &arguments, name),
            None,
            "{name} {arguments}"
        );
        tool(&mut server, name, arguments);
    }
    // Arguments every schema refuses and the tools refuse too.
    let refused = [
        (
            "transact",
            json!({"store": "notes", "tx": {"version": 1, "idempotency_key": "k", "metadata": {"agent": {"string": "x"}}}}),
        ),
        ("schema", json!({"store": "notes", "offset": 1})),
        (
            "lookup",
            json!({"store": "notes", "attr": "note/id", "value": {"string": 1}}),
        ),
        (
            "lookup",
            json!({"store": "notes", "attr": "note/id", "value": {"text": "a"}}),
        ),
        (
            "query",
            json!({"store": "notes", "query": {"version": 1, "query": {"match": [[0, 0, 0]], "select": []}}}),
        ),
        (
            "query",
            json!({"store": "notes", "query": {"version": 1, "query": {"match": [[{"var": "n"}, "note/id", {"var": "v"}]], "select": [], "where": [{"left": {"var": "v"}, "op": "like", "right": {"var": "v"}}]}}}),
        ),
        (
            "transact",
            json!({"store": "notes", "tx": {"version": 1, "idempotency_key": ""}}),
        ),
        (
            "transact",
            json!({"store": "notes", "tx": {"version": 1, "idempotency_key": "k", "metadata": {"why": {"ref": {"id": entity}}}}}),
        ),
        (
            "transact",
            json!({"store": "notes", "tx": {"version": 1, "idempotency_key": "k", "changes": [{"op": "assert", "entity": {"name": "x"}, "attr": "note/rank", "value": {"i64": "1"}}]}}),
        ),
        ("head", json!({"store": "nope"})),
        ("head", json!({"handle": "h1"})),
        (
            "facts",
            json!({"store": "notes", "filter": {"attribute": "note/rank"}}),
        ),
        ("facts", json!({"store": "notes", "limit": 0})),
        ("pull", json!({"store": "notes", "entity": {"id": entity}})),
        (
            "lookup",
            json!({"store": "notes", "attr": "note/id", "value": {"string": "a", "i64": "1"}}),
        ),
        ("history", json!({"store": "notes", "after": "01"})),
        (
            "transact",
            json!({"store": "copy", "tx": {"version": 1, "idempotency_key": "k"}}),
        ),
        ("transact", json!({"store": "notes", "tx": {"version": 1}})),
        (
            "transact",
            json!({"store": "notes", "tx": {"version": 1, "idempotency_key": "k", "changes": [{"op": "upsert", "entity": {"id": entity}, "attr": "note/rank", "value": {"i64": "1"}}]}}),
        ),
        (
            "query",
            json!({"store": "notes", "query": {"version": 1, "query": {"select": []}}}),
        ),
    ];
    for (name, arguments) in refused {
        assert!(
            violation(&schema_of(name), &arguments, name).is_some(),
            "{name} {arguments}"
        );
        let result = server.ok("tools/call", json!({"name": name, "arguments": arguments}));
        assert_eq!(result["isError"], true, "{name} {arguments}");
    }
}

#[test]
fn read_only_stores_open_read_only_and_structural_validation_skips_the_log() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notes.sqlite");
    store_with(&path, 2);
    // Damage a retained report: a full open replays the log and refuses the
    // file; a structural open, which only a read-only connection accepts,
    // does not look.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("UPDATE transactions SET report = X'00'")
        .unwrap();
    let full = start(&bind("--store", "notes", &path));
    let (status, _, stderr) = full.finish();
    assert_eq!(status.code(), Some(2), "{stderr}");
    assert!(
        stderr.starts_with("factlite-mcp: cannot open store notes at "),
        "{stderr}"
    );

    let mut args: Vec<String> = bind("--store", "notes", &path).into();
    args.extend(["--validation".into(), "structural".into()]);
    let mut server = start(&args);
    let head = tool(&mut server, "head", json!({}));
    assert_eq!(head["snapshot"]["revision"], "3");
    // A read-only server lists no transact tool at all.
    let tools = server.ok("tools/list", json!({}))["tools"].clone();
    let names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "head",
            "schema",
            "facts",
            "pull",
            "lookup",
            "history",
            "transactions",
            "query"
        ]
    );
    // With one store bound, no tool requires naming it.
    for tool in tools.as_array().unwrap() {
        let required = tool["inputSchema"]["required"].as_array().unwrap();
        assert!(!required.contains(&json!("store")), "{tool}");
        assert_eq!(
            tool["inputSchema"]["properties"]["store"]["enum"],
            json!(["notes"])
        );
    }
}

#[test]
fn batches_strict_params_and_notifications_that_would_write() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notes.sqlite");
    store_with(&path, 1);
    let mut server = start(&bind("--writable-store", "notes", &path));

    // A batch, which the 2025-03-26 revision allows, is answered per entry.
    server.send(
        r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"},7,{"jsonrpc":"2.0","id":2,"method":"tools/list"}]"#,
    );
    let responses = server.response();
    let responses = responses.as_array().unwrap();
    assert_eq!(responses.len(), 3, "{responses:?}");
    assert_eq!(
        responses[0],
        json!({"jsonrpc": "2.0", "id": 1, "result": {}})
    );
    assert_eq!(responses[1]["error"]["code"], -32600);
    assert_eq!(responses[2]["id"], 2);
    assert_eq!(responses[2]["result"]["tools"].as_array().unwrap().len(), 9);

    // initialize needs the client's capabilities and information as objects.
    for params in [
        json!({"protocolVersion": "2025-11-25", "clientInfo": {"name": "t", "version": "0"}}),
        json!({"protocolVersion": "2025-11-25", "capabilities": {}}),
        json!({"protocolVersion": "2025-11-25", "capabilities": [], "clientInfo": {"name": "t", "version": "0"}}),
        json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": 0}),
    ] {
        assert_eq!(
            server.err("initialize", params.clone())["code"],
            -32602,
            "{params}"
        );
    }
    // The stateless revision needs capabilities as an object.
    for capabilities in [json!(null), json!(42), json!("x")] {
        let error = server.err(
            "server/discover",
            json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": capabilities}}),
        );
        assert_eq!(error["code"], -32602);
    }
    let error = server.err(
        "server/discover",
        json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "2025-11-25",
            "io.modelcontextprotocol/clientCapabilities": {}}}),
    );
    assert!(
        error["message"].as_str().unwrap().contains("2026-07-28"),
        "{error}"
    );
    // No list is paged, so no cursor is valid.
    for method in ["tools/list", "resources/list", "resources/templates/list"] {
        for cursor in [json!("abc"), json!(false)] {
            assert_eq!(
                server.err(method, json!({"cursor": cursor}))["code"],
                -32602,
                "{method}"
            );
        }
    }
    // An unknown tool is a protocol error even when its arguments are bad.
    server.send(
        r#"{"jsonrpc":"2.0","id":"t","method":"tools/call","params":{"name":"not_a_tool","arguments":{"a":1,"a":2}}}"#,
    );
    assert_eq!(server.response()["error"]["code"], -32602);

    // A write sent as a notification is not executed.
    server.send(
        &json!({"jsonrpc": "2.0", "method": "tools/call", "params": {"name": "transact",
            "arguments": {"tx": {"version": 1, "idempotency_key": "quiet"}}}})
        .to_string(),
    );
    let head = tool(&mut server, "head", json!({}));
    assert_eq!(head["snapshot"]["revision"], "2");
    // An empty key is not a key.
    let empty = tool_error(
        &mut server,
        "transact",
        json!({"tx": {"version": 1, "idempotency_key": ""}}),
        "InvalidRequest",
    );
    assert_eq!(empty["context"]["field"], "idempotency_key");

    // A failing tool in the stateless form is still a result.
    let failed = server.ok(
        "tools/call",
        json!({"_meta": meta(), "name": "facts", "arguments": {"filter": {"attr": "note/none"}}}),
    );
    assert_eq!(failed["isError"], true);
    assert_eq!(failed["resultType"], "complete");
    assert_eq!(text_of(&failed)["error"]["code"], "UnknownAttribute");
}

#[test]
fn store_names_that_differ_only_in_case_have_distinct_resources() {
    let directory = tempfile::tempdir().unwrap();
    let full = directory.path().join("full.sqlite");
    let empty = directory.path().join("empty.sqlite");
    store_with(&full, 0);
    Store::open(&empty).unwrap();
    let mut args: Vec<String> = bind("--store", "Upper", &full).into();
    args.extend(bind("--store", "upper", &empty));
    let mut server = start(&args);
    let schema_of = |server: &mut Server, uri: &str| {
        let read = server.ok("resources/read", json!({"uri": uri}));
        parse(read["contents"][0]["text"].as_str().unwrap())["value"]["schema"]
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(schema_of(&mut server, "factlite://stores/Upper/schema"), 3);
    assert_eq!(schema_of(&mut server, "factlite://stores/upper/schema"), 0);
    // The name sits in the path, so the authority is the same lowercase
    // word for every store and normalizing it changes nothing.
    let listed = server.ok("resources/list", json!({}));
    let uris: Vec<&str> = listed["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|resource| resource["uri"].as_str().unwrap())
        .collect();
    assert_eq!(
        uris,
        [
            "factlite://stores/Upper/schema",
            "factlite://stores/upper/schema"
        ]
    );
}
