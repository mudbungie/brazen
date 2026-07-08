//! Token-count REQUEST projection (providers §10.1): canonical →
//! `POST {base_url}/v1beta/models/{model}:countTokens`. Google's `countTokens` accepts a
//! bare `contents[]` (which would UNDERCOUNT — no `systemInstruction`/`tools`) OR a
//! `generateContentRequest` envelope wrapping a full `GenerateContentRequest`. To count
//! the whole request faithfully, `count` reuses `super::body_map` (the SAME assembly
//! `encode` uses), injects the required `model` the URL path omits, and wraps it — the
//! one per-dialect count asymmetry, behind the shared `Protocol::count_tokens` seam.

use serde_json::{json, Map, Value};

use crate::canonical::{CanonicalError, CanonicalRequest};
use crate::protocol::json::finish_body;
use crate::protocol::{ProviderCtx, WireRequest};

/// Build the count request (§10.1): `{"generateContentRequest": <the generateContent
/// body + `model`>}` targeting `:countTokens`. `generationConfig` (a valid
/// `GenerateContentRequest` member) rides along — it does not affect the input-token
/// total. Response is `{"totalTokens": N}` (key read by the count runner).
pub(crate) fn count(
    req: &CanonicalRequest,
    ctx: &ProviderCtx,
) -> Result<WireRequest, CanonicalError> {
    let mut inner = super::body_map(req)?;
    // The model rides the URL path for `generateContent`, but a `GenerateContentRequest`
    // REQUIRES it in-body — inject `models/{model}`, matching the `:countTokens` path.
    inner.insert("model".into(), json!(format!("models/{}", ctx.model)));
    let mut body = Map::new();
    body.insert("generateContentRequest".into(), Value::Object(inner));
    let url = format!("{}/v1beta/models/{}:countTokens", ctx.base_url, ctx.model);
    Ok(finish_body(body, url))
}
