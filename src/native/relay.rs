//! The transport-delegate relay (transport spec §5.2) — the `Envelope::Http` half of
//! the subprocess seam: read the operator program's response head off the SAME child
//! stream [`exec`](super::exec) already gives us, then hand the rest on as the body.
//!
//! It is a decorator, not a second transport: spawn, stdin feed, stderr fold, the
//! silence budget and the reap/`Drop` backstop all stay in `exec`. The only thing
//! added here is where the head stops and the body starts — and that split is the
//! pure lib codec (`envelope_head`), so this file holds no parsing at all. Impure by
//! association (it consumes a child), so it lives in the coverage-excluded shim; the
//! behaviour is pinned end-to-end by `tests/transport_conformance.rs`.

use brazen::{envelope_error, envelope_head, Bytes, CanonicalError, TransportResponse};

use super::exec::ExecBody;

/// Turn a spawned delegate's stdout into a `TransportResponse`: pull chunks until
/// the head parses, then stream on. Bytes that arrived alongside the head are the
/// body's FIRST chunk — never buffered to end, so a streamed provider response stays
/// streamed through the delegate (spec §5.2).
///
/// The wait for the head is bounded by the same inter-chunk silence budget that
/// bounds the body (`exec`'s stall kill), so a delegate that spawns and says nothing
/// is killed exactly like one that stalls mid-stream. A child that dies first
/// surfaces its own error — including the stderr text `exec` folds onto a nonzero
/// exit, which is how a delegate's own diagnostics survive (spec §6).
pub(super) fn respond(mut child: ExecBody) -> Result<TransportResponse, CanonicalError> {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        if let Some(head) = envelope_head(&buf)? {
            let prefix = buf.split_off(head.body_start);
            let first: Option<std::io::Result<Bytes>> = (!prefix.is_empty()).then_some(Ok(prefix));
            return Ok(TransportResponse {
                status: head.status,
                retry_after: head.retry_after,
                body: Box::new(first.into_iter().chain(child)),
            });
        }
        match child.next() {
            Some(Ok(chunk)) => buf.extend_from_slice(&chunk),
            Some(Err(e)) => {
                return Err(envelope_error(&format!(
                    "delegate failed before the response head: {e}"
                )))
            }
            None => {
                return Err(envelope_error(
                    "delegate closed stdout before the response head",
                ))
            }
        }
    }
}
