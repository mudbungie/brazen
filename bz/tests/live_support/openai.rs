//! Shared acceptance/error harness for the live `openai-chatgpt` (codex backend)
//! suites — the generic request-driving leaf both `live_fuzz_openai.rs` (bl-b72f,
//! request/error matrix) and `live_encode_openai.rs` (bl-f8f7, encode circuits)
//! build on. The bl-04dc leaves (`exec.rs`, `grammar.rs`) carry the impure edges
//! and event grammar; this carries the openai-chatgpt-specific argv, gate flags,
//! and the exit-0 + canonical-grammar / exit-69 + surfaced-message assertions.
//! Single home so a second openai acceptance suite reuses, never re-spells, them.
//!
//! Depends on the sibling `exec`/`grammar` modules each test crate includes under
//! those names (`crate::exec`, `crate::grammar`) — the leaf-composition convention.

// Each consumer drives a different subset (the fuzz suite uses the error matrix,
// the encode suite the acceptance path), so some helpers are dead in either alone.
#![allow(dead_code)]

use serde_json::{Map, Value};

use crate::exec::run_bz;
use crate::grammar::{delta_has, events, kind_has, last_is, ty, want};

pub const PROVIDER: &str = "openai-chatgpt";
pub const MODEL: &str = "gpt-5.4";
pub const MODEL_ENV: &str = "BRAZEN_LIVE_OPENAI_CHATGPT_MODEL";
/// A 4xx provider status → `Unavailable` → exit 69 (canonical/error.rs §8 table).
pub const ERR_EXIT: i32 = 69;

/// An env flag is "on" iff set and non-empty. `BRAZEN_LIVE` is the suite gate;
/// `BRAZEN_LIVE_FUZZ_SPEND` is the second opt-in for the TOKEN-COSTING sets.
pub fn flag(name: &str) -> bool {
    std::env::var(name).map(|v| !v.is_empty()).unwrap_or(false)
}

/// The model to drive: `$BRAZEN_LIVE_OPENAI_CHATGPT_MODEL` if set, else `gpt-5.4`.
pub fn model() -> String {
    std::env::var(MODEL_ENV)
        .ok()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| MODEL.to_owned())
}

/// Argv: provider + model + `--json`. NO `--max-tokens` (codex rejects
/// `max_output_tokens`); NO `--api-key` (the OAuth2 stored cred is `bz`'s to read).
pub fn args(model: &str) -> Vec<String> {
    vec![
        "--provider".into(),
        PROVIDER.into(),
        "--model".into(),
        model.into(),
        "--json".into(),
    ]
}

/// A request map → its compact JSON body (the stdin `bz` reads).
pub fn body(m: &Map<String, Value>) -> String {
    Value::Object(m.clone()).to_string()
}

/// Print + return a provider-qualified failure (shared by every case kind).
pub fn fail(label: &str, m: &str) -> Option<String> {
    println!("  {label:<22} FAIL: {m}");
    Some(format!("{PROVIDER}/{label}: {m}"))
}

/// The first `Event::Error` in the `--json` stream.
pub fn error_event(out: &str) -> Option<Value> {
    events(out).ok()?.into_iter().find(|e| ty(e) == "error")
}

/// The carried provider status (`kind.provider.status`) of an error event, as text.
pub fn status_of(e: &Value) -> String {
    e["kind"]["provider"]["status"]
        .as_u64()
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{:?}", e["kind"]))
}

/// The canonical event grammar a 2xx codex stream MUST decode to: a `message_start`,
/// the kind-appropriate `content_start` + first delta, a `finish`, and `end` last.
pub fn grammar_ok(out: &str, is_tool: bool) -> Result<(), String> {
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
pub fn check_accept(label: &str, is_tool: bool, body_json: &str) -> Option<String> {
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

/// One error case: a body the codex backend MUST 400, plus a phrase its surfaced
/// message MUST contain. Asserts the FULL chain — exit 69 (4xx → Unavailable) AND
/// the decoded provider message matching the service's live wording (bl-5fe6).
pub fn check_error(label: &str, model: &str, body_json: &str, phrase: &str) -> Option<String> {
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
