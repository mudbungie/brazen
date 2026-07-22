//! The stdio HTTP envelope (transport spec §5): the PURE codec an operator-selected
//! transport delegate speaks — one whole HTTP/1.1 request message written to the
//! child's stdin, one whole HTTP/1.1 response message read back off its stdout. The
//! contract is HTTP itself, so there is no second vocabulary to version or misparse.
//!
//! Pure by construction (no spawn, no socket — that is the shim's job in
//! `src/native/`), so it lives in the library beside the seam it serves, is unit
//! tested to the line, and is public: an embedder writing its own `Transport` speaks
//! the same codec the `bz` shim does, from the same one home.

use crate::canonical::{CanonicalError, ErrorKind};
use crate::protocol::{Method, WireRequest};

/// The response head's hard ceiling. A delegate that never emits the blank line
/// would otherwise make `bz` buffer without bound: the silence budget catches a
/// stalled child, not a chatty one. 64 KiB is far above any real head.
const MAX_HEAD: usize = 64 * 1024;

/// Render the request the delegate must perform (spec §5.1): an **absolute-form**
/// request line (RFC 9112 §3.2.2 — what a proxy receives, so the child needs no
/// second URL channel), then `wire.headers` VERBATIM in order, then the body.
///
/// Brazen synthesizes nothing here — no `Host`, no `Content-Length`, no
/// `Accept-Encoding`, no `User-Agent`. Generating those IS the transport identity
/// being delegated; a header invented here is a header the operator's stack could
/// not own. The body is framed by stdin CLOSE (one request per child), so the
/// envelope needs neither a length nor chunking.
pub fn envelope_request(wire: &WireRequest) -> Vec<u8> {
    let method = match wire.method {
        Method::Post => "POST",
        Method::Get => "GET",
    };
    let mut out = format!("{method} {} HTTP/1.1\r\n", wire.url).into_bytes();
    for (name, value) in &wire.headers {
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(&wire.body);
    out
}

/// What Brazen reads off a delegate's response head (spec §5.2): the status every
/// transport already carries, the ONE response header it already keeps —
/// `retry-after`, verbatim (arch §3.3) — and where the BODY starts in the buffer, so
/// bytes that arrived alongside the head stream on as the first chunk instead of
/// being buffered to end. Widening the header set stays additive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvelopeHead {
    pub status: u16,
    pub retry_after: Option<String>,
    pub body_start: usize,
}

/// Parse the response head out of however much of the delegate's stdout has arrived
/// (spec §5.2). `Ok(None)` = still incomplete, read more; `Ok(Some(head))` = parsed,
/// with `head.body_start` splitting the buffer; `Err` = malformed or oversized
/// (spec §6, → exit 69). Caller-owned buffer, so the codec stays a pure function of
/// the bytes so far — no hidden state, and every arm is reachable from a table test.
pub fn envelope_head(buf: &[u8]) -> Result<Option<EnvelopeHead>, CanonicalError> {
    let Some(end) = head_end(buf) else {
        return if buf.len() > MAX_HEAD {
            Err(envelope_error(
                "response head exceeds 64 KiB with no blank line",
            ))
        } else {
            Ok(None)
        };
    };
    let text = String::from_utf8_lossy(&buf[..end]);
    let mut lines = text.lines();
    // The status is the status line's SECOND whitespace-separated token, so
    // `HTTP/1.1 200 OK`, `HTTP/1.0 429` and an `HTTP/2 200` spelling all read alike.
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|t| t.parse::<u16>().ok())
        .ok_or_else(|| envelope_error("response carries no HTTP status line"))?;
    let retry_after = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("retry-after"))
        .map(|(_, value)| value.trim().to_owned());
    Ok(Some(EnvelopeHead {
        status,
        retry_after,
        body_start: end,
    }))
}

/// The index just past the head-terminating blank line — `CRLFCRLF` or a bare
/// `LFLF`, whichever ends first (lenient in what it accepts: the delegate is the
/// operator's own program, not a hardened peer).
fn head_end(buf: &[u8]) -> Option<usize> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| i + 2);
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
    lf.into_iter().chain(crlf).min()
}

/// A malformed envelope as the seam's `Transport` error (→69). The message names a
/// REASON and never echoes head bytes: a broken — or hostile — delegate that echoed
/// the request back must not be able to make `bz` print the credential (spec §6).
pub fn envelope_error(reason: &str) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: format!("stdio transport: {reason}"),
        provider_detail: None,
        retry_after_seconds: None,
    }
}
