//! End-to-end `serve` probe orchestration (model-discovery §5.2, §8): the imprecise
//! path prepends ONE models-list GET, expands the seed against it, then runs the
//! UNCHANGED generation POST carrying the expanded wire id. The precise path sends
//! exactly once (no probe). Driven by `ScriptedTransport` (a distinct body per send);
//! zero network.

mod run_support;

use brazen::testing::{MockTransport, ScriptedTransport};
use brazen::Method;
use run_support::*;

/// The anthropic `/v1/models` body (newest-first), `claude-opus-4-1-…` first — the
/// seed `opus` expands to it (exact-before-contains, first-in-order, §4).
const MODELS: &[u8] = br#"{"data":[
    {"type":"model","id":"claude-opus-4-1-20250805"},
    {"type":"model","id":"claude-sonnet-4-5-20250929"}
],"has_more":false}"#;

#[test]
fn probe_prepends_a_models_get_then_the_chat_post_with_the_expanded_model() {
    // `--provider anthropic --model opus`: the row owns `claude-` not `opus`, so the
    // model is a SEED and `cfg.probe` is true (model-discovery §5.1). Send #1 returns
    // the models list; send #2 is the chat stream.
    let tx = ScriptedTransport::new(vec![(200, MODELS.to_vec()), (200, BASIC.to_vec())]);
    let o = go(
        &[
            "hi",
            "--provider",
            "anthropic",
            "--model",
            "opus",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "Hello"); // the generation streamed through unchanged

    let sent = tx.requests();
    assert_eq!(
        sent.len(),
        2,
        "exactly one probe + one generation round-trip"
    );

    // Send #1: a GET to {base_url}{models_path}, no body, carrying the row's required
    // `anthropic-version` header (the probe skips `encode`, which would otherwise stamp
    // it — a bare GET 400s on `/v1/models` without it).
    assert_eq!(sent[0].method, Method::Get);
    assert_eq!(sent[0].url, "https://api.anthropic.com/v1/models");
    assert!(sent[0].body.is_empty());
    assert_eq!(sent[0].header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(sent[0].header("x-api-key"), Some("sk")); // same auth seam

    // Send #2: the chat POST, its encoded body carrying the EXPANDED wire id, never
    // the `opus` seed.
    assert_eq!(sent[1].method, Method::Post);
    assert_eq!(sent[1].url, "https://api.anthropic.com/v1/messages");
    let body = String::from_utf8_lossy(&sent[1].body).into_owned();
    assert!(
        body.contains("claude-opus-4-1-20250805"),
        "the POST carries the expanded model: {body}"
    );
    assert!(
        !body.contains("\"opus\""),
        "the seed never reaches the wire"
    );
}

#[test]
fn absent_model_probes_and_picks_the_first_listed() {
    // No `--model`: the seed is `""`, so `cfg.probe` is true and `select_model` takes
    // the default — `models[0]` (none flags a default today, §4).
    let tx = ScriptedTransport::new(vec![(200, MODELS.to_vec()), (200, BASIC.to_vec())]);
    let o = go(
        &["hi", "--provider", "anthropic", "--api-key", "sk"],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    let sent = tx.requests();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].method, Method::Get);
    let body = String::from_utf8_lossy(&sent[1].body).into_owned();
    assert!(body.contains("claude-opus-4-1-20250805"));
}

#[test]
fn a_full_owned_model_sends_exactly_once_no_probe() {
    // `--model claude-x` is owned by the `claude-` prefix, so `cfg.probe` is false:
    // the model is the final wire id and `serve` is one round-trip, unchanged.
    let tx = MockTransport::ok(vec![BASIC]);
    let o = go(
        &[
            "hi",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 0);
    let sent = tx.requests();
    assert_eq!(
        sent.len(),
        1,
        "no probe — the precise path is one round-trip"
    );
    assert_eq!(sent[0].method, Method::Post);
    assert_eq!(sent[0].url, "https://api.anthropic.com/v1/messages");
}

#[test]
fn an_unmatched_seed_is_config_78_in_band() {
    // The probe returns a list, but no id contains `mythical` → `select_model` fails
    // Config (78), surfaced in-band like any pre-stream error.
    let tx = ScriptedTransport::new(vec![(200, MODELS.to_vec())]);
    let o = go(
        &[
            "hi",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "mythical",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 78);
    assert!(o.stdout.contains(r#""config""#));
    // Only the probe was sent — the generation never happened.
    assert_eq!(tx.requests().len(), 1);
}

#[test]
fn a_non_2xx_probe_carries_the_status_and_skips_generation() {
    // The models GET returns 500 → `from_http_status` → Provider{500} → exit 70, and
    // no generation request follows (the seed never expanded).
    let tx = ScriptedTransport::new(vec![(500, b"upstream boom".to_vec())]);
    let o = go(
        &[
            "hi",
            "--json",
            "--provider",
            "anthropic",
            "--model",
            "opus",
            "--api-key",
            "sk",
        ],
        &[],
        b"",
        &tx,
        &empty_store(),
    );
    assert_eq!(o.code, 70);
    assert!(o.stdout.contains(r#""provider""#));
    assert_eq!(tx.requests().len(), 1);
}
