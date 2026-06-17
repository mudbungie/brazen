//! Live, opt-in FUZZ of the OpenAI "Sign in with ChatGPT" (codex backend)
//! integration (bl-b72f). Where `live_conformance.rs` (bl-04dc) asserts the ONE
//! canonical happy path, this drives a WIDE range of request shapes at the live
//! `openai-chatgpt` provider — the codex backend's hard request preconditions and
//! well-formed variations — asserting brazen's error-mapping / normalization matches
//! what the service actually does. It REUSES the bl-04dc harness leaves verbatim
//! (`live_support/exec.rs`, `…/grammar.rs`) via `#[path]`. Black-box, no lib linkage →
//! the coverage-excluded `bz/` crate; never runs in `make check`.
//!
//! `#[ignore]`d AND `BRAZEN_LIVE`-gated; SKIPS (printed) without a `bz login
//! openai-chatgpt` cred. The error matrix is ~free (400s before generation); the
//! acceptance set GENERATES, so it is behind a SECOND opt-in (`BRAZEN_LIVE_FUZZ_SPEND=1`)
//! and prints what ran vs capped (AGENTS.md). Validated live 2026-06-16 (auth §10.7).
//!
//! ```text
//! BRAZEN_LIVE=1 BRAZEN_LIVE_FUZZ_SPEND=1 \
//!   cargo test -p bz --test live_fuzz_openai -- --ignored --nocapture
//! ```

#[allow(dead_code)] // `connectable` is unused here (keyless probe; we read a cred).
#[path = "live_support/exec.rs"]
mod exec;
#[path = "live_support/grammar.rs"]
mod grammar;
#[path = "live_support/openai.rs"]
mod openai;

use serde_json::{json, Map, Value};

use exec::cred_file;
use openai::{body, check_accept, check_error, flag, model, PROVIDER};

/// Gated for a ChatGPT account → 400 "…not supported" (the unsupported-model case).
const UNSUPPORTED_MODEL: &str = "gpt-5-codex";
const SYSTEM: &str = "You are a terse assistant. Reply with exactly one word when asked.";
const PROMPT: &str = "reply with the single word: ok";

/// The FULLY-VALID codex request body (the GENERAL path). Every error case is this
/// map with ONE codex-required key removed/flipped — special cases dissolved into
/// one builder (AGENTS.md). `store` is not a typed field; it rides the request
/// `extra` flatten onto the wire body.
fn valid() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("stream".into(), json!(true));
    m.insert("store".into(), json!(false));
    m.insert("system".into(), json!([{ "type": "text", "text": SYSTEM }]));
    m.insert(
        "messages".into(),
        json!([{ "role": "user", "content": [{ "type": "text", "text": PROMPT }] }]),
    );
    m
}

/// Well-formed request-shape variations the codex backend MUST accept, `(label,
/// is_tool, body)`. Each GENERATES (costs tokens), so the set is `spend`-gated.
fn accept_cases() -> Vec<(&'static str, bool, String)> {
    let mut uni = valid(); // unicode + emoji content (multi-byte text intact)
    uni.insert(
        "messages".into(),
        json!([{ "role": "user", "content": [{ "type": "text",
            "text": "Répondez « 🌍 » mais — reply with the single word: ok" }] }]),
    );
    // Multi-turn ordering: user / assistant / user (message role + order surface).
    let mut multi = valid();
    multi.insert(
        "messages".into(),
        json!([
            { "role": "user", "content": [{ "type": "text", "text": "Say the letter A." }] },
            { "role": "assistant", "content": [{ "type": "text", "text": "A" }] },
            { "role": "user", "content": [{ "type": "text", "text": PROMPT }] },
        ]),
    );
    // Tool def + the `{type:tool,name}` tool_choice spelling → a tool round-trip.
    let mut tool = valid();
    tool.insert(
        "messages".into(),
        json!([{ "role": "user", "content": [{ "type": "text",
            "text": "What is the weather in Paris? Call get_weather." }] }]),
    );
    tool.insert(
        "tools".into(),
        json!([{ "name": "get_weather", "description": "Weather for a city",
            "input_schema": { "type": "object",
                "properties": { "city": { "type": "string" } }, "required": ["city"] } }]),
    );
    tool.insert(
        "tool_choice".into(),
        json!({ "type": "tool", "name": "get_weather" }),
    );
    // `stream:false` — once a codex 400 mandate, now ACCEPTED (drift, bl-f8f7's
    // filed ball). bz's non-streaming decode must still emit the canonical grammar.
    let mut sfalse = valid();
    sfalse.insert("stream".into(), json!(false));
    vec![
        ("unicode-content", false, body(&uni)),
        ("multiturn-order", false, body(&multi)),
        ("tool-required", true, body(&tool)),
        ("stream-false", false, body(&sfalse)),
    ]
}

#[test]
#[ignore = "live: drives the codex backend over the network; run with --ignored"]
fn fuzz_openai_chatgpt_codex() {
    if !flag("BRAZEN_LIVE") {
        eprintln!("skipping OpenAI ChatGPT-SSO fuzz: set BRAZEN_LIVE=1 to run it");
        return;
    }
    if cred_file(PROVIDER).is_none() {
        eprintln!("skipping OpenAI ChatGPT-SSO fuzz: no stored `{PROVIDER}` cred — `bz login {PROVIDER}` first");
        return;
    }
    let m = model();
    println!("== {PROVIDER} fuzz ==  model {m}");
    let mut fails: Vec<String> = Vec::new();

    // 1) Error-conformance matrix (auth §10.7, validated live): each is the valid
    //    body MINUS one codex-required field → a specific 400 → exit 69, whose
    //    surfaced message must carry the service's wording. Near-free (no generation).
    println!("-- error conformance (near-free 400s) --");
    let (mut no_system, mut no_store) = (valid(), valid());
    no_system.remove("system");
    no_store.remove("store");
    let (bi, bs, bv) = (body(&no_system), body(&no_store), body(&valid()));
    let errors = [
        (
            "missing-instructions",
            m.as_str(),
            &bi,
            "Instructions are required",
        ),
        (
            "missing-store",
            m.as_str(),
            &bs,
            "Store must be set to false",
        ),
        // NB: `stream:false` was a codex mandate ("Stream must be set to true") at
        // bl-b72f; the backend DROPPED it (live 2026-06-17, bl-f8f7 / its filed ball)
        // — it now ACCEPTS a non-streamed request, so the case moved to acceptance
        // below (bz's non-streaming decode still yields the canonical grammar).
        ("unsupported-model", UNSUPPORTED_MODEL, &bv, "not supported"),
    ];
    let n_err = errors.len();
    for (label, mdl, b, phrase) in errors {
        if let Some(f) = check_error(label, mdl, b, phrase) {
            fails.push(f);
        }
    }

    // 2) Request-shape acceptance — TOKEN-COSTING, so behind the second opt-in.
    let accepts = accept_cases();
    let n_acc = accepts.len();
    let ran_acc = if flag("BRAZEN_LIVE_FUZZ_SPEND") {
        println!("-- request-shape acceptance ({n_acc} token-costing runs) --");
        for (label, is_tool, b) in accepts {
            if let Some(f) = check_accept(label, is_tool, &b) {
                fails.push(f);
            }
        }
        n_acc
    } else {
        println!("-- request-shape acceptance: SKIPPED {n_acc} token-costing runs (set BRAZEN_LIVE_FUZZ_SPEND=1) --");
        0
    };

    println!(
        "\n{n_err} error case(s) + {ran_acc}/{n_acc} acceptance case(s) exercised; {} failure(s)",
        fails.len()
    );
    // No silent truncation (AGENTS.md): raw-SSE golden capture (bl-080b having
    // landed, `--raw` now reaches the wire) is intentionally NOT duplicated here —
    // the offline `response.*` decoder is already exhaustively fixture-tested
    // (responses_fixtures.rs / responses_decode_errors.rs). This suite targets the
    // REQUEST + ERROR conformance the offline path structurally cannot reach.
    println!("NOTE: offline `response.*` decode is covered by responses_fixtures.rs; this suite is request/error conformance.");

    assert!(
        fails.is_empty(),
        "openai-chatgpt fuzz failures:\n  {}",
        fails.join("\n  ")
    );
}
