//! The request/response drive seam (arch §5.4, §13.14): the `--raw` directional split
//! made real. A generation is two halves that toggle **independently** — the REQUEST
//! half (verbatim bytes → wire, or the ergonomic constructor → `encode`) yields one
//! prepared response [`Sent`]; the RESPONSE half projects it (canonical events through
//! `pump`, or the provider's exact bytes through the `RawSink`). `drive` picks the
//! response half from `raw_out` alone, so the same `Sent` feeds either — the four
//! input×output combinations are these two 2-way choices, not four pipelines. The two
//! request halves live in [`send_raw`](super::raw::send_raw) and
//! [`send_encoded`](super::generate::send_encoded); this module owns the seam and the
//! response-half dispatch that both, plus the public [`generate`](super::generate), share.

use crate::canonical::{CanonicalError, Event};
use crate::pipeline::{pump, Sink};
use crate::protocol::Protocol;
use crate::transport::TransportResponse;

use super::events::{fail_inband, response_events};
use super::raw::stream_raw;

/// One prepared response — the output of EITHER request half, the input of EITHER
/// response half (arch §5.4). `proto` frames+decodes the body on the canonical-out
/// path; `streamed` routes the 2xx body (SSE stream vs one aggregate JSON, §5.6);
/// `hint` is the §5.3 model-provenance note (always `None` on the raw-in path, which
/// bypasses the model cache). The raw-out path reads only `resp` (byte passthrough).
pub(super) struct Sent {
    pub proto: &'static dyn Protocol,
    pub resp: TransportResponse,
    pub streamed: bool,
    pub hint: Option<String>,
}

/// Project a prepared response through the response half chosen by `raw_out` (arch
/// §5.4): raw-out streams the provider bytes verbatim (`RawSink`, the provider's own
/// terminator, exit SEEDED from the peeked status); else the canonical events flow
/// through `pump` with the one trailing `End` (exit derived from the in-band error).
/// The **raw-4xx/5xx-never-exits-0 rule holds in both** (§8) — the status is peeked on
/// the raw path, and decoded into an `Event::Error` on the canonical path. A request-
/// half failure (`Err`) is the pre-stream fatal: one in-band `Event::Error` + `End`
/// through the same sink, whatever the mode (§5.9).
pub(super) fn drive(
    sent: Result<Sent, CanonicalError>,
    raw_out: bool,
    sink: &mut dyn Sink,
    now: u64,
) -> u8 {
    match sent {
        Ok(sent) if raw_out => stream_raw(sink, sent.resp),
        Ok(sent) => pump(canonical_events(sent, now), sink),
        Err(e) => fail_inband(sink, e),
    }
}

/// The canonical response stream: frame+decode the body into events, terminated by the
/// one `End` (arch §5.6). Shared by [`drive`]'s canonical-out arm and the public
/// [`generate`](super::generate) — the single home of "response half → canonical
/// events + End", so `generate` and the `--raw=in` path can never disagree.
pub(super) fn canonical_events(sent: Sent, now: u64) -> Box<dyn Iterator<Item = Event>> {
    Box::new(
        response_events(sent.proto, sent.resp, sent.streamed, sent.hint, now)
            .chain(std::iter::once(Event::End)),
    )
}
