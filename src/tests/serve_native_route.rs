//! The native anthropic route under `bz --serve` (ingress.md §8, §14 — wave 3):
//! `POST /v1/messages` selects the `anthropic_messages` codec BY PATH (the route
//! table is the dialect signal; no new config), the real-SDK driver is a verbatim
//! wire capture of the anthropic python SDK's `client.messages.create(...)`
//! request round-tripped through the listener, and every HTTP-layer error on the
//! native surface wears the anthropic `{"type":"error","error":{…}}` envelope —
//! the precise status riding the HTTP status line only (the §12 narrowing).

use crate::testing::{MemConn, MemoryModelCache, MockTransport};
use crate::tests::run_support::{temp, BASIC};
use crate::tests::serve_support::*;

/// The body the anthropic python SDK serializes for a plain
/// `messages.create(model=…, max_tokens=1024, messages=[{"role":"user",…}])`.
const SDK_AGG: &str = r#"{"max_tokens":1024,"messages":[{"role":"user","content":"Hello, world"}],"model":"claude-opus-4-8"}"#;
/// The same call with `stream=True`.
const SDK_SSE: &str = r#"{"max_tokens":1024,"messages":[{"role":"user","content":"Hello, world"}],"model":"claude-opus-4-8","stream":true}"#;

/// A verbatim capture of the SDK's HTTP/1.1 request (httpx/h11 wire framing,
/// header spellings as sent — the fictional client key included), re-targeted
/// at the listener. Only `body` and the extra header block vary per test.
fn sdk_request(body: &str, extra: &str) -> Vec<u8> {
    format!(
        "POST /v1/messages HTTP/1.1\r\n\
         Host: 127.0.0.1:4891\r\n\
         Accept: application/json\r\n\
         Accept-Encoding: gzip, deflate\r\n\
         Connection: keep-alive\r\n\
         User-Agent: Anthropic/Python 0.58.2\r\n\
         X-Stainless-Lang: python\r\n\
         X-Stainless-Package-Version: 0.58.2\r\n\
         X-Stainless-OS: Linux\r\n\
         X-Stainless-Arch: x64\r\n\
         X-Stainless-Runtime: CPython\r\n\
         X-Stainless-Runtime-Version: 3.12.3\r\n\
         anthropic-version: 2023-06-01\r\n\
         x-api-key: sk-ant-client-fiction\r\n\
         X-Stainless-Retry-Count: 0\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         {extra}\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[test]
fn a_verbatim_sdk_request_round_trips_the_native_route() {
    // No dialect is configured (masq_cfg) — the PATH selects the codec, so the
    // anthropic SDK works with zero config change (§8).
    let cfg = masq_cfg("");
    let tx = MockTransport::ok(vec![BASIC]);
    let (conn, wrote) = MemConn::new(&sdk_request(SDK_AGG, ""));
    let (code, _, err) = drive(&cfg, vec![Box::new(conn)], &tx, &MemoryModelCache::new());
    assert_eq!(code, 0, "{err}");
    let out = wrote_str(&wrote);
    assert!(
        out.starts_with("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: "),
        "{out}"
    );
    let body: serde_json::Value =
        serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["type"], "message");
    assert_eq!(body["role"], "assistant");
    assert_eq!(body["id"], "msg_01XYZ", "upstream identity wins");
    assert_eq!(body["model"], "claude-opus-4-8");
    assert_eq!(body["content"][0]["type"], "text");
    assert_eq!(body["content"][0]["text"], "Hello");
    assert_eq!(body["stop_reason"], "end_turn");
    assert_eq!(body["usage"]["input_tokens"], 12);
    // The upstream half really ran the ordinary pipeline: the egress anthropic
    // wire, under the ROW's credential — the SDK's fictional key is ignored (§7).
    let sent = tx.requests();
    assert_eq!(sent[0].url, "https://api.anthropic.com/v1/messages");
    let key = sent[0].headers.iter().find(|(n, _)| n == "x-api-key");
    assert_eq!(key.map(|(_, v)| v.as_str()), Some("sk-test"), "row auth");
}

#[test]
fn a_streaming_sdk_request_gets_the_anthropic_native_sse_framing() {
    let cfg = masq_cfg("");
    let (conn, wrote) = MemConn::new(&sdk_request(SDK_SSE, ""));
    drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    let out = wrote_str(&wrote);
    assert!(
        out.starts_with("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream"),
        "{out}"
    );
    for frame in [
        "event: message_start\ndata: ",
        "event: content_block_delta\ndata: ",
        "event: message_delta\ndata: ",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}",
    ] {
        assert!(out.contains(frame), "missing {frame:?}: {out}");
    }
    assert!(
        !out.contains("[DONE]"),
        "openai's sentinel never leaks: {out}"
    );
    assert!(out.ends_with("0\r\n\r\n"), "chunked terminator: {out}");
}

#[test]
fn native_route_edge_rejections_wear_the_anthropic_envelope() {
    let cfg = masq_cfg("");
    // A path under the native surface with no route: 404, anthropic-shaped —
    // the envelope golden, byte for byte.
    let (conn, wrote) = MemConn::new(&post("/v1/messages/count_tokens", "{}", ""));
    drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    let out = wrote_str(&wrote);
    assert!(out.starts_with("HTTP/1.1 404 Not Found"), "{out}");
    assert_eq!(
        out.split("\r\n\r\n").nth(1).unwrap(),
        r#"{"error":{"message":"no route for POST /v1/messages/count_tokens","type":"not_found_error"},"type":"error"}"#
    );

    // A decode failure on the data route: the anthropic 400, naming the shape.
    let (conn, wrote) = MemConn::new(&post("/v1/messages", "not json", ""));
    drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    let out = wrote_str(&wrote);
    assert!(out.starts_with("HTTP/1.1 400 Bad Request"), "{out}");
    let body: serde_json::Value =
        serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("anthropic_messages ingress: request body is not JSON"),
        "{body}"
    );
}

#[test]
fn the_bearer_gate_401_is_anthropic_shaped_on_the_native_route() {
    // The path resolves the dialect BEFORE the gate, so even the 401 wears the
    // native envelope — the golden, byte for byte.
    let cfg = masq_cfg("token = \"hunter2\"\n");
    let (conn, wrote) = MemConn::new(&sdk_request(SDK_AGG, ""));
    drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    let out = wrote_str(&wrote);
    assert!(out.starts_with("HTTP/1.1 401 Unauthorized"), "{out}");
    assert_eq!(
        out.split("\r\n\r\n").nth(1).unwrap(),
        r#"{"error":{"message":"missing or invalid bearer token","type":"authentication_error"},"type":"error"}"#
    );
    // And the right token admits the SDK request through the same headers.
    let (conn, wrote) = MemConn::new(&sdk_request(SDK_AGG, "authorization: Bearer hunter2\r\n"));
    drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    assert!(wrote_str(&wrote).starts_with("HTTP/1.1 200 OK"));
}

#[test]
fn an_upstream_error_masquerades_natively_with_the_carried_status() {
    // Upstream 429: the precise status rides the HTTP line; in-band the type
    // coarsens to its family (the §12 narrowing — no numeric status slot).
    let cfg = masq_cfg("");
    let tx = MockTransport::new(
        429,
        vec![crate::testing::Chunk::Data(
            br#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#
                .to_vec(),
        )],
    );
    let (conn, wrote) = MemConn::new(&sdk_request(SDK_AGG, ""));
    drive(&cfg, vec![Box::new(conn)], &tx, &MemoryModelCache::new());
    let out = wrote_str(&wrote);
    assert!(out.starts_with("HTTP/1.1 429 Too Many Requests"), "{out}");
    let body: serde_json::Value =
        serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "rate_limit_error");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("slow down"),
        "{body}"
    );
}

#[test]
fn the_path_picks_the_codec_and_routeless_surfaces_wear_the_fixed_envelope() {
    // The path IS the dialect signal (§8): the openai-shaped route answers in
    // openai_chat and the anthropic route in anthropic_messages, whatever else
    // is configured — there is no dialect config key to outrank. A path NO
    // dialect owns falls back to the FIXED openai_chat envelope (the §8-style
    // narrowing: a client on an unknown route is unknown by definition, so the
    // routeless envelope is pinned, not guessed from config).
    let cfg = temp(
        r#"
api_key = "sk-test"

[ingress]

[[provider]]
name = "anthropic"
model_aliases = { "gpt-4o" = "claude-x" }

[[provider]]
name = "openai"
model_prefixes = []
"#,
    );
    let (conn, wrote) = MemConn::new(&post("/v1/chat/completions", AGG, ""));
    drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    let out = wrote_str(&wrote);
    assert!(out.starts_with("HTTP/1.1 200 OK"), "{out}");
    assert!(out.contains(r#""object":"chat.completion""#), "{out}");

    let (conn, wrote) = MemConn::new(b"GET /v1/health HTTP/1.1\r\n\r\n");
    drive(
        &cfg,
        vec![Box::new(conn)],
        &MockTransport::ok(vec![BASIC]),
        &MemoryModelCache::new(),
    );
    let out = wrote_str(&wrote);
    assert!(out.starts_with("HTTP/1.1 404 Not Found"), "{out}");
    assert_eq!(
        out.split("\r\n\r\n").nth(1).unwrap(),
        r#"{"error":{"code":404,"message":"no route for GET /v1/health","param":null,"type":"invalid_request_error"}}"#,
        "the routeless surface wears the FIXED openai_chat envelope"
    );
}
