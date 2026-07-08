//! End-to-end `bz --count-tokens` (architecture §5.10.1, anthropic-messages §2.11,
//! providers §10.1): the two live dialects (Anthropic + Google), the request read the SAME
//! way the data plane reads one, the two output shapes (`--json` object / bare number), and
//! the count-body projection (the messages/system/tools body MINUS the generation-only
//! keys). The decline arm + error paths live in `count_errors`. `MockTransport`; offline.

use crate::testing::{MemoryCredStore, MockTransport};
use crate::tests::count_support::{go, go_env, ANT, COUNT, REQ};
use crate::{Cred, Method, Secret};

#[test]
fn text_prints_the_bare_count_and_posts_the_count_endpoint() {
    let tx = MockTransport::ok(vec![COUNT]);
    let o = go(ANT, REQ, &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "42\n"); // bare number (text projection)
    assert!(o.stderr.is_empty());
    // ONE POST to `{base_url}/v1/messages/count_tokens`, auth + the required version.
    let sent = tx.requests();
    assert_eq!(sent.len(), 1, "exactly one round-trip");
    assert_eq!(sent[0].method, Method::Post);
    assert_eq!(
        sent[0].url,
        "https://api.anthropic.com/v1/messages/count_tokens"
    );
    assert_eq!(sent[0].header("x-api-key"), Some("sk"));
    assert_eq!(sent[0].header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(sent[0].header("content-type"), Some("application/json"));
    // The body is the messages body MINUS the generation-only keys the endpoint rejects.
    let b: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(b["model"], "claude-x");
    assert!(b["messages"].is_array());
    assert!(
        b.get("max_tokens").is_none(),
        "max_tokens is generation-only"
    );
    assert!(b.get("stream").is_none(), "stream is generation-only");
}

#[test]
fn json_emits_the_canonical_input_tokens_object() {
    let tx = MockTransport::ok(vec![COUNT]);
    let o = go(
        &[
            "--count-tokens",
            "--provider",
            "anthropic",
            "--api-key",
            "sk",
            "--json",
        ],
        REQ,
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    let v: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(v["input_tokens"], 42);
}

#[test]
fn brazen_output_ndjson_selects_the_object_with_no_flag() {
    // The output shape is the RESOLVED `OutMode` (flag/env/file), the same fact the data
    // plane folds — `BRAZEN_OUTPUT=ndjson` with NO `--json` selects the object form.
    let tx = MockTransport::ok(vec![COUNT]);
    let o = go_env(
        ANT,
        &[("BRAZEN_OUTPUT", "ndjson")],
        REQ,
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    let v: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(v["input_tokens"], 42);
}

#[test]
fn a_positional_prompt_is_the_request() {
    // `--count-tokens` reads a request the SAME way the data plane does (§5.5): a positional
    // prompt is the request and stdin is not read (here it is empty anyway).
    let tx = MockTransport::ok(vec![COUNT]);
    let o = go(
        &[
            "--count-tokens",
            "--provider",
            "anthropic",
            "--api-key",
            "sk",
            "--model",
            "claude-x",
            "count",
            "this",
        ],
        b"",
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "42\n");
    let b: serde_json::Value = serde_json::from_slice(&tx.requests()[0].body).unwrap();
    // The prompt is the trailing user-message text part.
    assert_eq!(b["messages"][0]["content"][0]["text"], "count this");
}

#[test]
fn the_full_request_shape_rides_the_count_body_minus_generation_keys() {
    // A rich request exercises every count-body branch: reasoning→thinking, system, tools,
    // tool_choice + disable_parallel, the `extra` passthrough — and drops the generation
    // controls (`temperature`, `max_tokens`, `stream`) the count endpoint does not accept.
    let req = br#"{
        "model":"claude-x",
        "system":[{"type":"text","text":"sys"}],
        "messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}],
        "tools":[{"name":"t","input_schema":{"type":"object"}}],
        "tool_choice":{"type":"any"},
        "parallel_tool_calls":false,
        "reasoning":"low",
        "temperature":0.5,
        "metadata":{"k":"v"}
    }"#;
    let tx = MockTransport::ok(vec![COUNT]);
    let o = go(ANT, req, &tx, &MemoryCredStore::new());
    assert_eq!(o.code, 0);
    let b: serde_json::Value = serde_json::from_slice(&tx.requests()[0].body).unwrap();
    assert_eq!(b["thinking"]["type"], "enabled"); // reasoning → thinking
    assert_eq!(b["system"][0]["text"], "sys");
    assert_eq!(b["tools"][0]["name"], "t");
    assert_eq!(b["tool_choice"]["type"], "any");
    assert_eq!(b["tool_choice"]["disable_parallel_tool_use"], true);
    assert_eq!(b["metadata"]["k"], "v"); // the `extra` fold
    assert!(
        b.get("temperature").is_none(),
        "temperature is generation-only"
    );
    assert!(b.get("max_tokens").is_none());
    assert!(b.get("stream").is_none());
}

#[test]
fn google_counts_via_the_generate_content_request_envelope() {
    // Google's count endpoint takes a `generateContentRequest` envelope; the body reuses
    // this dialect's `encode` projection and injects the `model` the URL path omits.
    let tx = MockTransport::ok(vec![br#"{"totalTokens":7}"#]);
    let o = go(
        &["--count-tokens", "--provider", "google", "--api-key", "sk"],
        REQ,
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "7\n"); // decoded from `totalTokens`, printed as the canonical count
    let sent = tx.requests();
    assert_eq!(sent[0].method, Method::Post);
    assert_eq!(
        sent[0].url,
        "https://generativelanguage.googleapis.com/v1beta/models/claude-x:countTokens"
    );
    assert_eq!(sent[0].header("x-goog-api-key"), Some("sk"));
    let b: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(b["generateContentRequest"]["model"], "models/claude-x");
    assert!(b["generateContentRequest"]["contents"].is_array());
}

#[test]
fn input_file_is_read_as_the_request() {
    // `--input FILE` is the simulated pipe (§5.5): the request comes from the named file,
    // read exactly as the data plane reads it, so stdin is not consulted.
    let path = std::env::temp_dir().join(format!("brazen_count_{}.json", std::process::id()));
    std::fs::write(&path, REQ).unwrap();
    let tx = MockTransport::ok(vec![COUNT]);
    let o = go(
        &[
            "--count-tokens",
            "--provider",
            "anthropic",
            "--api-key",
            "sk",
            "--input",
            path.to_str().unwrap(),
        ],
        b"",
        &tx,
        &MemoryCredStore::new(),
    );
    let _ = std::fs::remove_file(&path);
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "42\n");
}

#[test]
fn a_stored_credential_is_used_for_the_count() {
    let store = MemoryCredStore::with(
        "anthropic",
        Cred::ApiKey {
            key: Secret::new("sk-store"),
        },
    );
    let tx = MockTransport::ok(vec![COUNT]);
    let o = go(
        &["--count-tokens", "--provider", "anthropic"],
        REQ,
        &tx,
        &store,
    );
    assert_eq!(o.code, 0);
    assert_eq!(tx.requests()[0].header("x-api-key"), Some("sk-store"));
}

#[test]
fn help_and_version_probes_win_before_any_network() {
    // The probes ride the SAME flag layer + doc, answering with no provider/network.
    let tx = MockTransport::ok(vec![COUNT]);
    let h = go(
        &["--count-tokens", "--help"],
        b"",
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(h.code, 0);
    assert!(h.stdout.contains("--count-tokens"));
    let v = go(
        &["--count-tokens", "--version"],
        b"",
        &tx,
        &MemoryCredStore::new(),
    );
    assert_eq!(v.code, 0);
    assert!(v.stdout.starts_with("bz "));
    assert!(tx.requests().is_empty(), "a probe makes no round-trip");
}
