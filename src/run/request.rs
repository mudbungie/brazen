//! The shared generation-request tail (architecture §4.4): normalized and raw
//! request halves both arrive with a protocol-owned URL/body, then this module
//! appends row-owned query data, stamps static headers/timeouts, authenticates,
//! and performs the one send. Query data never replaces a protocol path.

use crate::auth::encode_pairs;
use crate::canonical::CanonicalError;
use crate::config::ResolvedConfig;
use crate::protocol::{Protocol, ProviderCtx, WireRequest};
use crate::registry::Registry;
use crate::transport::TransportResponse;

use super::Host;

/// Finish and send one generation request, identically for encoded and raw input.
/// The protocol has already built `wire.url`; `generation_query` is an optional
/// suffix over that authoritative path. Empty query data performs no URL write.
pub(super) fn send(
    mut wire: WireRequest,
    cfg: &ResolvedConfig,
    proto: &'static dyn Protocol,
    ctx: &ProviderCtx,
    host: &Host,
) -> Result<TransportResponse, CanonicalError> {
    append_query(&mut wire.url, &cfg.provider.generation_query);
    wire.set_header("content-type", proto.content_type());
    for (name, value) in ctx.beta_headers {
        wire.set_header(name, value);
    }
    wire.timeouts = cfg.timeouts();

    Registry::builtin().auth(cfg.provider.auth).apply(
        &mut wire,
        ctx,
        &cfg.auth_ctx(),
        host.store,
        host.clock,
        host.transport,
    )?;
    host.transport.send(wire)
}

/// Append percent-encoded query pairs without disturbing a protocol-owned query.
/// The empty list is the identity. A streaming Google URL already ending in
/// `?alt=sse` receives `&…`; a plain Messages URL receives `?…`.
pub(crate) fn append_query(url: &mut String, query: &[(String, String)]) {
    if query.is_empty() {
        return;
    }
    let pairs: Vec<(&str, &str)> = query
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let separator = if url.contains('?') {
        if url.ends_with('?') || url.ends_with('&') {
            ""
        } else {
            "&"
        }
    } else {
        "?"
    };
    url.push_str(separator);
    url.push_str(&encode_pairs(&pairs));
}
