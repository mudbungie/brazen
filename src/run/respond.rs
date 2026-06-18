//! Response driving (arch §4.4, §5.6): peek the status for the exit code, frame
//! the body, decode each frame to canonical events, and project them through the
//! sink — computing the exit last-error-wins (§8). The framing decision is the one
//! place the non-2xx whole-body path (sse §9) and the `--raw` Identity passthrough
//! diverge from the normalized SSE/NDJSON stream; `decode` stays the sole home of
//! provider-error parsing. `run` owns the single trailing `End`.

use std::io;

use crate::canonical::{CanonicalError, ErrorKind, Event, ExitClass};
use crate::pipeline::Sink;
use crate::protocol::{DecodeState, Frame, Framing, Protocol};
use crate::transport::{Bytes, TransportResponse};

/// Stream the response into the sink and return the exit code. The exit starts
/// from the peeked HTTP status and is overridden by any in-band error; a decoded
/// terminal marker suppresses the premature-EOF injection (CR-9). `streamed` is the
/// wire's streaming intent (serve §3.2), CARRIED so the 2xx fold matches the body
/// shape the request asked for rather than guessing it back: a `!streamed` 2xx body
/// is one aggregate JSON the framers cannot cut, folded whole via `decode_full`.
/// `model_hint` is the §5.3 provenance hint (the model + whether it came from the
/// cache); CARRIED here so a 404 on the GENERATION request enriches its message with
/// the caller's next move (stale cache vs. cold cache/typo), the one status where the
/// resolved model is the likely cause — appended to whatever the provider's body says.
pub(super) fn drive(
    sink: &mut dyn Sink,
    raw: bool,
    streamed: bool,
    proto: &dyn Protocol,
    resp: TransportResponse,
    model_hint: Option<&str>,
) -> u8 {
    let status = resp.status;
    let mut exit = exit_from_status(status);
    let mut state = DecodeState::default();
    // The hint rides only a 404 — the status that means "no such model"; `--raw` keeps
    // the provider's bytes verbatim (no normalized error to enrich), and the model is
    // never read there anyway, so it is never hinted.
    let hint = (status == 404 && !raw).then_some(model_hint).flatten();
    let outcome = if !is_2xx(status) && !raw {
        whole_body(sink, proto, status, resp.body, &mut state, &mut exit, hint)
    } else if is_2xx(status) && !raw && !streamed {
        whole_body_success(sink, proto, resp.body, &mut state, &mut exit)
    } else {
        stream(sink, raw, proto, resp.body, &mut state, &mut exit)
    };
    match outcome.and_then(|()| write_event(sink, Event::End, &mut exit)) {
        Ok(()) => exit,
        Err(code) => code,
    }
}

/// The non-stream 2xx fold (sse §9): the body is one COMPLETE aggregate JSON, so it
/// is drained whole and handed to the protocol's `decode_full` (the explode→replay
/// reconstruction of the streamed event sequence). No premature-EOF check — the body
/// is complete, never a cut stream — and no framing: the single JSON object is not a
/// frame grammar. A mid-collection transport drop is the same in-band `Transport`
/// error the error path surfaces.
fn whole_body_success(
    sink: &mut dyn Sink,
    proto: &dyn Protocol,
    body: Box<dyn Iterator<Item = io::Result<Bytes>>>,
    state: &mut DecodeState,
    exit: &mut u8,
) -> Result<(), u8> {
    match super::drain(body) {
        Ok(data) => emit(sink, proto.decode_full(&data, state), exit),
        Err(_) => {
            let err = Event::Error(transport_err("failed to read response body"));
            write_event(sink, err, exit)
        }
    }
}

/// The non-2xx normalized path (sse §9): collect the whole body as one error frame
/// carrying the authoritative status, and let `decode` parse the envelope. A `hint`
/// (a 404 on the generation request, §5.3) is appended to the decoded error message
/// so the provider's diagnostic AND the cache-provenance next-move both reach the
/// caller; the exit is unchanged (69).
fn whole_body(
    sink: &mut dyn Sink,
    proto: &dyn Protocol,
    status: u16,
    body: Box<dyn Iterator<Item = io::Result<Bytes>>>,
    state: &mut DecodeState,
    exit: &mut u8,
    hint: Option<&str>,
) -> Result<(), u8> {
    match super::drain(body) {
        Ok(data) => {
            let frame = Frame {
                event: None,
                data,
                status: Some(status),
            };
            emit(
                sink,
                with_hint(decode_one(false, proto, frame, state), hint),
                exit,
            )
        }
        Err(_) => {
            let err = Event::Error(transport_err("failed to read error response body"));
            write_event(sink, err, exit)
        }
    }
}

/// Append the §5.3 provenance `hint` to the decoded 404 error's message — the one
/// place the cache-provenance next-move joins the provider's own diagnostic. A whole-
/// body status frame decodes to `Ok(vec![Event::Error(…)])` (the authoritative-status
/// path, §4.3), so the hint maps over the events, enriching each `Event::Error`. A
/// `None` hint (non-404 or `--raw`) is the empty case — `result` is returned untouched.
/// The exit is computed from `kind`, unchanged by the message.
fn with_hint(
    result: Result<Vec<Event>, CanonicalError>,
    hint: Option<&str>,
) -> Result<Vec<Event>, CanonicalError> {
    let Some(h) = hint else { return result };
    result.map(|events| events.into_iter().map(|ev| append_hint(ev, h)).collect())
}

/// Append `; <hint>` to an `Event::Error`'s message (the §5.3 enrichment), leaving
/// `kind`/`provider_detail` — and so the exit — untouched. A non-error event is the
/// empty case, returned as-is (the general path, not a special branch) — the §4.3
/// status frame yields only the error, so the pass-through is exercised in the unit
/// test below.
fn append_hint(ev: Event, hint: &str) -> Event {
    match ev {
        Event::Error(mut e) => {
            e.message = format!("{}; {hint}", e.message);
            Event::Error(e)
        }
        other => other,
    }
}

/// The streaming path (§4.4): drive chunks through the framer and `decode`. A
/// mid-stream transport drop is an in-band `Transport` error that stops the loop;
/// otherwise `finish` flushes a trailing unterminated frame and — if the provider
/// terminal marker was never seen (normalized only) — a premature-EOF error fires.
fn stream(
    sink: &mut dyn Sink,
    raw: bool,
    proto: &dyn Protocol,
    body: Box<dyn Iterator<Item = io::Result<Bytes>>>,
    state: &mut DecodeState,
    exit: &mut u8,
) -> Result<(), u8> {
    let framing = if raw {
        Framing::Identity
    } else {
        proto.framing()
    };
    let mut decoder = framing.decoder();
    let mut dropped = false;
    for chunk in body {
        match chunk {
            // The framers are infallible for the shipped framings (sse §4); an Err
            // would be a future grammar's concern, so default to no frames.
            Ok(c) => {
                for frame in decoder.push(c).unwrap_or_default() {
                    emit(sink, decode_one(raw, proto, frame, state), exit)?;
                }
            }
            Err(_) => {
                let dropped_err = Event::Error(transport_err("transport stream dropped"));
                write_event(sink, dropped_err, exit)?;
                dropped = true;
                break;
            }
        }
    }
    if !dropped {
        for frame in decoder.finish().unwrap_or_default() {
            emit(sink, decode_one(raw, proto, frame, state), exit)?;
        }
        if !raw && !state.terminated {
            let eof_err = Event::Error(transport_err("premature upstream EOF"));
            write_event(sink, eof_err, exit)?;
        }
    }
    Ok(())
}

/// One frame → events: verbatim `Raw` bytes under `--raw`, else the protocol's
/// pure `decode` (the only home of provider-error parsing, §8).
fn decode_one(
    raw: bool,
    proto: &dyn Protocol,
    frame: Frame,
    state: &mut DecodeState,
) -> Result<Vec<Event>, CanonicalError> {
    if raw {
        Ok(vec![Event::Raw(frame.into_bytes())])
    } else {
        proto.decode(frame, state)
    }
}

/// Write a `decode` result: each event in order, or the decode error in-band. The
/// error arm is the single home for surfacing a malformed/whole-body provider
/// error as an `Event::Error` (§8).
fn emit(
    sink: &mut dyn Sink,
    result: Result<Vec<Event>, CanonicalError>,
    exit: &mut u8,
) -> Result<(), u8> {
    match result {
        Ok(events) => {
            for ev in events {
                write_event(sink, ev, exit)?;
            }
            Ok(())
        }
        Err(e) => write_event(sink, Event::Error(e), exit),
    }
}

/// Write one event to the sink, computing the exit on an `Event::Error`
/// (last-error-wins, §8). A sink write failure ends the run with the mapped exit
/// (`BrokenPipe` → 141, the Windows SIGPIPE path; else 69) — returned as `Err`.
pub(super) fn write_event(sink: &mut dyn Sink, ev: Event, exit: &mut u8) -> Result<(), u8> {
    if let Event::Error(e) = &ev {
        *exit = e.exit_code();
    }
    sink.write(&ev).map_err(|io| ExitClass::from_io(&io).code())
}

/// A `Transport`-kind error (§8 → exit 69): a transport drop or premature EOF.
fn transport_err(message: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: message.to_owned(),
        provider_detail: None,
    }
}

/// Is this a 2xx status? The one place the success/error split is named — `models`
/// reads the same boundary for its GET (the verb/probe share this rule, not a
/// re-coded range).
pub(super) fn is_2xx(status: u16) -> bool {
    (200..300).contains(&status)
}

/// The initial exit from the peeked HTTP status (§8): 2xx → 0 (an in-band error
/// may still override), else the status-driven class (401/403 → 77, other 4xx →
/// 69, 5xx → 70). Under `--raw` this is the final word; normalized, the whole-body
/// in-band error re-derives the same code from the same status.
fn exit_from_status(status: u16) -> u8 {
    if is_2xx(status) {
        ExitClass::Ok.code()
    } else {
        ExitClass::from_kind(ErrorKind::from_http_status(status)).code()
    }
}

#[cfg(test)]
mod tests {
    use super::{append_hint, with_hint};
    use crate::canonical::{CanonicalError, ErrorKind, Event};

    fn err(msg: &str) -> CanonicalError {
        CanonicalError {
            kind: ErrorKind::Provider { status: 404 },
            message: msg.to_owned(),
            provider_detail: None,
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
    /// passes any other event through (the §4.3 status-frame yields only the error, so
    /// the pass-through is unit-pinned like `print_models`' suffix); `with_hint` is a
    /// no-op with `None` and passes a decode `Err` straight through (`Result::map`).
    #[test]
    fn the_hint_enriches_only_the_error_and_no_ops_otherwise() {
        assert_eq!(msg(append_hint(Event::Error(err("boom")), "x")), "boom; x");
        assert_eq!(msg(append_hint(Event::End, "x")), "<non-error>");
        let ok = with_hint(Ok(vec![Event::Error(err("a"))]), None).unwrap();
        assert_eq!(ok.into_iter().map(msg).collect::<Vec<_>>(), ["a"]);
        assert_eq!(
            with_hint(Err(err("y")), Some("x")).unwrap_err().message,
            "y"
        );
    }
}
