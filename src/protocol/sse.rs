//! The shared transport framers (sse-decoder spec §6–§8): `SseDecoder`,
//! `NdjsonDecoder`, `IdentityDecoder`, and `Framing::decoder()`. Each cuts a byte
//! stream at the right boundary and hands `decode` complete `Frame`s; none knows
//! events, `terminated`, or which provider. The byte buffer is their ONLY state —
//! the §10 determinism contract falls out of "emit a frame only once complete."

use crate::canonical::CanonicalError;

use super::frame::{Decoder, Frame, Framing};

impl Framing {
    /// Construct the framer for a SUCCESSFUL (2xx) stream (sse §4). A non-2xx body
    /// bypasses framing as one whole-body frame (sse §9), so this only builds on
    /// the streaming path — a map of three over DATA, not a vendor branch.
    pub fn decoder(self) -> Box<dyn Decoder> {
        match self {
            Framing::Sse => Box::new(SseDecoder::default()),
            Framing::Ndjson => Box::new(NdjsonDecoder::default()),
            Framing::Identity => Box::new(IdentityDecoder),
        }
    }
}

/// A leading UTF-8 byte-order mark. WHATWG SSE requires ONE such mark at stream
/// start to be ignored (sse §6.1); a mid-stream occurrence is ordinary data.
const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// The shared SSE framer for every SSE-framed protocol (sse §6). Buffers bytes
/// across chunks; yields one `Frame` per blank-line-delimited event block. The byte
/// buffer is the state; two scalars are pure bookkeeping OVER it (they hold no event
/// state — sse §5): `scan` is the terminator-search resume offset that keeps framing
/// linear across pushes, and `bom_stripped` records that the one-time leading-BOM
/// decision (§6.1) is settled.
#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
    /// The index into `buf` from which the next blank-line search resumes. Every byte
    /// before it is already known to begin no `\n\n`/`\r\n\r\n`, so a frame that never
    /// terminates is scanned once as it arrives, not re-scanned from 0 on every push
    /// (the O(n^2) blowup on a hostile never-blank stream — sse §6.2).
    scan: usize,
    /// Whether the leading-BOM check has run. A stream-start UTF-8 BOM (`EF BB BF`) is
    /// stripped exactly once per WHATWG SSE (§6.1); thereafter `buf` is framed verbatim
    /// (a later `EF BB BF` is a data byte sequence, never a BOM).
    bom_stripped: bool,
}

impl Decoder for SseDecoder {
    fn push(&mut self, chunk: Vec<u8>) -> Result<Vec<Frame>, CanonicalError> {
        self.buf.extend_from_slice(&chunk);
        if !self.strip_bom() {
            return Ok(Vec::new()); // still buffering a possible leading BOM (§6.1)
        }
        let mut frames = Vec::new();
        // Peel each complete block (terminated by a blank line) off the FRONT of buf,
        // resuming the search at `self.scan` so bytes already proven blank-line-free are
        // not rescanned (sse §6.2); whatever remains is an incomplete frame held for a
        // future chunk.
        while let Some(rel) = find_frame_end(&self.buf[self.scan..]) {
            let end = self.scan + rel;
            let block: Vec<u8> = self.buf.drain(..end).collect();
            self.scan = 0; // the front shifted; scan the remainder from its start
            if let Some(frame) = parse_block(&block) {
                frames.push(frame);
            }
        }
        // No terminator from `self.scan` on: resume 3 bytes back next push so a
        // `\r\n\r\n` straddling the chunk boundary is caught (its longest incomplete
        // prefix, `\r\n\r`, is 3 bytes).
        self.scan = self.buf.len().saturating_sub(3);
        Ok(frames)
    }

    fn finish(&mut self) -> Result<Vec<Frame>, CanonicalError> {
        // Flush a final block the server left without its trailing blank line; a
        // genuine partial (no field lines) parses to None and is dropped (sse §6.4).
        let block = std::mem::take(&mut self.buf);
        Ok(parse_block(&block).into_iter().collect())
    }
}

impl SseDecoder {
    /// The one-time WHATWG leading-BOM decision (§6.1). Returns `true` once settled
    /// (BOM removed, or ruled out) so `push` may frame; returns `false` while `buf` is
    /// still only a proper prefix of `EF BB BF` and the next byte could complete or
    /// break it — the caller buffers and waits, which is why a BOM split byte-by-byte
    /// (the §10 one-byte rechunking) still strips cleanly. Idempotent once `true`.
    fn strip_bom(&mut self) -> bool {
        if self.bom_stripped {
            return true;
        }
        if self.buf.len() < BOM.len() && BOM.starts_with(&self.buf) {
            return false; // an incomplete BOM prefix so far — await more bytes
        }
        if self.buf.starts_with(BOM) {
            self.buf.drain(..BOM.len());
        }
        self.bom_stripped = true;
        true
    }
}

/// The first index PAST a blank-line terminator (`\n\n` or `\r\n\r\n`) in the given
/// slice, else `None` — the caller passes `buf[scan..]` and offsets the result. A byte
/// scan: `\n` cannot fall inside a multi-byte UTF-8 sequence, so the boundary is
/// byte-exact wherever a chunk was cut (sse §6.3).
fn find_frame_end(buf: &[u8]) -> Option<usize> {
    (0..buf.len()).find_map(|i| {
        if buf[i..].starts_with(b"\n\n") {
            Some(i + 2)
        } else if buf[i..].starts_with(b"\r\n\r\n") {
            Some(i + 4)
        } else {
            None
        }
    })
}

/// Parse one complete block into a `Frame` per the §6.1 field rules. `Some` if the
/// block carried any `data:` or `event:`; `None` for a pure-comment keep-alive.
/// Runs only on complete blocks, so every `data:` byte is whole UTF-8 (sse §6.3).
fn parse_block(block: &[u8]) -> Option<Frame> {
    let mut event: Option<String> = None;
    let mut data: Vec<Vec<u8>> = Vec::new();
    for raw_line in block.split(|&b| b == b'\n') {
        let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        // A line with no colon (blank line, bare token) contributes nothing.
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            continue;
        };
        let field = &line[..colon];
        let value = &line[colon + 1..];
        let value = value.strip_prefix(b" ").unwrap_or(value);
        match field {
            b"event" => event = Some(String::from_utf8_lossy(value).into_owned()),
            b"data" => data.push(value.to_vec()),
            _ => {} // `:` comment (empty field), `id:`, `retry:` — ignored.
        }
    }
    if event.is_none() && data.is_empty() {
        None
    } else {
        Some(Frame {
            event,
            data: data.join(&b'\n'),
            status: None,
        })
    }
}

/// The NDJSON line-framer (sse §7, Ollama): one JSON object per `\n`-terminated
/// line, no `event:`/`data:` grammar.
#[derive(Default)]
pub struct NdjsonDecoder {
    buf: Vec<u8>,
}

impl Decoder for NdjsonDecoder {
    fn push(&mut self, chunk: Vec<u8>) -> Result<Vec<Frame>, CanonicalError> {
        self.buf.extend_from_slice(&chunk);
        let mut frames = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=nl).collect();
            line.pop(); // drop the trailing `\n`
            if let Some(frame) = line_frame(line) {
                frames.push(frame);
            }
        }
        Ok(frames) // partial last line stays in buf
    }

    fn finish(&mut self) -> Result<Vec<Frame>, CanonicalError> {
        // A final line lacking its `\n` (server closed without a trailing newline)
        // is a complete frame; a blank tail is nothing (sse §7).
        let line = std::mem::take(&mut self.buf);
        Ok(line_frame(line).into_iter().collect())
    }
}

/// One NDJSON line → a `Frame`, stripping a trailing `\r`; a blank line is no frame.
fn line_frame(mut line: Vec<u8>) -> Option<Frame> {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    if line.is_empty() {
        None
    } else {
        Some(Frame {
            event: None,
            data: line,
            status: None,
        })
    }
}

/// The Identity framer (sse §8, `--raw`): stateless and lossless — each transport
/// chunk becomes exactly one `Frame` verbatim, in arrival order. No boundary scan,
/// no UTF-8 validation, no terminal-marker recognition.
pub struct IdentityDecoder;

impl Decoder for IdentityDecoder {
    fn push(&mut self, chunk: Vec<u8>) -> Result<Vec<Frame>, CanonicalError> {
        Ok(vec![Frame {
            event: None,
            data: chunk,
            status: None,
        }])
    }

    fn finish(&mut self) -> Result<Vec<Frame>, CanonicalError> {
        Ok(vec![]) // nothing buffered, nothing to flush
    }
}
