//! The terminal + error projections for the Responses stream (providers §3.4–§3.7):
//! `response.completed`/`incomplete` drain open blocks → `Usage` → `Finish`, and the
//! mid-stream error envelope maps to a `CanonicalError`. The whole-body HTTP case
//! lives in the shared `json::http_error` — status is authoritative there. Pure
//! helpers over `Value`; `super::decode` dispatches into them.

use serde_json::Value;

use crate::canonical::{CanonicalError, ErrorKind, Event, FinishReason, Usage};
use crate::protocol::json::text_of;
use crate::protocol::DecodeState;

/// `response.completed` (§3.4): drain any still-open blocks, emit `Usage` then
/// `Finish`, set `terminated`. A streamed refusal wins; else a `function_call` in
/// the output promotes to `ToolUse`; else the response status (§3.6).
pub(super) fn completed(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let response = &v["response"];
    let reason = completed_finish(response, &state.refusal);
    terminal(response, reason, state)
}

/// `response.incomplete` (§3.4): `Length` for `max_output_tokens`, else `Other` —
/// then the same drain/usage/finish/terminate as completion.
pub(super) fn incomplete(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let response = &v["response"];
    let r = response["incomplete_details"]["reason"]
        .as_str()
        .unwrap_or_default();
    let reason = if r == "max_output_tokens" {
        FinishReason::Length
    } else {
        FinishReason::Other(r.to_owned())
    };
    terminal(response, reason, state)
}

/// The shared terminal path (§3.4): drain open blocks → `ContentStop` ascending,
/// then `Usage` (if present), then `Finish`, and flip `terminated`. `decode` never
/// emits `End`; run appends the one terminator at body EOF.
fn terminal(response: &Value, reason: FinishReason, state: &mut DecodeState) -> Vec<Event> {
    let mut out = Vec::new();
    let mut open: Vec<u32> = state.open.keys().copied().collect();
    open.sort_unstable();
    for index in open {
        state.open.remove(&index);
        out.push(Event::ContentStop { index });
    }
    if let Some(u) = usage(response) {
        out.push(Event::Usage(u));
    }
    out.push(Event::Finish { reason });
    state.terminated = true;
    out
}

/// `response.completed` status → `FinishReason` (§3.6): a streamed refusal wins; a
/// `function_call` item in the output promotes to `ToolUse`; an unknown status
/// preserves verbatim via `Other`.
fn completed_finish(response: &Value, refusal: &str) -> FinishReason {
    if !refusal.is_empty() {
        return FinishReason::Refusal {
            category: "refusal".into(),
            explanation: Some(refusal.to_owned()),
        };
    }
    let has_tool = response["output"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|i| i["type"].as_str() == Some("function_call"));
    if has_tool {
        return FinishReason::ToolUse;
    }
    match response["status"].as_str() {
        None | Some("completed") => FinishReason::Stop,
        Some(other) => FinishReason::Other(other.to_owned()),
    }
}

/// `response.usage` → canonical `Usage` (§3.5), or `None` when absent. Every field
/// `Option`, never a fabricated `0`; Responses reports no cache-write.
fn usage(response: &Value) -> Option<Usage> {
    let u = response.get("usage").filter(|u| u.is_object())?;
    Some(Usage {
        input_tokens: u["input_tokens"].as_u64().map(|x| x as u32),
        output_tokens: u["output_tokens"].as_u64().map(|x| x as u32),
        cache_read_tokens: u["input_tokens_details"]["cached_tokens"]
            .as_u64()
            .map(|x| x as u32),
        cache_write_tokens: None,
    })
}

/// A mid-stream `response.failed`/`response.error` on a 2xx stream (§3.7): no
/// governing HTTP status, so `kind` decodes from the error body (CR-10), not the
/// transport. The `error` object rides `provider_detail`. Never folded into `Finish`.
pub(super) fn stream_error(v: &Value) -> CanonicalError {
    let err = if v["response"]["error"].is_object() {
        v["response"]["error"].clone()
    } else {
        v["error"].clone()
    };
    CanonicalError {
        kind: stream_error_kind(&err),
        message: text_of(&err, "message"),
        provider_detail: Some(err),
        retry_after_seconds: None,
    }
}

/// Mid-stream error body → `kind` (§3.7): OpenAI tags the failure in `code` (the
/// response-level error) or `type` — a server fault is 5xx-class (`Provider{500}`,
/// exit 70), a rate limit is `Provider{429}` (exit 69), anything else (or no tag)
/// is retryable `Transport` (exit 69). The status is NOT consulted (CR-10): a 2xx
/// stream has none.
fn stream_error_kind(err: &Value) -> ErrorKind {
    let tag = err["code"]
        .as_str()
        .or_else(|| err["type"].as_str())
        .unwrap_or_default();
    match tag {
        "server_error" => ErrorKind::Provider { status: 500 },
        "rate_limit_exceeded" | "rate_limit_error" => ErrorKind::Provider { status: 429 },
        _ => ErrorKind::Transport,
    }
}
