//! Leaf JSON accessors shared by every protocol decode/encode (protocol-dedup
//! spec, D1). Pure over `&Value`/`&[u8]` with zero wire knowledge — the canonical
//! "JSON access" mechanics, the single home for what was copied per provider. The
//! synthesized-stream mechanics (`next_index`/`open_text`/`drain`) live in `synth`.

use serde_json::Value;

use crate::canonical::{CanonicalError, ErrorKind};

/// Parse a frame's bytes as JSON; a malformed body surfaces as a `Transport`
/// error, never a panic (the wire never crashes us).
pub(crate) fn parse(data: &[u8]) -> Result<Value, CanonicalError> {
    serde_json::from_slice(data).map_err(|e| CanonicalError {
        kind: ErrorKind::Transport,
        message: e.to_string(),
        provider_detail: None,
    })
}

/// A string field, or `""` when absent/non-string (the wire never panics us).
pub(crate) fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

/// A non-empty string at `v`, else `None` — collapses null / absent / `""` so a
/// role-only chunk and a stray empty fragment open no block.
pub(crate) fn nonempty(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.is_empty())
}

/// A `u32` wire index field, or `0` when absent — the wire never panics us.
pub(crate) fn u32_at(v: &Value, key: &str) -> u32 {
    v[key].as_u64().unwrap_or(0) as u32
}

/// A `Value` → its JSON-encoded **string** (for a tool-call `arguments` slot, or a
/// single `JsonDelta` fragment): re-serialization of a `serde_json::Value` is
/// infallible.
pub(crate) fn to_json_string(v: &Value) -> String {
    #[allow(clippy::expect_used)]
    serde_json::to_string(v).expect("a serde_json::Value re-serializes infallibly")
}
