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

/// The shared SSE framer for every SSE-framed protocol (sse §6). Buffers bytes
/// across chunks; yields one `Frame` per blank-line-delimited event block.
#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
}

impl Decoder for SseDecoder {
    fn push(&mut self, chunk: Vec<u8>) -> Result<Vec<Frame>, CanonicalError> {
        self.buf.extend_from_slice(&chunk);
        let mut frames = Vec::new();
        // Peel each complete block (terminated by a blank line) off the FRONT of buf;
        // whatever remains is an incomplete frame, held for a future chunk (sse §6.2).
        while let Some(end) = find_frame_end(&self.buf) {
            let block: Vec<u8> = self.buf.drain(..end).collect();
            if let Some(frame) = parse_block(&block) {
                frames.push(frame);
            }
        }
        Ok(frames)
    }

    fn finish(&mut self) -> Result<Vec<Frame>, CanonicalError> {
        // Flush a final block the server left without its trailing blank line; a
        // genuine partial (no field lines) parses to None and is dropped (sse §6.4).
        let block = std::mem::take(&mut self.buf);
        Ok(parse_block(&block).into_iter().collect())
    }
}

/// The first index PAST a blank-line terminator (`\n\n` or `\r\n\r\n`) in `buf`,
/// else `None`. A byte scan: `\n` cannot fall inside a multi-byte UTF-8 sequence,
/// so the boundary is byte-exact wherever a chunk was cut (sse §6.3).
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
