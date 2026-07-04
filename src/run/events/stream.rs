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

use super::transport_err;

pub(super) struct StreamEvents {
    proto: &'static dyn Protocol,
    body: Box<dyn Iterator<Item = io::Result<Bytes>>>,
    decoder: Box<dyn Decoder>,
    state: DecodeState,
    pending: VecDeque<Event>,
    body_done: bool,
    finished: bool,
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
        }
    }

    /// Decode one frame into the pending queue, surfacing a decode error in-band — the
    /// single home for a malformed-frame `Event::Error` on the stream (§8).
    fn decode_into_pending(&mut self, frame: Frame) {
        match self.proto.decode(frame, &mut self.state) {
            Ok(events) => self.pending.extend(events),
            Err(e) => self.pending.push_back(Event::Error(e)),
        }
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
                    self.pending
                        .push_back(Event::Error(transport_err("premature upstream EOF")));
                }
                self.finished = true;
                continue;
            }
            match self.body.next() {
                Some(Ok(chunk)) => {
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
