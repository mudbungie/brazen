//! The masquerade shell (ingress.md §5, §7–§11): the ONE per-request bridge both
//! front doors share — `--serve` (the listener, [`listen`]) and `--in` (the
//! one-shot filter, [`filter`]). Per request it does `decode_request` → the
//! replay-stash recall/re-inject (§5) → the ordinary `generate` → the encoder
//! fold — nothing inside `generate` learns it is served (§7). The shell owns the
//! two stash join points the encoder exposes (`take_stash`, fail-open writes)
//! and the §9 edge rejections (decode/config/auth/route failures re-encoded in
//! the CLIENT dialect before any round-trip). The transport-shaped seams live
//! here too: [`Bind`]/[`Listener`]/[`ServeConn`] are the accept loop's injection
//! points — the shim wires the OS TCP listener (`src/native/listen.rs`), tests
//! wire in-memory pairs — kept `Read + Write` so the lib never names a socket
//! (arch §9.5, the purity invariant).

mod filter;
mod http;
mod listen;

pub(super) use filter::filter;
pub use listen::{serve, ServeIo};

use core::net::SocketAddr;
use std::io::{self, Read, Write};

use crate::canonical::{CanonicalError, CanonicalRequest, Event, ExitClass};
use crate::config::partial::{LossyMode, PartialIngress};
use crate::config::{PartialConfig, ResolvedConfig};
use crate::ingress::{
    decode_request, encode_response, reinject, IngressId, IngressState, THINKING_REPLAY,
};
use crate::store::{Clock, ReplayStash};

use super::{generate, Host};

/// One accepted client connection: a blocking byte stream both ways. `Send` so
/// the accept loop can hand it to its connection thread (ingress §7).
pub trait ServeConn: Read + Write + Send {}
impl<T: Read + Write + Send> ServeConn for T {}

/// The accept seam (ingress §7): blocks for the next connection; `None` ends
/// the loop. The native impl accepts forever (SIGINT/SIGTERM end the process,
/// the repo's default-disposition signal convention); test doubles script a
/// finite queue, which is what keeps the loop's shutdown testable.
pub trait Listener {
    fn accept(&self) -> Option<Box<dyn ServeConn>>;
}

/// The bind seam: the resolved `[ingress].listen` address becomes a listener.
/// The shim wires the OS TCP bind; tests wire in-memory queues.
pub trait Bind {
    fn bind(&self, addr: SocketAddr) -> io::Result<Box<dyn Listener>>;
}

/// The response consumer the two front doors implement: `--in` writes bytes to
/// stdout, `--serve` speaks HTTP (status line + headers, then a `Content-Length`
/// aggregate or chunked SSE). `begin` is called exactly once, before any bytes —
/// lazily, at the first encoded chunk, so an early in-band `Error` event has
/// already stamped the §9 masqueraded status by then.
pub(super) trait Respond {
    fn begin(&mut self, status: u16, sse: bool) -> io::Result<()>;
    fn chunk(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn end(&mut self) -> io::Result<()>;
}

/// One masquerade request's inputs: the dialect, the resolved rung-3 policy for
/// [`THINKING_REPLAY`], the merged config (consumed by resolution), the stash.
pub(super) struct MasqIn<'a> {
    pub dialect: IngressId,
    pub reject: bool,
    pub merged: PartialConfig,
    pub stash: &'a ReplayStash,
}

/// Decode + stash-recall + resolve — the shared pre-`generate` half (§2, §5,
/// §6). Every failure is a `CanonicalError` the caller re-encodes at the edge
/// (§9) or, on the `--raw=out` composition, surfaces in-band.
pub(super) fn prepare(
    cx: MasqIn,
    body: &[u8],
) -> Result<(CanonicalRequest, ResolvedConfig, Vec<String>), CanonicalError> {
    // The ingress §4 never-silently-inert rule on the `--in` door: a present
    // table's override names are validated before any policy read, so a typo'd
    // adaptation is Config/78 exactly as on `--serve` (which already refused it
    // at startup via `resolve_ingress` — this re-check never fires there).
    if let Some(table) = &cx.merged.ingress {
        table.validate_lossy_overrides()?;
    }
    let mut req = decode_request(cx.dialect, body)?;
    let adaptations = reinject(&mut req, cx.stash, cx.reject)?;
    let req_model = (!req.model.is_empty()).then(|| req.model.clone());
    let cfg = cx.merged.into_resolved(req_model.as_deref())?;
    Ok((req, cfg, adaptations))
}

/// One full masquerade turn (§7): prepare, run the unchanged pipeline, fold the
/// canonical events through the ingress encoder into `out`, then write the §5
/// stash pairs (fail-open — a stash failure never fails the turn). Returns the
/// exit code the `--in` filter surfaces (last in-band error wins, §8); the
/// listener ignores it (the §9 status already carried the verdict). A write
/// failure ends the turn immediately: returning drops the `generate` iterator,
/// which drops the transport read — a mid-stream client disconnect kills only
/// this request's upstream (§7).
pub(super) fn turn(cx: MasqIn, body: &[u8], host: &Host, out: &mut dyn Respond) -> u8 {
    let dialect = cx.dialect;
    let stash = cx.stash;
    let (req, cfg, adaptations) = match prepare(cx, body) {
        Ok(p) => p,
        Err(e) => return edge(dialect, e, host.clock, out),
    };
    let mut state = IngressState::for_request(&req, adaptations, host.clock);
    let sse = state.stream;
    let mut exit = ExitClass::Ok.code();
    let mut begun = false;
    let outcome: Result<(), io::Error> = (|| {
        for ev in generate(req, cfg, host) {
            if let Event::Error(e) = &ev {
                exit = e.exit_code();
            }
            let bytes = encode_response(dialect, &ev, &mut state);
            if bytes.is_empty() {
                continue; // the aggregate fold: silent until `End` (§10)
            }
            if !begun {
                out.begin(state.status(), sse)?;
                begun = true;
            }
            out.chunk(&bytes)?;
        }
        // The §5 write join point: the pairs `End` finalized, through the stash,
        // fail-open — degraded replay fidelity, never a failed turn. `begun` is
        // guaranteed by now: `generate` always ends with `End`, and `End` encodes
        // non-empty bytes on both shapes (the aggregate body / the sentinel).
        for (key, payload) in state.take_stash() {
            let _ = stash.stash(&key, &payload, host.clock);
        }
        out.end()
    })();
    match outcome {
        Ok(()) => exit,
        Err(io) => ExitClass::from_io(&io).code(),
    }
}

/// A §9 edge rejection (rung 4, auth 401, route 404, malformed HTTP): the error
/// re-encoded as the dialect's AGGREGATE envelope — no stream ever started, so
/// the shape is the pre-stream JSON body every SDK expects — with the status
/// from the one shared `ErrorKind` table. Returns the error's exit code.
pub(super) fn edge(
    dialect: IngressId,
    err: CanonicalError,
    clock: &dyn Clock,
    out: &mut dyn Respond,
) -> u8 {
    let exit = err.exit_code();
    let mut state = IngressState::for_request(&CanonicalRequest::default(), Vec::new(), clock);
    let mut body = encode_response(dialect, &Event::Error(err), &mut state);
    body.extend(encode_response(dialect, &Event::End, &mut state));
    let _ = out
        .begin(state.status(), false)
        .and_then(|()| out.chunk(&body))
        .and_then(|()| out.end());
    exit
}

/// The `--in` rung-3 policy read (ingress §4, §11): the [`THINKING_REPLAY`]
/// override, else the table's global `lossy`, else the `adapt` default — no
/// `[ingress]` table required (there is no listener to configure). The serve
/// path resolves the SAME fields through `resolve_ingress`. Override NAMES are
/// not this read's concern: `prepare` has every table pass
/// `validate_lossy_overrides` before the policy matters, so an unknown key is
/// Config/78 on both doors, never a silently inert entry.
pub(super) fn reject_replay(table: Option<&PartialIngress>) -> bool {
    table.is_some_and(|t| {
        t.lossy_overrides
            .get(THINKING_REPLAY)
            .copied()
            .unwrap_or_else(|| t.lossy.unwrap_or_default())
            == LossyMode::Reject
    })
}
