//! The session printed in `docs/rpc.md`, run as written.

mod common;

use assert_cmd::cargo::cargo_bin;
use common::Server;
use serde_json::Value as Json;

/// The lines of the first fenced block with this info string.
fn block<'a>(guide: &'a str, info: &str) -> Vec<&'a str> {
    let start = guide
        .find(&format!("```{info}\n"))
        .unwrap_or_else(|| panic!("the guide has a {info} block"));
    let body = &guide[start + info.len() + 4..];
    let end = body.find("\n```").expect("the block is closed");
    body[..end].lines().collect()
}

#[test]
fn the_guide_session_succeeds_line_by_line_and_leaves_what_it_describes() {
    let guide = include_str!("../../../docs/rpc.md");
    let requests = block(guide, "jsonl");
    assert!(requests.len() >= 6, "the guide shows a whole session");

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("demo.sqlite");
    let quoted = serde_json::to_string(&path).unwrap();
    let mut server = Server::start(&cargo_bin("factlite-rpc"), &[]);
    let mut results = Vec::new();
    for (index, request) in requests.iter().enumerate() {
        assert!(
            index != 0 || request.contains(r#""path":"demo.sqlite""#),
            "the first request opens the demo store"
        );
        server.send(&request.replace(r#""demo.sqlite""#, &quoted));
        let response = server.response();
        let sent: Json = serde_json::from_str(request).unwrap();
        assert_eq!(response["id"], sent["id"], "{request}");
        assert!(response.get("error").is_none(), "{request}\n{response}");
        results.push((
            sent["method"].as_str().unwrap().to_owned(),
            response["result"].clone(),
        ));
    }
    let of = |method: &str| -> Vec<&Json> {
        results
            .iter()
            .filter(|(name, _)| name == method)
            .map(|(_, result)| result)
            .collect()
    };

    // What the prose around the block says the responses hold.
    assert_eq!(of("open")[0]["handle"], "h1");
    assert_eq!(of("open")[0]["head"]["revision"], "0");
    let lookup = of("lookup")[0];
    assert_eq!(lookup["basis"]["revision"], "1");
    assert!(
        lookup["value"]["entity"]["id"]
            .as_str()
            .unwrap()
            .starts_with("e1.")
    );
    assert_eq!(lookup["value"]["version"], 1);
    // The status was set and then replaced: three events, one current row.
    assert_eq!(
        of("history")[0]["value"]["events"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        of("query")[0]["value"]["rows"],
        serde_json::json!([[{"string": "T-1"}]])
    );
    assert_eq!(of("close")[0], &serde_json::json!({}));

    // The two example responses in the guide are the shapes actually sent,
    // with identities elided.
    for example in block_all(guide, "json") {
        let example: Json = serde_json::from_str(example).unwrap();
        let actual = results
            .iter()
            .zip(&requests)
            .find(|(_, request)| {
                serde_json::from_str::<Json>(request).unwrap()["id"] == example["id"]
            })
            .map(|((_, result), _)| result)
            .expect("the example answers a request in the session");
        assert_same_shape(&example["result"], actual, "result");
    }
}

/// Every fenced block with this info string, as one line each.
fn block_all<'a>(guide: &'a str, info: &str) -> Vec<&'a str> {
    let marker = format!("```{info}\n");
    guide
        .match_indices(&marker)
        .map(|(start, _)| {
            let body = &guide[start + marker.len()..];
            &body[..body.find("\n```").expect("the block is closed")]
        })
        .collect()
}

/// The same members and kinds; strings match unless the guide elided them
/// with an ellipsis.
fn assert_same_shape(example: &Json, actual: &Json, at: &str) {
    match (example, actual) {
        (Json::Object(example), Json::Object(actual)) => {
            let keys = |object: &serde_json::Map<String, Json>| -> Vec<String> {
                object.keys().cloned().collect()
            };
            assert_eq!(keys(example), keys(actual), "{at}");
            for (key, value) in example {
                assert_same_shape(value, &actual[key], &format!("{at}.{key}"));
            }
        }
        (Json::String(example), Json::String(actual)) if example.contains('…') => {
            let mut parts = example.split('…');
            let prefix = parts.next().unwrap();
            let suffix = parts.next_back().unwrap_or("");
            assert!(
                actual.starts_with(prefix) && actual.ends_with(suffix),
                "{at}: {example} against {actual}"
            );
        }
        _ => assert_eq!(example, actual, "{at}"),
    }
}
