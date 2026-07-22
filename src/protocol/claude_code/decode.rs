//! RESPONSE projection (claude-code spec §5, §6): one stream-json NDJSON line → ≥0
//! canonical events. The whole decoder is a LINE DISPATCH: `stream_event` payloads
//! ARE Anthropic Messages SSE events, so they DELEGATE to the `anthropic_messages`
//! decoder with the same `DecodeState` (one Messages parser — the protocol-dedup
//! rule); `result` is the CLI's own terminator and error envelope; everything else
//! is envelope chatter (`system`, `rate_limit_event`, the duplicate `assistant`
//! aggregates — which on a failed run carry the classifying `error` tag, noted into
//! `DecodeState.error_tag` for the `result` fold).

use serde_json::Value;

use crate::canonical::{CanonicalError, ErrorKind, Event};
use crate::protocol::anthropic::AnthropicMessages;
use crate::protocol::json::{parse, to_json_string};
use crate::protocol::{DecodeState, Frame, Protocol};

/// Dispatch one NDJSON line on its `type` (spec §5.2). A whole-body error frame
/// (`frame.status: Some`) delegates to the shared status-driven fold — unreachable
/// from the shipped exec transport (status is always 200, spec §3.2) but reachable
/// through the seam by any embedder's transport, so it stays uniform.
pub(super) fn decode(frame: Frame, state: &mut DecodeState) -> Result<Vec<Event>, CanonicalError> {
    if frame.status.is_some() {
        return AnthropicMessages.decode(frame, state);
    }
    let v: Value = parse(&frame.data)?;
    match v["type"].as_str().unwrap_or_default() {
        // The wrapped Messages SSE event, verbatim — re-framed and DELEGATED (spec
        // §5.2). `message_stop` inside sets `state.terminated`; deltas, usage,
        // signatures and Finish all fall out of the one existing state machine.
        "stream_event" => AnthropicMessages.decode(
            Frame {
                event: None,
                data: to_json_string(&v["event"]).into_bytes(),
                status: None,
            },
            state,
        ),
        // The aggregate duplicate of the stream; on a FAILED run it carries the
        // classifying tag (`"error": "authentication_failed"`) — the fact is CARRIED
        // to the `result` fold, never re-derived from message strings (spec §6).
        "assistant" => {
            if let Some(tag) = v["error"].as_str() {
                state.error_tag = Some(tag.to_owned());
            }
            Ok(vec![])
        }
        "result" => Ok(result_events(&v, state)),
        // `system`/`rate_limit_event`/unknown: envelope chatter (the arch §3.2
        // unknown-provider-block drop).
        _ => Ok(vec![]),
    }
}

/// The CLI's terminal `result` line (spec §5.2, §6). Success after a completed
/// message adds nothing (the inner `message_stop` already terminated); success with
/// NO message stream is a malformed run (in-band `Transport` error, never a silent
/// empty exit-0); `is_error: true` folds to the canonical error. All arms terminate
/// the stream, so EOF after `result` is clean.
fn result_events(v: &Value, state: &mut DecodeState) -> Vec<Event> {
    let had_message = state.terminated;
    state.terminated = true;
    if v["is_error"].as_bool() == Some(true) {
        return vec![Event::Error(result_error(v, state))];
    }
    if had_message {
        return vec![];
    }
    vec![Event::Error(CanonicalError {
        kind: ErrorKind::Transport,
        message: "claude exited without a message stream (result with no completion)".to_owned(),
        provider_detail: Some(v.clone()),
        retry_after_seconds: None,
    })]
}

/// An `is_error` result → the canonical error (spec §6), kind derived from CARRIED
/// facts in order: the upstream API's HTTP status when the CLI relays one
/// (`api_error_status`, through the ONE shared status table); else the noted
/// `authentication_failed` tag → `Auth` (77, the logged-out capture); else
/// `Transport` (69), the response-side safe default. The whole result object rides
/// `provider_detail` verbatim; no HTTP handshake exists, so `retry_after_seconds`
/// stays `None` (never fabricated).
fn result_error(v: &Value, state: &DecodeState) -> CanonicalError {
    let kind = match v["api_error_status"].as_u64() {
        Some(status) => ErrorKind::from_http_status(status as u16),
        None if state.error_tag.as_deref() == Some("authentication_failed") => ErrorKind::Auth,
        None => ErrorKind::Transport,
    };
    let message = match v["result"].as_str() {
        Some(text) if !text.is_empty() => text.to_owned(),
        _ => "claude exited with an error".to_owned(),
    };
    CanonicalError {
        kind,
        message,
        provider_detail: Some(v.clone()),
        retry_after_seconds: None,
    }
}

/// Decode a COMPLETE drained body (the `stream:false` fold, spec §5.3): the
/// aggregate IS the stream's own NDJSON lines, so the explode→replay rule (arch
/// §3.2) degenerates to replaying each line through [`decode`]. `run`'s
/// `ensure_terminal` guard covers a verdict-less body.
pub(super) fn decode_full(
    body: &[u8],
    state: &mut DecodeState,
) -> Result<Vec<Event>, CanonicalError> {
    let mut out = Vec::new();
    for line in body.split(|b| *b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        out.extend(decode(
            Frame {
                event: None,
                data: line.to_vec(),
                status: None,
            },
            state,
        )?);
    }
    Ok(out)
}
