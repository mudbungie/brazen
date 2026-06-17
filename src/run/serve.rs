//! The request half of the spine (arch §4.4), live once the sink exists: read →
//! resolve → dispatch → encode → auth → send, then hand off to [`drive`](super::respond)
//! for the response. Every failure from here on is in-band — a `CanonicalError`
//! written through the sink, then the one `End`, then the exit (§8). No vendor name
//! is matched: dispatch is the registry lookup (§4.4).

use std::io::Read;

use crate::auth::AuthCtx;
use crate::canonical::{CanonicalError, CanonicalRequest, ErrorKind, Event};
use crate::config::{fill_absent, PartialConfig};
use crate::pipeline::{read_request, Sink};
use crate::protocol::{ProviderCtx, WireRequest};
use crate::registry::Registry;
use crate::store::{Clock, CredStore};
use crate::transport::Transport;

use super::respond::{drive, write_event};

/// The post-sink pipeline (§4.4): read → resolve → dispatch → encode → auth →
/// send → drive. Every error is written in-band and ends the run with its exit
/// code; `merged` is consumed by resolution.
#[allow(clippy::too_many_arguments)]
pub(super) fn serve(
    reader: &mut dyn Read,
    raw: bool,
    prompt: Option<String>,
    merged: PartialConfig,
    sink: &mut dyn Sink,
    transport: &dyn Transport,
    store: &dyn CredStore,
    clock: &dyn Clock,
) -> u8 {
    // Input: raw stdin bytes verbatim, or the canonical request (positional XOR
    // stdin). The mode was resolved before input, so this never branches on body.
    let input = if raw {
        match read_to_vec(reader) {
            Ok(bytes) => Input::Raw(bytes),
            Err(e) => return fail_inband(sink, e),
        }
    } else {
        match read_request(prompt.as_deref(), reader) {
            Ok(req) => Input::Canonical(req),
            Err(e) => return fail_inband(sink, e),
        }
    };

    // The request's model wins for routing when set; cloned so resolution does not
    // borrow `input` (which is moved into the wire below).
    let req_model = match &input {
        Input::Canonical(req) if !req.model.is_empty() => Some(req.model.clone()),
        _ => None,
    };
    let cfg = match merged.into_resolved(req_model.as_deref()) {
        Ok(c) => c,
        Err(e) => return fail_inband(sink, e.into()),
    };

    let registry = Registry::builtin();
    // Dispatch is a total match over the closed `ProtocolId`/`AuthId` key-enums
    // (registry §4.4), so both lookups return an impl directly — unrepresentable to
    // miss. An `oauth2` row that cannot run is surfaced earlier, at resolve, as a
    // missing `oauth` block (78).
    let proto = registry.protocol(cfg.provider.protocol);
    let auth = registry.auth(cfg.provider.auth);

    let beta: Vec<(&str, &str)> = cfg
        .provider
        .beta_headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let ctx = ProviderCtx {
        base_url: &cfg.provider.base_url,
        model: &cfg.model,
        beta_headers: &beta,
    };
    let authc = AuthCtx {
        store_key: &cfg.provider.name,
        inline_key: cfg.inline_key.as_ref(),
        api_header: cfg.provider.api_header.as_ref(),
        oauth: cfg.provider.oauth.as_ref(),
    };

    let mut wire = match input {
        // --raw skips encode, so it gets the SAME target encode would build from the
        // protocol's one path home (`proto.path`) — `base_url` + path. Without this the
        // url stays empty and every raw request is a connect error (bl-080b).
        Input::Raw(bytes) => {
            WireRequest::new(format!("{}{}", ctx.base_url, proto.path(&ctx)), bytes)
        }
        Input::Canonical(mut req) => {
            fill_absent(&mut req, &cfg);
            // Streaming is the implicit default: `drive` decodes a 2xx only as a
            // framed stream (every concrete protocol frames SSE/NDJSON; there is no
            // non-stream-2xx fold), and serve owns that round-trip — so it requests
            // streaming unless the request/config opted out (architecture §3.2,
            // config §4.2). `get_or_insert` leaves an explicit `Some(false)` intact.
            req.stream.get_or_insert(true);
            match proto.encode(&req, &ctx) {
                Ok(w) => w,
                Err(e) => return fail_inband(sink, e),
            }
        }
    };
    // Stamp the resolved transport timeouts onto the request the impure transport
    // consumes (config §4). Done here, once, for both the encoded and raw paths —
    // `encode` stays timeout-agnostic — and BEFORE `auth.apply`, so the silent
    // OAuth refresh's own token POST inherits the same bounds (auth/refresh.rs).
    wire.timeouts = cfg.timeouts();
    if let Err(e) = auth.apply(&mut wire, &ctx, &authc, store, clock, transport) {
        return fail_inband(sink, e);
    }
    let resp = match transport.send(wire) {
        Ok(r) => r,
        Err(e) => return fail_inband(sink, e),
    };
    drive(sink, raw, proto, resp)
}

/// Either input channel after resolution: provider-native bytes (`--raw`, sent
/// verbatim) or a canonical request (encoded). The mode picks the variant once.
enum Input {
    Raw(Vec<u8>),
    Canonical(CanonicalRequest),
}

/// Emit a pre-streaming `CanonicalError` in-band, then the one `End`, returning
/// the exit (§8). Under `--raw` the sink drops the error line; the exit still
/// carries it (§5.4).
fn fail_inband(sink: &mut dyn Sink, err: CanonicalError) -> u8 {
    let mut exit = err.exit_code();
    match write_event(sink, Event::Error(err), &mut exit)
        .and_then(|()| write_event(sink, Event::End, &mut exit))
    {
        Ok(()) => exit,
        Err(code) => code,
    }
}

/// Read a byte source to end into a `Vec` (the `--raw` request body), mapping an
/// IO failure to an in-band input error (64).
fn read_to_vec(reader: &mut dyn Read) -> Result<Vec<u8>, CanonicalError> {
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).map_err(|e| CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("failed to read stdin: {e}"),
        provider_detail: None,
    })?;
    Ok(buf)
}
