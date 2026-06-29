//! Live, opt-in FUZZ of the OpenAI "Sign in with ChatGPT" (codex backend)
//! integration (bl-b72f). Where `live_conformance.rs` (bl-04dc) asserts the ONE
//! canonical happy path, this drives a WIDE range of request shapes at the live
//! `openai-chatgpt` provider — the codex backend's hard request preconditions and
//! well-formed variations — asserting brazen's error-mapping / normalization matches
//! what the service actually does. It REUSES the bl-04dc harness leaves verbatim
//! (`live_support/exec.rs`, `…/grammar.rs`) via `#[path]`. Black-box, no lib linkage →
//! the coverage-excluded `bz/` crate; never runs in `make check`.
//!
//! `#[ignore]`d AND `BRAZEN_LIVE`-gated; SKIPS (printed) without a `bz --login --provider
//! openai-chatgpt` cred. The error matrix is ~free (400s before generation); the
//! acceptance set GENERATES, so it is behind a SECOND opt-in (`BRAZEN_LIVE_FUZZ_SPEND=1`)
//! and prints what ran vs capped (AGENTS.md). Validated live 2026-06-16 (auth §10.7).
//!
//! ```text
//! BRAZEN_LIVE=1 BRAZEN_LIVE_FUZZ_SPEND=1 \
//!   cargo test -p brazen --test live_fuzz_openai -- --ignored --nocapture
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
use openai::{body, check_accept, check_error, flag, model, Shape, PROVIDER};

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
/// shape, body)`. Each GENERATES (costs tokens), so the set is `spend`-gated.
fn accept_cases() -> Vec<(&'static str, Shape, String)> {
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
    // No `stream:false` case here: this harness probes codex's PARAM handling, not
    // its stream mandate. brazen now HONORS `stream:false` (bl-24c2): serve carries
    // the resolved intent to `drive`, which folds the non-stream 2xx body whole via
    // `decode_full` — covered offline/deterministically (run_stream.rs
    // `an_explicit_stream_false_request_reaches_the_wire` + the per-protocol
    // `decode_full` fixtures). codex itself may still 400 on `stream:false` (an honest
    // surfaced provider error, not a brazen bug); probing that live is out of scope here.
    //
    // Unsupported sampling/length params — `temperature`/`top_p`/`max_tokens`. The
    // codex backend 400s `{"detail":"Unsupported parameter: <field>"}` on each, but
    // brazen's canonical path STRIPS all three before encode (config §4.1.1, the
    // `unsupported_body_keys` row datum, bl-d54a) — AFTER `fill_absent`, so even these
    // EXPLICIT request values are cleared. The request the service sees carries none,
    // so it is a normal 200 completion. This is the LIVE acceptance side of the offline
    // `tests/config_strip.rs` (bl-2869): the strip held against the real backend, not
    // just the encoder. Like the stream force above, the strip is a canonical-path
    // operation — it MUST stay on `--json`: `--raw` bypasses encode, so the strip would
    // NOT run and codex WOULD 400. Do not "fix" this case to `--raw`.
    let mut strip = valid();
    strip.insert("max_tokens".into(), json!(64));
    strip.insert("temperature".into(), json!(0.5));
    strip.insert("top_p".into(), json!(0.9));
    // Reasoning: `reasoning:{effort,summary}` rides the request `extra` flatten (like
    // `store`); WITHOUT it codex emits no reasoning at all. For this backend only the
    // SUMMARY channel fires — `response.reasoning_summary_text.delta` → a `thinking`
    // block with `thinking_delta`s, THEN the text answer (decoder verified live
    // 2026-06-17, bl-f308; the raw `reasoning_text` channel bl-7e50 was NOT observed).
    // The summary is the model's DISCRETION: a trivial prompt opens the thinking block
    // but may emit ZERO summary delta (seen live), so the case uses the classic
    // "missing dollar" riddle at high effort, which reliably triggers one (3/3 live).
    let mut reason = valid();
    reason.insert(
        "reasoning".into(),
        json!({ "effort": "high", "summary": "detailed" }),
    );
    reason.insert(
        "system".into(),
        json!([{ "type": "text", "text": "You are a careful problem solver." }]),
    );
    reason.insert(
        "messages".into(),
        json!([{ "role": "user", "content": [{ "type": "text", "text":
            "Three guests pay $10 each for a $30 room. The clerk refunds $5 via a \
             bellhop who pockets $2 and returns $1 to each guest. Now $9*3=$27 plus \
             $2 is $29. Where is the missing dollar? Explain the accounting carefully." }] }]),
    );
    vec![
        ("unicode-content", Shape::Text, body(&uni)),
        ("multiturn-order", Shape::Text, body(&multi)),
        ("tool-required", Shape::Tool, body(&tool)),
        ("strip-unsupported-params", Shape::Text, body(&strip)),
        ("reasoning-summary", Shape::Reasoning, body(&reason)),
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
        eprintln!("skipping OpenAI ChatGPT-SSO fuzz: no stored `{PROVIDER}` cred — `bz --login --provider {PROVIDER}` first");
        return;
    }
    let m = model();
    println!("== {PROVIDER} fuzz ==  model {m}");
    let mut fails: Vec<String> = Vec::new();

    // 1) Error-conformance matrix (auth §10.7, validated live): each is the valid
    //    body MINUS one codex-required field → a specific 400 → exit 69, whose
    //    surfaced message must carry the service's wording. Near-free (no generation).
    //    DRIFT POLICY: each row is a live tripwire on a codex mandate REACHABLE
    //    through the wire — `missing-instructions` (instructions <- system) and
    //    `missing-store` (store rides `extra`) both pass their flipped key through
    //    to the wire, so a 400->200 drift here is genuinely codex's. If one starts
    //    returning 200, MOVE it to the acceptance set (assert exit 0 + canonical
    //    grammar), NOT delete it — the suite still guards a silent re-imposition.
    //    Both STILL 400 (re-verified live 2026-06-17); the assertion IS the detector.
    //    `stream` is NOT in this matrix: serve forces `stream:true` (serve.rs:112,
    //    bl-9e3d) so this path structurally cannot put `stream:false` on the wire —
    //    codex's stream mandate is unverifiable here (see the NB below).
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
        // NB: `stream:false` was a codex mandate ("Stream must be set to true")
        // recorded on-wire at bl-b72f. It is NOT a case here, in either set: serve
        // forces `stream:true` (serve.rs:112, bl-9e3d, landed AFTER bl-b72f) so the
        // canonical path this harness drives can never send `stream:false` — the 200
        // bl-cc84 read as "codex dropped the mandate" was just the force at work, not
        // codex (bl-22d5). Codex's current mandate is unverified via this path; to
        // probe it, drive `--raw` with a hand-built `stream:false` body.
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
        for (label, shape, b) in accepts {
            if let Some(f) = check_accept(label, shape, &b) {
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
