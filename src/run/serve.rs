//! The `--raw` byte path (arch §5.4): provider-native bytes in, provider-native bytes
//! out. Distinct from the typed [`generate`](super::generate) core because raw NEVER
//! decodes — it sends the user's bytes verbatim and streams the response back as
//! `Event::Raw` through the sink, so there are no canonical events to carry the exit;
//! the exit is SEEDED from the peeked status (raw's final word bar a transport drop,
//! §5.4) rather than derived from an in-band error. read → resolve → send → stream.

use std::io::Read;

use crate::canonical::{CanonicalError, ErrorKind, Event};
use crate::config::PartialConfig;
use crate::pipeline::Sink;
use crate::protocol::{Framing, WireRequest};
use crate::registry::Registry;
use crate::transport::TransportResponse;

use super::events::{exit_from_status, fail_inband, transport_err, write_event};
use super::Host;

/// Send a raw request and stream the raw response (arch §5.4). The model cache and
/// `encode` are bypassed — `--raw`'s contract is exactly-the-user's-bytes, so the model
/// is never read (resolving it would be waste and would break that contract, config
/// §4.2). The wire target is still the protocol's one path home (`base_url` + `path`),
/// and content-type / static beta-headers / timeouts are stamped as on the typed path —
/// without them a JSON provider 400s a raw POST (bl-da81/bl-3e2f).
pub(super) fn serve_raw(
    reader: &mut dyn Read,
    merged: PartialConfig,
    sink: &mut dyn Sink,
    host: &Host,
) -> u8 {
    let bytes = match read_to_vec(reader) {
        Ok(b) => b,
        Err(e) => return fail_inband(sink, e),
    };
    // No request model to route on (the bytes are opaque), so resolution routes on the
    // row alias alone; the model field is never read on this path.
    let cfg = match merged.into_resolved(None) {
        Ok(c) => c,
        Err(e) => return fail_inband(sink, e.into()),
    };
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
    if let Err(e) = auth.apply(
        &mut wire,
        &ctx,
        &authc,
        host.store,
        host.clock,
        host.transport,
    ) {
        return fail_inband(sink, e);
    }
    let resp = match host.transport.send(wire) {
        Ok(r) => r,
        Err(e) => return fail_inband(sink, e),
    };
    stream_raw(sink, resp)
}

/// Stream the response bytes as `Event::Raw` (arch §5.4): the exit is SEEDED from the
/// status (raw decodes no error to carry it) and overridden only by a transport drop
/// (→ 69). `--raw` is `Identity` framing (sse §8) — each transport chunk is passed
/// through verbatim as one frame; the framer is STATELESS, so there is no buffered tail
/// to flush (no `finish`, unlike the SSE/NDJSON stream). The `RawSink` writes the bytes
/// and drops the drop-error LINE — the exit still carries it. A drop ends the stream.
fn stream_raw(sink: &mut dyn Sink, resp: TransportResponse) -> u8 {
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

/// Read a byte source to end into a `Vec` (the `--raw` request body), mapping an IO
/// failure to an in-band input error (64).
fn read_to_vec(reader: &mut dyn Read) -> Result<Vec<u8>, CanonicalError> {
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).map_err(|e| CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("failed to read stdin: {e}"),
        provider_detail: None,
        retry_after_seconds: None,
    })?;
    Ok(buf)
}
