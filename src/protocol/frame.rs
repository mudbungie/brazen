//! The framing-layer seams (sse-decoder spec §3, §4, §5): the one parsed `Frame`
//! every `decode` consumes, the `Framing` data enum a `Protocol` declares, and
//! the caller-owned `DecodeState` threaded through `decode`. The framers
//! themselves (SSE/NDJSON/Identity) and `Framing::decoder()` land with the
//! decoder task; this file owns the types they and `decode` meet at.

use std::collections::HashMap;
use std::str::Utf8Error;

use crate::canonical::{ContentKind, Usage};

/// One complete, framing-stripped unit handed to `Protocol::decode` (sse §3).
/// Identical across SSE / NDJSON / Identity — the framing is spent producing it,
/// so `decode` never asks which framer produced a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// The SSE `event:` value, if any; `None` for NDJSON/Identity and event-less
    /// SSE blocks. Envelope data a protocol MAY consult; never load-bearing here.
    pub event: Option<String>,
    /// The framing-stripped payload bytes. `Vec<u8>` (not `String`) so Identity
    /// passes raw bytes through and the framer can hold a partial-UTF-8 tail;
    /// every emitted frame's `data` is nonetheless complete UTF-8 for SSE/NDJSON.
    pub data: Vec<u8>,
    /// Set only for a non-2xx whole-body error frame (sse §9); the SSE/NDJSON
    /// grammars never set it. `decode` reads it to parse an error envelope.
    pub whole_body: bool,
}

impl Frame {
    /// The payload bytes, written verbatim by the raw sink under `--raw` (sse §3).
    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }

    /// The payload as `&str` for a JSON parse. A malformed frame surfaces as the
    /// protocol's own `Provider` error, never a panic (sse §3).
    pub fn as_str(&self) -> Result<&str, Utf8Error> {
        std::str::from_utf8(&self.data)
    }
}

/// The transport framing a `Protocol` declares as DATA (arch §4.1, sse §4). The
/// only enum this layer matches on — a map of three over the protocol's own
/// choice, not a vendor branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
    Sse,
    Ndjson,
    Identity,
}

/// One in-flight content block's cross-frame state (sse §5). The map (on
/// `DecodeState`) is owned by this layer; the value is protocol-shaped — `kind`
/// identifies the block, `buffer` accumulates streamed fragments (tool-arg JSON /
/// thinking text) until the block closes.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenBlock {
    pub kind: ContentKind,
    pub buffer: String,
}

/// Caller-owned cross-frame state, threaded by `&mut` into every `decode` (sse
/// §5). Keeping ALL cross-frame state here (never on the `Protocol` impl) is what
/// lets a protocol be a pure `(frame, state)` function shareable as `&'static dyn`.
#[derive(Debug, Default)]
pub struct DecodeState {
    /// In-flight blocks keyed by canonical index — the single source of truth for
    /// "which block a delta routes to" and "which are still open at finish."
    pub open: HashMap<u32, OpenBlock>,
    /// Cumulative usage as last revealed by the wire — a last-wins-per-field
    /// snapshot re-emitted via `Event::Usage`, not an accumulator.
    pub usage: Usage,
    /// "Stream is over." Set TRUE by `decode` when it consumes the provider
    /// terminal marker — NEVER by the framer, NEVER on bare EOF (arch §3.5, CR-9).
    pub terminated: bool,
}
