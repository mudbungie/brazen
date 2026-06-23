//! Leaf JSON accessors shared by every protocol decode/encode (protocol-dedup
//! spec, D1). Pure over `&Value`/`&[u8]` with zero wire knowledge — the canonical
//! "JSON access" mechanics, the single home for what was copied per provider. The
//! synthesized-stream mechanics (`next_index`/`open_text`/`drain`) live in `synth`.

use serde_json::{Map, Value};

use crate::canonical::{CanonicalError, ErrorKind, Model};
use crate::protocol::WireRequest;

/// Project a models-list body onto the canonical ordered `Vec<Model>` (model-discovery
/// §3.1), the single home every `decode_models` shares. The dialects coincide on the
/// shape — a top-level `array_key` array of objects each carrying the wire id at
/// `id_key` — so they differ only as DATA: the two keys, and Google's `strip` of a
/// leading `models/` so the id is usable in encode's path. ORDER-PRESERVING: the
/// `Vec` index IS the provider's suggested order (§4 reads it). A body that is not
/// the expected `{array_key:[…]}` shape is a `Provider{502}` error — the list-models GET
/// drained a 2xx, so an unparseable list is an upstream contract violation, never a silent
/// empty list (§3.1). `default` is `false`: no dialect flags one today (§3).
pub(crate) fn decode_models(
    data: &[u8],
    array_key: &str,
    id_key: &str,
    strip: &str,
) -> Result<Vec<Model>, CanonicalError> {
    let v: Value = serde_json::from_slice(data).map_err(|e| models_error(&e.to_string()))?;
    let entries = v[array_key]
        .as_array()
        .ok_or_else(|| models_error(&format!("models body has no `{array_key}` array")))?;
    Ok(entries
        .iter()
        .filter_map(|e| e[id_key].as_str())
        .map(|id| Model {
            id: id.strip_prefix(strip).unwrap_or(id).to_owned(),
            default: false,
        })
        .collect())
}

/// A malformed/unexpected models-list body → `Provider{502}` (model-discovery §3.1):
/// the list-models GET drained a 2xx, so a body we cannot project is the upstream
/// returning an invalid response (Bad Gateway), retryable like any 5xx — distinct
/// from `parse`'s mid-stream `Transport`, which has no governing status.
fn models_error(detail: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Provider { status: 502 },
        message: format!("malformed models list: {detail}"),
        provider_detail: None,
    }
}

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

/// The byte-identical encoder tail every `encode` shares (protocol-dedup spec, D1):
/// serialize the assembled `body` (our own owned `Map<String,Value>` serializes
/// infallibly — the lone `expect_used` for the whole encoder layer lives here, not
/// once per dialect), wrap it in a `WireRequest` at the caller-built `url`, and fold
/// the row's `beta_headers` on verbatim. Content-type is NOT stamped here — it rides
/// via `Protocol::content_type()`, applied once in `serve` for both this path and
/// `--raw` (the single home for the dialect's media type). The only per-dialect
/// variation is how `url` is computed, so the caller builds it and hands it in.
pub(crate) fn finish_body(
    body: Map<String, Value>,
    url: String,
    beta: &[(&str, &str)],
) -> WireRequest {
    #[allow(clippy::expect_used)]
    let bytes = serde_json::to_vec(&body).expect("request body is infallibly serializable");
    let mut wire = WireRequest::new(url, bytes);
    for (k, v) in beta {
        wire.set_header(k, v);
    }
    wire
}

/// The ONE whole-body non-2xx HTTP error projection, shared by every protocol's
/// decode (bl-5fe6). The HTTP status is the authoritative fact — `kind` derives
/// from it via the single `ErrorKind::from_http_status` table — and the **RAW
/// response body rides `provider_detail` VERBATIM** so a provider error is never
/// undiagnosable, whatever envelope shape it took. We deliberately do NOT assume a
/// uniform `{"error":…}` schema: OpenAI's codex backend returns `{"detail":…}`,
/// Ollama a bare `{"error":"…"}` string, a proxy plain HTML — the bytes that
/// actually arrived are what diagnose the failure, so they are what we carry.
///
/// A JSON body rides as the parsed `Value`; a non-JSON body (proxy HTML, plain
/// text) rides as a `Value::String` of its bytes; an empty body degrades to
/// `None`. `message` is a best-effort human summary pulled from a known field
/// (`error.message`, a bare `error` string, or `detail`), else the body itself —
/// never empty when a body exists, so text mode (which shows only `message`) is
/// diagnosable too. The body is a RESPONSE — it carries no request creds, so there
/// is no secret to redact here.
pub(crate) fn http_error(data: &[u8], status: u16) -> CanonicalError {
    let kind = ErrorKind::from_http_status(status);
    let (message, provider_detail) = match serde_json::from_slice::<Value>(data) {
        Ok(body) => (error_message(&body), Some(body)),
        Err(_) => {
            // Non-JSON: surface the raw bytes verbatim rather than discard them.
            let raw = String::from_utf8_lossy(data).trim().to_owned();
            if raw.is_empty() {
                (String::new(), None)
            } else {
                (raw.clone(), Some(Value::String(raw)))
            }
        }
    };
    CanonicalError {
        kind,
        message,
        provider_detail,
    }
}

/// Best-effort human message from a parsed error body: a nested `error.message`, a
/// bare `error` string (Ollama), or a `detail` string (OpenAI codex) — else the
/// whole body re-serialized, so the message is never empty when a body parsed.
fn error_message(body: &Value) -> String {
    body["error"]["message"]
        .as_str()
        .or_else(|| body["error"].as_str())
        .or_else(|| body["detail"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| to_json_string(body))
}
