//! Canonical event-grammar probes for the live suite (bl-04dc): parse the
//! `--json` NDJSON stream and assert the normalized event shapes — the surface
//! that is identical across providers (the whole point of brazen).

use serde_json::Value;

/// Parse the NDJSON event stream into one `Value` per non-empty line.
pub fn events(out: &str) -> Result<Vec<Value>, String> {
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| format!("non-JSON event line `{l}`: {e}")))
        .collect()
}

/// The `"type"` tag of an event (or `""`).
pub fn ty(e: &Value) -> &str {
    e.get("type").and_then(Value::as_str).unwrap_or("")
}

/// A `content_start` whose `kind` carries the given key (`text` / `tool_use`).
pub fn kind_has(e: &Value, key: &str) -> bool {
    ty(e) == "content_start" && e.get("kind").and_then(|k| k.get(key)).is_some()
}

/// A `content_delta` whose `delta` carries the given key (`text_delta`/`json_delta`).
pub fn delta_has(e: &Value, key: &str) -> bool {
    ty(e) == "content_delta" && e.get("delta").and_then(|d| d.get(key)).is_some()
}

/// Require at least one event satisfying `pred`.
pub fn want(evs: &[Value], what: &str, pred: impl Fn(&Value) -> bool) -> Result<(), String> {
    if evs.iter().any(pred) {
        Ok(())
    } else {
        Err(format!("missing `{what}` in the canonical stream"))
    }
}

/// Require the LAST event to be `{"type": t}` (the terminal marker).
pub fn last_is(evs: &[Value], t: &str) -> Result<(), String> {
    match evs.last() {
        Some(e) if ty(e) == t => Ok(()),
        _ => Err(format!("stream did not terminate in `{t}`")),
    }
}
