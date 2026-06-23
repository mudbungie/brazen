//! Live, opt-in plumbing of the `openai_responses` ENCODE circuits the fuzz suite
//! (bl-b72f) never drove (bl-f8f7). `live_fuzz_openai.rs` exercised text + a tool
//! DEFINITION/CALL; several `src/protocol/openai_responses/encode.rs` branches stayed
//! unvalidated against the live codex backend, each a candidate SILENT encode mismatch.
//! The point is to plumb BRAZEN's projection, not OpenAI: that codex accepts the exact
//! wire shape brazen emits. Circuits driven here —
//!   1. IMAGE content → `input_image` (encode::input_image): a base64 data-URI AND a
//!      URL passthrough (the `image_url` field name + `data:<mt>;base64,` format).
//!   2. tool_choice SPELLINGS (encode::tool_choice_value): `Any`→`"required"` (forces a
//!      call) and `None`→`"none"` (forbids one) — the fuzz only drove `Tool{name}`.
//!   3. Full tool-RESULT round-trip (encode::function_call_output + message_items
//!      hoisting): a prior assistant `tool_use` AND a `tool`-role `tool_result` →
//!      `function_call` + `function_call_output` keyed by `call_id`; `is_error` prefix.
//!
//! REUSES the bl-04dc/bl-b72f leaves (`live_support/{exec,grammar,openai}.rs`) via
//! `#[path]`; adds NO duplicate harness. Black-box, no lib linkage → the
//! coverage-excluded `bz/` crate; never runs in `make check`. `#[ignore]`d AND
//! `BRAZEN_LIVE`-gated; every circuit GENERATES, so the whole body is behind the
//! second opt-in `BRAZEN_LIVE_FUZZ_SPEND=1` (the fuzz suite's spend convention) and
//! prints what ran vs SKIPPED (AGENTS.md). Validated live 2026-06-17 (bl-f8f7): all
//! six circuits 200 with no mismatch — gpt-5.4 is vision-capable, accepts the
//! data-URI + `image_url` shape, both tool_choice spellings, and the fed-back
//! `function_call_output` (synthetic `call_id`, `[error]` prefix included).
//!
//! ```text
//! BRAZEN_LIVE=1 BRAZEN_LIVE_FUZZ_SPEND=1 \
//!   cargo test -p brazen --test live_encode_openai -- --ignored --nocapture
//! ```

#[allow(dead_code)] // `connectable` is unused here (keyless probe; we read a cred).
#[path = "live_support/exec.rs"]
mod exec;
#[path = "live_support/grammar.rs"]
mod grammar;
#[path = "live_support/openai.rs"]
mod openai;

use serde_json::{json, Map, Value};

use exec::{cred_file, run_bz};
use openai::{
    args, body, check_accept, error_event, fail, flag, grammar_ok, model, Shape, ERR_EXIT, PROVIDER,
};

/// A genuinely valid 8×8 solid-red PNG, base64 (a hand-built 2×2 was rejected as
/// "not a valid image" — codex DECODES the bytes, so the data must be real). The
/// base64 → `data:image/png;base64,…` data-URI is `encode::input_image`'s job.
const RED_PNG_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEklEQVR4nGP4z8CAFWEXHbQSACj/P8Fu7N9hAAAAAElFTkSuQmCC";
/// A reliable public red-square host codex's egress can fetch (Wikimedia/GitHub raw
/// blocked the fetch in probing). Drives the `ImageSource::Url` verbatim passthrough.
const IMAGE_URL: &str = "https://placehold.co/64x64/ff0000/ff0000.png";
const ONE_WORD: &str = "You are a terse assistant. Reply with exactly one word.";

/// The codex-mandated request frame (stream/store), per the fuzz suite's `valid()`.
/// `system` and the rest are layered per circuit. `store` rides the `extra` flatten.
fn base(system: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("stream".into(), json!(true));
    m.insert("store".into(), json!(false));
    m.insert("system".into(), json!([{ "type": "text", "text": system }]));
    m
}

/// The shared `get_weather` tool definition (circuits 2 & 3).
fn weather_tool() -> Value {
    json!([{ "name": "get_weather", "description": "Weather for a city",
        "input_schema": { "type": "object",
            "properties": { "city": { "type": "string" } }, "required": ["city"] } }])
}

/// Circuit 1a: a user message carrying text + a base64 `image` part → `input_image`
/// with a `data:image/png;base64,…` data-URI. gpt-5.4 must read it (answers "red").
fn image_base64() -> String {
    let mut m = base(ONE_WORD);
    m.insert(
        "messages".into(),
        json!([{ "role": "user", "content": [
            { "type": "text", "text": "What primary color fills this image? One word." },
            { "type": "image",
              "source": { "kind": "base64", "media_type": "image/png", "data": RED_PNG_B64 } },
        ] }]),
    );
    body(&m)
}

/// Circuit 1b: a URL `image` part → `input_image` with the URL passed through verbatim
/// (no data-URI wrapping). Exercises the `ImageSource::Url` branch of `encode`.
fn image_url() -> String {
    let mut m = base(ONE_WORD);
    m.insert(
        "messages".into(),
        json!([{ "role": "user", "content": [
            { "type": "text", "text": "What primary color fills this image? One word." },
            { "type": "image", "source": { "kind": "url", "url": IMAGE_URL } },
        ] }]),
    );
    body(&m)
}

/// Circuit 2a: tools + `tool_choice {type:none}` → `"none"`. The model is asked to call
/// `get_weather` but MUST NOT (none forbids it) → a plain text reply (`Shape::Text`).
fn tool_choice_none() -> String {
    let mut m = base(ONE_WORD);
    m.insert("tools".into(), weather_tool());
    m.insert(
        "messages".into(),
        json!([{ "role": "user", "content": [{ "type": "text",
            "text": "What is the weather in Paris? Call get_weather. If you cannot, reply: ok" }] }]),
    );
    m.insert("tool_choice".into(), json!({ "type": "none" }));
    body(&m)
}

/// Circuit 2b: tools + `tool_choice {type:any}` → `"required"`. FORCES a tool call →
/// the canonical `tool_use` grammar (`Shape::Tool`).
fn tool_choice_required() -> String {
    let mut m = base("You are a terse assistant.");
    m.insert("tools".into(), weather_tool());
    m.insert(
        "messages".into(),
        json!([{ "role": "user", "content": [{ "type": "text",
            "text": "What is the weather in Paris?" }] }]),
    );
    m.insert("tool_choice".into(), json!({ "type": "any" }));
    body(&m)
}

/// Circuit 3: a full tool-result round-trip — a prior assistant `tool_use` AND a
/// `tool`-role `tool_result` fed back. `message_items` hoists the call to a standalone
/// `function_call` and `function_call_output` emits the result keyed by `call_id`
/// (synthetic ids are accepted). `is_error` toggles the `[error]` textual prefix. The
/// model continues with text incorporating the result (`Shape::Text`).
fn roundtrip(is_error: bool, result_text: &str) -> String {
    let system = if is_error {
        "You are a terse assistant. If a tool errored, apologize in one short sentence."
    } else {
        "You are a terse assistant. After a tool result, state it in one short sentence."
    };
    let mut m = base(system);
    m.insert("tools".into(), weather_tool());
    m.insert(
        "messages".into(),
        json!([
            { "role": "user", "content": [{ "type": "text",
                "text": "What is the weather in Paris? Call get_weather." }] },
            { "role": "assistant", "content": [{ "type": "tool_use",
                "id": "call_brazen0001", "name": "get_weather", "input": { "city": "Paris" } }] },
            { "role": "tool", "content": [{ "type": "tool_result",
                "tool_use_id": "call_brazen0001",
                "content": [{ "type": "text", "text": result_text }], "is_error": is_error }] },
        ]),
    );
    body(&m)
}

/// The `image-url` case rides codex's egress fetching an external host. A `param:"url"`
/// download error (the host blocked the fetch) is NOT a brazen encode mismatch — the
/// `input_image` URL shape was accepted (codex reached the download step) — so it is a
/// LOGGED SKIP, not a failure (AGENTS.md). Any other non-2xx is a real failure.
fn check_image_url(label: &str, body_json: &str) -> Option<String> {
    let (code, out, err) = run_bz(&args(&model()), body_json);
    if code == 0 {
        return match grammar_ok(&out, Shape::Text) {
            Ok(()) => {
                println!("  {label:<22} ok (exit 0, canonical grammar)");
                None
            }
            Err(e) => fail(label, &e),
        };
    }
    if code == ERR_EXIT {
        if let Some(e) = error_event(&out) {
            let param = e["provider_detail"]["error"]["param"]
                .as_str()
                .unwrap_or("");
            let msg = e["message"].as_str().unwrap_or("");
            if param == "url" || msg.contains("downloading file") {
                println!("  {label:<22} SKIP: external host blocked codex fetch ({msg:?}); the input_image URL shape was accepted");
                return None;
            }
        }
    }
    fail(label, &format!("exit {code}; stderr: {}", err.trim()))
}

#[test]
#[ignore = "live: drives the codex backend over the network; run with --ignored"]
fn encode_openai_chatgpt_codex() {
    if !flag("BRAZEN_LIVE") {
        eprintln!("skipping OpenAI ChatGPT-SSO encode plumbing: set BRAZEN_LIVE=1 to run it");
        return;
    }
    if cred_file(PROVIDER).is_none() {
        eprintln!("skipping OpenAI ChatGPT-SSO encode plumbing: no stored `{PROVIDER}` cred — `bz login {PROVIDER}` first");
        return;
    }
    // Every circuit GENERATES (costs tokens), so the WHOLE set is spend-gated.
    if !flag("BRAZEN_LIVE_FUZZ_SPEND") {
        println!("OpenAI ChatGPT-SSO encode plumbing: SKIPPED all 6 token-costing circuits (set BRAZEN_LIVE_FUZZ_SPEND=1)");
        return;
    }
    let m = model();
    println!("== {PROVIDER} encode plumbing ==  model {m}");
    let mut fails: Vec<String> = Vec::new();

    // (label, shape, body) acceptance circuits driven via the shared harness.
    let cases: Vec<(&str, Shape, String)> = vec![
        ("image-base64", Shape::Text, image_base64()),
        ("tool-choice-none", Shape::Text, tool_choice_none()),
        ("tool-choice-required", Shape::Tool, tool_choice_required()),
        (
            "roundtrip-ok",
            Shape::Text,
            roundtrip(false, "18C and sunny"),
        ),
        (
            "roundtrip-error",
            Shape::Text,
            roundtrip(true, "service unavailable"),
        ),
    ];
    let n = cases.len() + 1; // + the url circuit (its own skip-aware runner)
    println!("-- encode circuits ({n} token-costing runs) --");
    for (label, shape, b) in cases {
        if let Some(f) = check_accept(label, shape, &b) {
            fails.push(f);
        }
    }
    if let Some(f) = check_image_url("image-url", &image_url()) {
        fails.push(f);
    }

    println!(
        "\n{n} encode circuit(s) exercised; {} failure(s)",
        fails.len()
    );
    assert!(
        fails.is_empty(),
        "openai-chatgpt encode failures:\n  {}",
        fails.join("\n  ")
    );
}
