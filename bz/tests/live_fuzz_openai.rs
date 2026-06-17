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

use serde_json::{json, Map, Value};

use exec::{cred_file, run_bz};
use grammar::{delta_has, events, kind_has, last_is, ty, want};

const PROVIDER: &str = "openai-chatgpt";
const MODEL: &str = "gpt-5.4";
const MODEL_ENV: &str = "BRAZEN_LIVE_OPENAI_CHATGPT_MODEL";
/// Gated for a ChatGPT account → 400 "…not supported" (the unsupported-model case).
const UNSUPPORTED_MODEL: &str = "gpt-5-codex";
const SYSTEM: &str = "You are a terse assistant. Reply with exactly one word when asked.";
const PROMPT: &str = "reply with the single word: ok";
/// A 4xx provider status → `Unavailable` → exit 69 (canonical/error.rs §8 table).
const ERR_EXIT: i32 = 69;

/// An env flag is "on" iff set and non-empty. `BRAZEN_LIVE` is the suite gate (dual
/// of the smoke tests); `BRAZEN_LIVE_FUZZ_SPEND` is the second opt-in for the
/// TOKEN-COSTING acceptance set (the error matrix is ~free, the happy path generates).
fn flag(name: &str) -> bool {
    std::env::var(name).map(|v| !v.is_empty()).unwrap_or(false)
}

/// The model to drive: `$BRAZEN_LIVE_OPENAI_CHATGPT_MODEL` if set, else `gpt-5.4`.
fn model() -> String {
    std::env::var(MODEL_ENV)
        .ok()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| MODEL.to_owned())
}

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

fn body(m: &Map<String, Value>) -> String {
    Value::Object(m.clone()).to_string()
}

/// Argv: provider + model + `--json`. NO `--max-tokens` (codex rejects
/// `max_output_tokens`); NO `--api-key` (the OAuth2 stored cred is `bz`'s to read).
fn args(model: &str) -> Vec<String> {
    vec![
        "--provider".into(),
        PROVIDER.into(),
        "--model".into(),
        model.into(),
        "--json".into(),
    ]
}

/// The first `Event::Error` in the `--json` stream.
fn error_event(out: &str) -> Option<Value> {
    events(out).ok()?.into_iter().find(|e| ty(e) == "error")
}

/// The carried provider status (`kind.provider.status`) of an error event, as text.
fn status_of(e: &Value) -> String {
    e["kind"]["provider"]["status"]
        .as_u64()
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{:?}", e["kind"]))
}

/// Print + return a provider-qualified failure (shared by both case kinds).
fn fail(label: &str, m: &str) -> Option<String> {
    println!("  {label:<22} FAIL: {m}");
    Some(format!("{PROVIDER}/{label}: {m}"))
}

/// One error case: a body the codex backend MUST 400, plus a phrase its surfaced
/// message MUST contain. Asserts the FULL chain — exit 69 (4xx → Unavailable) AND the
/// decoded provider message matching the service's live wording. The codex backend
/// answers `{"detail":"…"}`; bl-5fe6 (landed) carries that body into the
/// `CanonicalError`, so an empty message here is a real regression. `None` = green.
fn check_error(label: &str, model: &str, body_json: &str, phrase: &str) -> Option<String> {
    let (code, out, err) = run_bz(&args(model), body_json);
    if code != ERR_EXIT {
        let m = format!("exit {code} (want {ERR_EXIT}); {}", err.trim());
        return fail(label, &m);
    }
    let Some(e) = error_event(&out) else {
        return fail(label, "exit 69 but no Error event in the stream");
    };
    let msg = e["message"].as_str().unwrap_or("");
    if !msg.contains(phrase) {
        let m = format!("message {msg:?} lacks {phrase:?} (bl-5fe6?)");
        return fail(label, &m);
    }
    println!("  {label:<22} ok (status {}; {msg:?})", status_of(&e));
    None
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
    vec![
        ("unicode-content", false, body(&uni)),
        ("multiturn-order", false, body(&multi)),
        ("tool-required", true, body(&tool)),
    ]
}

/// The canonical event grammar a 2xx codex stream MUST decode to: a `message_start`,
/// the kind-appropriate `content_start` + first delta, a `finish`, and `end` last.
fn grammar_ok(out: &str, is_tool: bool) -> Result<(), String> {
    let evs = events(out)?;
    want(&evs, "message_start", |e| ty(e) == "message_start")?;
    if is_tool {
        want(&evs, "tool_use content_start", |e| kind_has(e, "tool_use"))?;
        want(&evs, "json_delta", |e| delta_has(e, "json_delta"))?;
    } else {
        want(&evs, "text content_start", |e| kind_has(e, "text"))?;
        want(&evs, "text_delta", |e| delta_has(e, "text_delta"))?;
    }
    want(&evs, "finish", |e| ty(e) == "finish")?;
    last_is(&evs, "end")
}

/// One acceptance case: exit 0 + the canonical grammar. `None` = green.
fn check_accept(label: &str, is_tool: bool, body_json: &str) -> Option<String> {
    let (code, out, err) = run_bz(&args(&model()), body_json);
    if code != 0 {
        return fail(
            label,
            &format!("exit {code} (want 0); stderr: {}", err.trim()),
        );
    }
    match grammar_ok(&out, is_tool) {
        Ok(()) => {
            println!("  {label:<22} ok (exit 0, canonical grammar)");
            None
        }
        Err(e) => fail(label, &e),
    }
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
    let (mut no_system, mut no_store, mut sfalse) = (valid(), valid(), valid());
    no_system.remove("system");
    no_store.remove("store");
    sfalse.insert("stream".into(), json!(false));
    let (bi, bs, bf, bv) = (
        body(&no_system),
        body(&no_store),
        body(&sfalse),
        body(&valid()),
    );
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
        (
            "stream-false",
            m.as_str(),
            &bf,
            "Stream must be set to true",
        ),
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
