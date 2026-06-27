//! The framing-layer seams (sse-decoder spec §3, §4, §5): the one parsed `Frame`
//! every `decode` consumes, the `Framing` data enum a `Protocol` declares, and
//! the caller-owned `DecodeState` threaded through `decode`. The framers
//! themselves (SSE/NDJSON/Identity) and `Framing::decoder()` land with the
//! decoder task; this file owns the types they and `decode` meet at.

use std::collections::HashMap;
#[cfg(test)]
use std::str::Utf8Error;

use crate::canonical::{CanonicalError, ContentKind};

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
    /// The HTTP status of a non-2xx **whole-body error frame** (sse §9): `Some(code)`
    /// for the single frame the `run` loop hands `decode` when the handshake failed,
    /// `None` for every streaming frame (the SSE/NDJSON grammars never set it).
    /// `decode` reads `Some(status)` BOTH to know it is parsing an error envelope and
    /// to derive the error kind from the authoritative status
    /// (`ErrorKind::from_http_status`) — never reconstructed from the body's error
    /// strings. (`status.is_some()` is the old `whole_body` bit, now carrying its
    /// value too: the status had one true home, the response — so the frame carries
    /// it rather than forcing `decode` to guess it back from the body.)
    pub status: Option<u16>,
}

impl Frame {
    /// The payload bytes, written verbatim by the raw sink under `--raw` (sse §3).
    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }

    /// The payload as `&str` for a JSON parse. A malformed frame surfaces as the
    /// protocol's own `Provider` error, never a panic (sse §3). CLI-unreachable —
    /// the decoders parse via the shared `json` accessors, so this convenience is
    /// reached only by the in-crate tests; `#[cfg(test)]` keeps it off the release
    /// surface and out of its dead-code set (arch §9.8).
    #[cfg(test)]
    pub(crate) fn as_str(&self) -> Result<&str, Utf8Error> {
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
/// identifies the block. Fragments are emitted directly as `ContentDelta`s, never
/// buffered: a block carries only the identity needed to route deltas and to
/// synthesize its `ContentStop` at the terminal drain.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenBlock {
    pub kind: ContentKind,
}

/// The object-safe framer the `run` loop drives (sse §4). One local instance per
/// request; it holds the only cross-chunk BYTE buffer and never event state (that
/// is `DecodeState`). `Framing::decoder()` constructs the right one from DATA —
/// never a vendor branch. The framers live in `super::sse`.
pub trait Decoder {
    /// Feed one transport chunk; return every COMPLETE frame it now yields. May be
    /// empty — a chunk that only extends an open frame yields nothing. A partial
    /// frame is buffered, never an error (sse §4, §6.2): the `Result` leaves room
    /// for a future framing whose grammar can be structurally impossible.
    fn push(&mut self, chunk: Vec<u8>) -> Result<Vec<Frame>, CanonicalError>;

    /// Called once after the transport body drains: flush a final, boundary-char-
    /// unterminated complete frame (a real server quirk — sse §6.4, §7.2). A genuine
    /// partial is dropped and the framer NEVER fabricates a terminal marker, so
    /// `run`'s premature-EOF path fires on `!state.terminated` (sse §6.5).
    fn finish(&mut self) -> Result<Vec<Frame>, CanonicalError>;
}

/// Caller-owned cross-frame state, threaded by `&mut` into every `decode` (sse
/// §5). Keeping ALL cross-frame state here (never on the `Protocol` impl) is what
/// lets a protocol be a pure `(frame, state)` function shareable as `&'static dyn`.
#[derive(Debug, Default)]
pub struct DecodeState {
    /// In-flight blocks keyed by canonical index — the single source of truth for
    /// "which block a delta routes to" and "which are still open at finish."
    pub open: HashMap<u32, OpenBlock>,
    /// "Stream is over." Set TRUE by `decode` when it consumes the provider
    /// terminal marker — NEVER by the framer, NEVER on bare EOF (arch §3.5, CR-9).
    pub terminated: bool,
    /// Whether the synthesized `MessageStart` has been emitted yet. A protocol
    /// with a native message-start event (Anthropic) never reads this; one that
    /// must synthesize it on the first chunk (openai §3.3) gates emission on it.
    pub started: bool,
    /// Wire-positional block index → canonical content index (openai §3.1). Maps
    /// OpenAI's `tool_calls[].index` namespace onto the canonical index space so
    /// later argument fragments route to the block opened on first sight. Empty
    /// for protocols whose wire already speaks the canonical index (Anthropic).
    pub tool_index: HashMap<u32, u32>,
    /// Responses `(output_index, content_index)` → canonical content index
    /// (openai_responses §3.4). A single `message` output item streams ≥1 content
    /// parts (distinct `content_index`), so the canonical index keys off the PAIR,
    /// not the bare `output_index` (which would collide the parts). Assigned
    /// monotonically on first sight; the map only grows, so its `len` is the next
    /// index. Empty for protocols whose wire index is already canonical (Anthropic)
    /// or whose positional index lives in `tool_index` (openai chat).
    pub part_index: HashMap<(u32, u32), u32>,
    /// Accumulated `delta.refusal` text (openai §3.5), surfaced in the terminal
    /// `Finish{Refusal}`. Empty when no refusal field streamed.
    pub refusal: String,
}
