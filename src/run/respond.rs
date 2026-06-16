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
/// terminal marker suppresses the premature-EOF injection (CR-9).
pub(super) fn drive(
    sink: &mut dyn Sink,
    raw: bool,
    proto: &dyn Protocol,
    resp: TransportResponse,
) -> u8 {
    let status = resp.status;
    let mut exit = exit_from_status(status);
    let mut state = DecodeState::default();
    let outcome = if !is_2xx(status) && !raw {
        whole_body(sink, proto, status, resp.body, &mut state, &mut exit)
    } else {
        stream(sink, raw, proto, resp.body, &mut state, &mut exit)
    };
    match outcome.and_then(|()| write_event(sink, Event::End, &mut exit)) {
        Ok(()) => exit,
        Err(code) => code,
    }
}

/// The non-2xx normalized path (sse §9): collect the whole body as one error frame
/// carrying the authoritative status, and let `decode` parse the envelope.
fn whole_body(
    sink: &mut dyn Sink,
    proto: &dyn Protocol,
    status: u16,
    body: Box<dyn Iterator<Item = io::Result<Bytes>>>,
    state: &mut DecodeState,
    exit: &mut u8,
) -> Result<(), u8> {
    match drain(body) {
        Ok(data) => {
            let frame = Frame {
                event: None,
                data,
                status: Some(status),
            };
            emit(sink, decode_one(false, proto, frame, state), exit)
        }
        Err(()) => {
            let err = Event::Error(transport_err("failed to read error response body"));
            write_event(sink, err, exit)
        }
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

/// Collect a response body to end; `Err(())` on a mid-collection transport drop.
fn drain(body: Box<dyn Iterator<Item = io::Result<Bytes>>>) -> Result<Vec<u8>, ()> {
    let mut buf = Vec::new();
    for chunk in body {
        match chunk {
            Ok(c) => buf.extend_from_slice(&c),
            Err(_) => return Err(()),
        }
    }
    Ok(buf)
}

/// A `Transport`-kind error (§8 → exit 69): a transport drop or premature EOF.
fn transport_err(message: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: message.to_owned(),
        provider_detail: None,
    }
}

/// Is this a 2xx status? The one place the success/error split is named.
fn is_2xx(status: u16) -> bool {
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
