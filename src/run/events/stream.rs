//! The streaming 2xx path as a pull iterator (arch §4.4): each `next` pulls transport
//! chunks through the framer and `decode` until an event is ready. A mid-stream drop
//! is an in-band `Transport` error that ends the stream; a clean EOF flushes a trailing
//! unterminated frame and — if the provider terminal marker was never seen — fires a
//! premature-EOF error. This is the lazy counterpart of the old push loop: identical
//! frames, identical errors, only inverted control flow.

use std::collections::VecDeque;
use std::io;

use crate::canonical::Event;
use crate::protocol::{DecodeState, Decoder, Frame, Protocol};
use crate::transport::Bytes;

use super::{premature_eof_with_body, transport_err};

/// The bounded body sample retained for the zero-frames diagnostic (§5.6): a 200 whose
/// body never framed a single frame (gateway HTML, a JSON error served 200) attaches
/// this leading slice to its premature-EOF error so the real upstream text is
/// diagnosable rather than discarded. 8 KiB is enough to carry a provider error
/// envelope whole while never holding a real response body.
const PREMATURE_EOF_BODY_CAP: usize = 8 * 1024;

pub(super) struct StreamEvents {
    proto: &'static dyn Protocol,
    body: Box<dyn Iterator<Item = io::Result<Bytes>>>,
    decoder: Box<dyn Decoder>,
    state: DecodeState,
    pending: VecDeque<Event>,
    body_done: bool,
    finished: bool,
    /// Whether the framer has ever yielded a frame. The single home for "did this body
    /// frame at all": the driver is the one place frames flow (through
    /// `decode_into_pending`) and owns the premature-EOF injection, so the fact lives
    /// here — NOT on the event-blind framer (whose only state is its byte buffer, sse
    /// §6) nor on `DecodeState` (which `decode` mutates per frame, never "ever").
    saw_frame: bool,
    /// The bounded leading-body sample, accumulated only while `!saw_frame`; dropped the
    /// moment a frame decodes (a framed stream is self-describing). Rides `provider_detail`
    /// on a zero-frames premature EOF (§5.6).
    head: Vec<u8>,
}

impl StreamEvents {
    pub(super) fn new(
        proto: &'static dyn Protocol,
        body: Box<dyn Iterator<Item = io::Result<Bytes>>>,
    ) -> Self {
        StreamEvents {
            decoder: proto.framing().decoder(),
            proto,
            body,
            state: DecodeState::default(),
            pending: VecDeque::new(),
            body_done: false,
            finished: false,
            saw_frame: false,
            head: Vec::new(),
        }
    }

    /// Decode one frame into the pending queue, surfacing a decode error in-band — the
    /// single home for a malformed-frame `Event::Error` on the stream (§8). Records that
    /// a frame was seen (regardless of decode outcome — a framed-but-malformed frame IS a
    /// frame, so the body was SSE) and drops the diagnostic head, now moot.
    fn decode_into_pending(&mut self, frame: Frame) {
        self.saw_frame = true;
        self.head = Vec::new();
        match self.proto.decode(frame, &mut self.state) {
            Ok(events) => self.pending.extend(events),
            Err(e) => self.pending.push_back(Event::Error(e)),
        }
    }

    /// Buffer up to the cap's worth of the leading body while nothing has framed yet —
    /// the diagnostic sample for a non-SSE 200 (§5.6). A no-op once a frame is seen.
    fn accumulate_head(&mut self, chunk: &[u8]) {
        if self.saw_frame {
            return;
        }
        let room = PREMATURE_EOF_BODY_CAP.saturating_sub(self.head.len());
        self.head.extend_from_slice(&chunk[..room.min(chunk.len())]);
    }

    /// The premature-EOF error at a drained-but-unterminated stream (§5.6). A stream that
    /// framed at least one frame gets the bare error (its content already surfaced); one
    /// that never framed — a 200 whose body is not SSE — carries the accumulated body
    /// head in `provider_detail`, so the actual upstream error is not silently discarded.
    fn premature_eof(&self) -> Event {
        Event::Error(if self.saw_frame {
            transport_err("premature upstream EOF")
        } else {
            premature_eof_with_body(&self.head)
        })
    }
}

impl Iterator for StreamEvents {
    type Item = Event;

    fn next(&mut self) -> Option<Event> {
        loop {
            if let Some(ev) = self.pending.pop_front() {
                return Some(ev);
            }
            if self.finished {
                return None;
            }
            if self.body_done {
                // The framers are infallible for the shipped framings (sse §4); an Err
                // would be a future grammar's concern, so default to no frames.
                let tail = self.decoder.finish().unwrap_or_default();
                for frame in tail {
                    self.decode_into_pending(frame);
                }
                if !self.state.terminated {
                    self.pending.push_back(self.premature_eof());
                }
                self.finished = true;
                continue;
            }
            match self.body.next() {
                Some(Ok(chunk)) => {
                    self.accumulate_head(&chunk);
                    for frame in self.decoder.push(chunk).unwrap_or_default() {
                        self.decode_into_pending(frame);
                    }
                }
                Some(Err(_)) => {
                    // A transport drop ends the stream — no finish/EOF check follows.
                    self.pending
                        .push_back(Event::Error(transport_err("transport stream dropped")));
                    self.finished = true;
                }
                None => self.body_done = true,
            }
        }
    }
}
