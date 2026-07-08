//! Response → canonical events (arch §4.4, §5.6): peek the status, frame the body,
//! decode each frame to events — as a LAZY `Iterator<Item = Event>`, the typed half of
//! [`generate`](super::generate). The non-2xx whole-body path (sse §9) and the
//! non-stream 2xx fold drain a complete body (eager); only the streaming 2xx path is
//! incremental, and it is the one that must not collect — so it is a hand-rolled
//! iterator pulling one transport chunk per step. `decode` stays the sole home of
//! provider-error parsing. The trailing `End` is appended by `generate`, not here.
//! The raw passthrough does NOT come here (it never decodes); it streams bytes to the
//! sink in [`serve_raw`](super::serve::serve_raw), so this module is canonical-only.

mod stream;

use crate::canonical::{parse_retry_after, CanonicalError, ErrorKind, Event, ExitClass};
use crate::pipeline::Sink;
use crate::protocol::{DecodeState, Frame, Protocol};
use crate::transport::TransportResponse;

/// The canonical response as a lazy event stream (no trailing `End` — `generate`
/// appends it). The exit is NOT computed here: every governing HTTP status is carried
/// into an `Event::Error` (a non-2xx body always decodes to one, §4.3), so folding the
/// stream's errors last-error-wins — what [`pump`](crate::pipeline::pump) does — yields
/// the same exit the old status-seed did. `streamed` (serve §3.2) routes the 2xx body:
/// a `!streamed` 2xx is one aggregate JSON folded whole via `decode_full`; a streamed
/// 2xx is framed incrementally. `hint` is the §5.3 provenance note appended to a 404;
/// `now` (unix seconds, the `Clock` seam) parses any `Retry-After` header on the
/// non-2xx path to `retry_after_seconds` (§3.3), stamped onto the whole-body error.
pub(super) fn response_events(
    proto: &'static dyn Protocol,
    resp: TransportResponse,
    streamed: bool,
    hint: Option<String>,
    now: u64,
) -> Box<dyn Iterator<Item = Event>> {
    let status = resp.status;
    let mut state = DecodeState::default();
    if !is_2xx(status) {
        // The §4.3 authoritative-status path: the whole body is ONE error frame
        // carrying the status; `decode` parses the envelope, the 404 hint is appended.
        // The `Retry-After` header — a response-level fact the body never held — is
        // parsed and stamped onto the error, the sibling of the 404-hint enrichment.
        let retry_after = resp
            .retry_after
            .as_deref()
            .and_then(|h| parse_retry_after(h, now));
        let events = match super::drain(resp.body) {
            Ok(data) => {
                let frame = Frame {
                    event: None,
                    data,
                    status: Some(status),
                };
                with_hint(proto.decode(frame, &mut state), hint.as_deref())
            }
            Err(_) => vec![Event::Error(transport_err(
                "failed to read error response body",
            ))],
        };
        Box::new(stamp_retry_after(events, retry_after).into_iter())
    } else if !streamed {
        // The non-stream 2xx fold (sse §9): one COMPLETE aggregate JSON, drained whole
        // and exploded back into the streamed event sequence by `decode_full`.
        let events = match super::drain(resp.body) {
            Ok(data) => flatten(proto.decode_full(&data, &mut state)),
            Err(_) => vec![Event::Error(transport_err("failed to read response body"))],
        };
        Box::new(events.into_iter())
    } else {
        Box::new(stream::StreamEvents::new(proto, resp.body))
    }
}

/// Append the §5.3 provenance `hint` to the decoded 404 error's message — the one place
/// the cache-provenance next-move joins the provider's own diagnostic. The §4.3 status
/// frame decodes to `Ok(vec![Event::Error(…)])`, so the hint maps over the events; a
/// `None` hint (non-404) returns them untouched. A decode `Err` becomes the in-band
/// error event. The exit (from `kind`) is unchanged by the message.
fn with_hint(result: Result<Vec<Event>, CanonicalError>, hint: Option<&str>) -> Vec<Event> {
    let events = flatten(result);
    match hint {
        Some(h) => events.into_iter().map(|ev| append_hint(ev, h)).collect(),
        None => events,
    }
}

/// Append `; <hint>` to an `Event::Error`'s message (§5.3), leaving `kind`/
/// `provider_detail` — and so the exit — untouched. A non-error event passes through
/// (the general path; the §4.3 status frame yields only the error).
fn append_hint(ev: Event, hint: &str) -> Event {
    match ev {
        Event::Error(mut e) => {
            e.message = format!("{}; {hint}", e.message);
            Event::Error(e)
        }
        other => other,
    }
}

/// Stamp the transport-level `retry_after_seconds` (from the `Retry-After` response
/// header, §3.3) onto every `Event::Error` in the whole-body non-2xx path — the
/// sibling of [`append_hint`], carrying a fact the parsed body never held (the same
/// carry-the-fact rule as `frame.status`, CR-10). `None` (no header / unparseable)
/// leaves the events untouched; a non-error event passes through (the general map,
/// though the non-2xx path yields only the error). It never OVERWRITES with `None`:
/// decode's own kind→body errors keep their absent field.
fn stamp_retry_after(events: Vec<Event>, secs: Option<u32>) -> Vec<Event> {
    match secs {
        None => events,
        Some(_) => events
            .into_iter()
            .map(|ev| match ev {
                Event::Error(mut e) => {
                    e.retry_after_seconds = secs;
                    Event::Error(e)
                }
                other => other,
            })
            .collect(),
    }
}

/// A `decode` result flattened to a plain event list: the events, or the decode error
/// as a single in-band `Event::Error` (§8) — the boundary where a parse failure becomes
/// a representable event rather than a `Result`.
fn flatten(result: Result<Vec<Event>, CanonicalError>) -> Vec<Event> {
    result.unwrap_or_else(|e| vec![Event::Error(e)])
}

/// Write one event to the sink, computing the exit on an `Event::Error`
/// (last-error-wins, §8). Used by the raw passthrough and the pre-stream fatals, which
/// push to the sink directly rather than through [`pump`](crate::pipeline::pump). A
/// sink write failure ends the run with the mapped exit (`BrokenPipe` → 141, the Windows
/// SIGPIPE path; else 69) — returned as `Err`.
pub(super) fn write_event(sink: &mut dyn Sink, ev: Event, exit: &mut u8) -> Result<(), u8> {
    if let Event::Error(e) = &ev {
        *exit = e.exit_code();
    }
    sink.write(&ev).map_err(|io| ExitClass::from_io(&io).code())
}

/// Emit a pre-stream `CanonicalError` in-band, then the one `End`, returning the exit
/// (§8): the fatal path once the sink exists but before any events — a parse/config
/// failure on the canonical path, or a request-half failure on the raw path. Under
/// `--raw` the sink drops the error line; the exit still carries it (§5.4).
pub(super) fn fail_inband(sink: &mut dyn Sink, err: CanonicalError) -> u8 {
    let mut exit = err.exit_code();
    match write_event(sink, Event::Error(err), &mut exit)
        .and_then(|()| write_event(sink, Event::End, &mut exit))
    {
        Ok(()) => exit,
        Err(code) => code,
    }
}

/// A `Transport`-kind error (§8 → exit 69): a transport drop or premature EOF.
pub(super) fn transport_err(message: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: message.to_owned(),
        provider_detail: None,
        retry_after_seconds: None,
    }
}

/// Is this a 2xx status? The one place the success/error split is named — `models`
/// reads the same boundary for its GET, and the raw path for its exit seed.
pub(super) fn is_2xx(status: u16) -> bool {
    (200..300).contains(&status)
}

/// The exit from a peeked HTTP status (§8): 2xx → 0, else the status-driven class
/// (401/403 → 77, other 4xx → 69, 5xx → 70). The `--raw` path's exit SEED (its final
/// word bar a transport drop, §5.4); the canonical path re-derives the same code from
/// the in-band error the same status decodes to, so it needs no seed.
pub(super) fn exit_from_status(status: u16) -> u8 {
    if is_2xx(status) {
        ExitClass::Ok.code()
    } else {
        ExitClass::from_kind(ErrorKind::from_http_status(status)).code()
    }
}

#[cfg(test)]
mod tests {
    use super::{append_hint, stamp_retry_after, with_hint};
    use crate::canonical::{CanonicalError, ErrorKind, Event};

    fn err(msg: &str) -> CanonicalError {
        CanonicalError {
            kind: ErrorKind::Provider { status: 404 },
            message: msg.to_owned(),
            provider_detail: None,
            retry_after_seconds: None,
        }
    }

    /// An event's message, or `<non-error>` — a probe that names a value (no `panic!`
    /// arm, which would be an uncovered line in this measured `src/` module).
    fn msg(ev: Event) -> String {
        match ev {
            Event::Error(e) => e.message,
            _ => "<non-error>".to_owned(),
        }
    }

    /// The §5.3 enrichment, all arms: `append_hint` enriches an `Event::Error` and
    /// passes any other event through; `with_hint` is a no-op with `None` and maps the
    /// error message with `Some` — and folds a decode `Err` to one in-band error event.
    #[test]
    fn the_hint_enriches_only_the_error_and_no_ops_otherwise() {
        assert_eq!(msg(append_hint(Event::Error(err("boom")), "x")), "boom; x");
        assert_eq!(msg(append_hint(Event::End, "x")), "<non-error>");
        let kept = with_hint(Ok(vec![Event::Error(err("a"))]), None);
        assert_eq!(kept.into_iter().map(msg).collect::<Vec<_>>(), ["a"]);
        let hinted = with_hint(Ok(vec![Event::Error(err("a"))]), Some("x"));
        assert_eq!(hinted.into_iter().map(msg).collect::<Vec<_>>(), ["a; x"]);
        let folded = with_hint(Err(err("y")), Some("x"));
        assert_eq!(folded.into_iter().map(msg).collect::<Vec<_>>(), ["y; x"]);
    }

    /// The §3.3 retry-after stamp, all arms: `Some` sets the field on every
    /// `Event::Error` and passes any non-error event through; `None` is a no-op that
    /// leaves the error's absent field absent (never overwriting the body-derived one).
    #[test]
    fn the_retry_after_stamps_only_errors_and_no_ops_on_none() {
        let retry = |ev: &Event| match ev {
            Event::Error(e) => e.retry_after_seconds,
            _ => None,
        };
        let stamped = stamp_retry_after(vec![Event::Error(err("boom")), Event::End], Some(30));
        assert_eq!(retry(&stamped[0]), Some(30));
        assert_eq!(retry(&stamped[1]), None); // the non-error End passes through untouched
        let untouched = stamp_retry_after(vec![Event::Error(err("boom"))], None);
        assert_eq!(retry(&untouched[0]), None);
    }
}
