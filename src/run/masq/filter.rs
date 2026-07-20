//! `bz --in DIALECT` (ingress.md §11): the one-shot ingress filter — the same
//! codecs, ladder, and stash as the listener, no listener. One dialect request
//! read whole from stdin, the dialect response written to stdout: aggregate by
//! default, SSE frames when the request says `stream:true` (§10). No `[ingress]`
//! table is required; a present table's lossy fields are honored (§4). It
//! composes with `--raw=out` exactly like canonical input does — the dialect
//! request drives the ordinary encoded send, and the provider's exact response
//! bytes stream back through the `RawSink` (the encoder never runs, so a fired
//! adaptation has no surface there — raw out drops the canonical vocabulary by
//! contract, §5.4).

use std::io::{Read, Write};

use crate::config::PartialConfig;
use crate::ingress::IngressId;
use crate::pipeline::RawSink;

use super::super::events::fail_inband;
use super::super::raw::read_to_vec;
use super::super::{drive, generate};
use super::{edge, prepare, reject_replay, turn, Host, MasqIn, Respond};

/// Run the `--in` filter and return the POSIX exit code: the last in-band
/// error's class, 0 on a clean turn (§8) — the dialect envelope on stdout
/// carries the story either way.
pub(crate) fn filter(
    dialect: IngressId,
    reader: &mut dyn Read,
    merged: PartialConfig,
    raw_out: bool,
    stdout: &mut dyn Write,
    host: &Host,
) -> u8 {
    let cx = MasqIn {
        dialect,
        reject: reject_replay(merged.ingress.as_ref()),
        merged,
        stash: host.stash,
        cache: host.cache,
    };
    // The one-shot read: stdin IS the request (§11). A read failure surfaces
    // in-band like every post-sink failure — as the dialect envelope (or, on
    // the raw composition, as the exit the RawSink path carries).
    let body = match read_to_vec(reader) {
        Ok(bytes) => bytes,
        Err(e) => {
            if raw_out {
                return fail_inband(&mut RawSink::new(stdout), e);
            }
            return edge(dialect, e, host.clock, &mut StdoutRespond { w: stdout });
        }
    };
    if raw_out {
        // The §11 `--raw=out` composition: dialect in, provider bytes out.
        let mut sink = RawSink::new(stdout);
        return match prepare(cx, &body) {
            Ok((req, cfg, _)) => drive::drive(
                generate::send_encoded(req, cfg, host),
                true,
                &mut sink,
                host.clock.now(),
            ),
            Err(e) => fail_inband(&mut sink, e),
        };
    }
    turn(cx, &body, host, &mut StdoutRespond { w: stdout })
}

/// The stdout half of [`Respond`]: the POSIX filter has no status line — the
/// §9 verdict rides the exit code — so `begin` is a no-op and the bytes flow
/// as the encoder emits them (SSE frames incrementally, the aggregate whole).
struct StdoutRespond<'a> {
    w: &'a mut dyn Write,
}

impl Respond for StdoutRespond<'_> {
    fn begin(&mut self, _status: u16, _sse: bool) -> std::io::Result<()> {
        Ok(())
    }

    fn chunk(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.w.write_all(bytes).and_then(|()| self.w.flush())
    }

    fn end(&mut self) -> std::io::Result<()> {
        self.w.flush()
    }
}
