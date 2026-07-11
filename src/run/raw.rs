//! The `--raw` request half + byte-out response half (arch §5.4, §13.14): the verbatim
//! REQUEST path and the verbatim RESPONSE path, which toggle independently of each other
//! (the directional split). [`send_raw`] is the request half — provider-native bytes in,
//! NO `parse`/`encode`, the model cache bypassed — yielding a prepared [`Sent`] that
//! either response half (canonical `pump` or [`stream_raw`]) consumes, dispatched by
//! [`drive`](super::drive::drive). `stream_raw` is the raw response half: it streams the
//! body back as `Event::Raw`, so there are no canonical events to carry the exit — the
//! exit is SEEDED from the peeked status (raw's final word bar a transport drop, §5.4).

use std::io::Read;

use crate::canonical::{CanonicalError, ErrorKind, Event};
use crate::config::PartialConfig;
use crate::pipeline::Sink;
use crate::protocol::{Framing, WireRequest};
use crate::registry::Registry;
use crate::transport::TransportResponse;

use super::drive::Sent;
use super::events::{exit_from_status, transport_err, write_event};
use super::Host;

/// The verbatim (raw) request half (arch §5.4, §13.14): read the stdin body, resolve the
/// row, stamp headers/auth, and send — returning one prepared [`Sent`] the response half
/// projects (raw bytes out for bare `--raw`, canonical events out for `--raw=in`). The
/// model cache and `encode` are bypassed — `--raw`'s contract is exactly-the-user's-bytes,
/// so the model is never read (resolving it would be waste and would break that contract,
/// config §4.2); the §5.3 model-provenance `hint` is therefore always `None`. The wire
/// target is still the protocol's one path home (`base_url` + `path`), and content-type /
/// static beta-headers / timeouts are stamped as on the typed path — without them a JSON
/// provider 400s a raw POST (bl-da81/bl-3e2f). Any pre-send failure (read, config, auth,
/// transport) rides back as a `CanonicalError` for `drive` to surface in-band.
pub(super) fn send_raw(
    reader: &mut dyn Read,
    merged: PartialConfig,
    host: &Host,
) -> Result<Sent, CanonicalError> {
    let bytes = read_to_vec(reader)?;
    // The response framing (SSE stream vs one aggregate JSON, §5.6) reads the SAME single
    // fact the normalized path reads — the request's `stream` — but from the verbatim
    // bytes rather than a parsed field, so `--raw=in` frames the response correctly. This
    // is a read-only PEEK: the bytes still reach the wire verbatim; only response framing
    // consults it, and only the `--raw=in` (canonical-out) path uses the result.
    let streamed = peek_stream(&bytes);
    // No request model to route on (the bytes are opaque), so resolution routes on the
    // row alias alone; the model field is never read on this path.
    let cfg = merged.into_resolved(None)?;
    let registry = Registry::builtin();
    let proto = registry.protocol(cfg.provider.protocol);
    let auth = registry.auth(cfg.provider.auth);
    let beta: Vec<(&str, &str)> = cfg
        .provider
        .beta_headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let ctx = cfg.provider_ctx(&beta);
    let authc = cfg.auth_ctx();

    let mut wire = WireRequest::new(format!("{}{}", ctx.base_url, proto.path(&ctx)), bytes);
    wire.set_header("content-type", proto.content_type());
    for (k, v) in ctx.beta_headers {
        wire.set_header(k, v);
    }
    wire.timeouts = cfg.timeouts();
    auth.apply(
        &mut wire,
        &ctx,
        &authc,
        host.store,
        host.clock,
        host.transport,
    )?;
    let resp = host.transport.send(wire)?;
    Ok(Sent {
        proto,
        resp,
        streamed,
        hint: None,
    })
}

/// Peek the request's `stream` intent from the verbatim body (arch §5.4, §5.6): a minimal
/// read-only deserialize of the one top-level field the response framing needs, defaulting
/// to brazen's stream-native `true` — a non-JSON body or an absent field frames as SSE
/// (the same default `fill_absent` gives the typed path). The bytes are NOT modified.
fn peek_stream(bytes: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct StreamPeek {
        stream: Option<bool>,
    }
    serde_json::from_slice::<StreamPeek>(bytes)
        .ok()
        .and_then(|p| p.stream)
        .unwrap_or(true)
}

/// Stream the response bytes as `Event::Raw` (arch §5.4): the exit is SEEDED from the
/// status (raw decodes no error to carry it) and overridden only by a transport drop
/// (→ 69). `--raw` is `Identity` framing (sse §8) — each transport chunk is passed
/// through verbatim as one frame; the framer is STATELESS, so there is no buffered tail
/// to flush (no `finish`, unlike the SSE/NDJSON stream). The `RawSink` writes the bytes
/// and drops the drop-error LINE — the exit still carries it. A drop ends the stream.
pub(super) fn stream_raw(sink: &mut dyn Sink, resp: TransportResponse) -> u8 {
    let mut exit = exit_from_status(resp.status);
    let mut framer = Framing::Identity.decoder();
    let outcome = (|| {
        for chunk in resp.body {
            match chunk {
                // The Identity framer is infallible (sse §4); a push error would be a
                // future grammar's concern, so default to no frames.
                Ok(c) => {
                    for frame in framer.push(c).unwrap_or_default() {
                        write_event(sink, Event::Raw(frame.into_bytes()), &mut exit)?;
                    }
                }
                Err(_) => {
                    let err = transport_err("transport stream dropped");
                    return write_event(sink, Event::Error(err), &mut exit);
                }
            }
        }
        Ok(())
    })();
    match outcome.and_then(|()| write_event(sink, Event::End, &mut exit)) {
        Ok(()) => exit,
        Err(code) => code,
    }
}

/// Read a byte source to end into a `Vec` (the `--raw` request body, and the
/// `--in` dialect request — both verbatim stdin contracts), mapping an IO
/// failure to an in-band input error (64).
pub(super) fn read_to_vec(reader: &mut dyn Read) -> Result<Vec<u8>, CanonicalError> {
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).map_err(|e| CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("failed to read stdin: {e}"),
        provider_detail: None,
        retry_after_seconds: None,
    })?;
    Ok(buf)
}
