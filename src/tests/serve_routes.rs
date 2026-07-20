//! The `bz --serve` pseudo-routes + error masquerade (ingress.md §8, §9, §14):
//! `GET /v1/models` cold/warm/aliases (never an upstream list), the dialect 404
//! for unknown routes, malformed HTTP → 400 + close, carried upstream statuses
//! (incl. the reason-phrase-less exotic tail), the 502 gateway answer for a
//! transport failure, and a per-request config failure answering 500 while the
//! loop stays up.

use crate::testing::{MemConn, MemoryModelCache, MockTransport};
use crate::tests::run_support::{temp, BASIC};
use crate::tests::serve_support::*;
use crate::Model;

#[test]
fn v1_models_unions_the_cache_with_every_rows_aliases() {
    let cfg = masq_cfg("");
    // Cold cache: aliases only (§8).
    let (conn, wrote) = MemConn::new(b"GET /v1/models HTTP/1.1\r\n\r\n");
    drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    let out = wrote_str(&wrote);
    let body: serde_json::Value =
        serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["object"], "list");
    assert_eq!(body["data"][0]["id"], "gpt-4o", "{out}");
    assert_eq!(body["data"][0]["object"], "model");
    assert_eq!(body["data"][0]["owned_by"], "anthropic");
    assert_eq!(
        body["data"].as_array().unwrap().len(),
        1,
        "cold cache → aliases only"
    );

    // Warm: cached ids join the union; a query string still routes (§8).
    let cache = MemoryModelCache::with(
        "anthropic",
        vec![Model {
            id: "claude-x".into(),
            ..Model::default()
        }],
    );
    let (conn, wrote) = MemConn::new(b"GET /v1/models?probe=1 HTTP/1.1\r\n\r\n");
    drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &cache,
    );
    let body: serde_json::Value =
        serde_json::from_str(wrote_str(&wrote).split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let ids: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["claude-x", "gpt-4o"], "cache ∪ aliases, sorted");
}

#[test]
fn unknown_routes_get_the_dialect_404_envelope() {
    let cfg = masq_cfg("");
    let (conn, wrote) = MemConn::new(b"GET /v1/health HTTP/1.1\r\n\r\n");
    drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    let out = wrote_str(&wrote);
    assert!(out.starts_with("HTTP/1.1 404 Not Found"), "{out}");
    assert!(out.contains("no route for GET /v1/health"), "{out}");
}

#[test]
fn malformed_http_gets_the_dialect_400_and_the_connection_closes() {
    let cfg = masq_cfg("");
    let cases: &[&[u8]] = &[
        b"GARBAGE\r\n\r\n",                                    // bad request line
        b"GET / HTTP/2\r\n\r\n",                               // unsupported version
        b"POST / HTTP/1.1\r\nno-colon-here\r\n\r\n",           // bad header line
        b"POST / HTTP/1.1\r\ncontent-length: nope\r\n\r\n",    // bad length
        b"POST / HTTP/1.1\r\ncontent-length: 99\r\n\r\nshort", // torn body
        b"POST / HTTP/1.1\r\n",                                // EOF inside headers
        b"POST / HTTP/1.1\r\nx: \xff\xfe\r\n\r\n",             // non-UTF-8 header
    ];
    for case in cases {
        let (conn, wrote) = MemConn::new(case);
        drive(
            &cfg,
            vec![Box::new(conn)],
            &MockTransport::ok(vec![BASIC]),
            &MemoryModelCache::new(),
        );
        let out = wrote_str(&wrote);
        assert!(
            out.starts_with("HTTP/1.1 400 Bad Request"),
            "{case:?}: {out}"
        );
        assert!(out.contains("malformed HTTP request"), "{out}");
    }
}

#[test]
fn upstream_statuses_masquerade_through_including_exotic_ones() {
    let cfg = masq_cfg("");
    for (status, line) in [
        (429, "HTTP/1.1 429 Too Many Requests"),
        (418, "HTTP/1.1 418 "), // no reason phrase for the exotic tail
    ] {
        let tx = MockTransport::new(
            status,
            vec![crate::testing::Chunk::Data(
                br#"{"type":"error","error":{"type":"x","message":"nope"}}"#.to_vec(),
            )],
        );
        let (conn, wrote) = MemConn::new(&post("/v1/chat/completions", AGG, ""));
        drive(&cfg, vec![Box::new(conn)], &tx, &MemoryModelCache::new());
        let out = wrote_str(&wrote);
        assert!(out.starts_with(line), "{status}: {out}");
    }
    // And a transport failure is brazen answering AS the gateway: 502.
    let (conn, wrote) = MemConn::new(&post("/v1/chat/completions", AGG, ""));
    drive(
        &cfg,
        vec![Box::new(conn)],
        &crate::tests::run_support::ErrTransport,
        &MemoryModelCache::new(),
    );
    assert!(wrote_str(&wrote).starts_with("HTTP/1.1 502 Bad Gateway"));
}

#[test]
fn a_config_failure_on_the_data_route_answers_500_and_stays_up() {
    // A row whose empty `model_prefixes` element would own every model is
    // `BadValue`/78 (config §7) → resolution fails per request; the §9 envelope
    // answers 500 and the LOOP keeps serving (the process never dies). Two rows
    // owning one model is NOT such a failure — greedy-first picks the first
    // (arch §4.3, `config_priority`).
    let cfg = temp(
        r#"
api_key = "sk"

[ingress]
dialect = "openai_chat"

[[provider]]
name = "anthropic"
model_prefixes = [""]
"#,
    );
    let (conn, wrote) = MemConn::new(&post("/v1/chat/completions", AGG, ""));
    let (code, _, _) = drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    assert_eq!(code, 0);
    let out = wrote_str(&wrote);
    assert!(
        out.starts_with("HTTP/1.1 500 Internal Server Error"),
        "{out}"
    );
    assert!(out.contains(r#""type":"server_error""#), "{out}");
}
