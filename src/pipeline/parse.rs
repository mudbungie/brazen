//! Canonical-in parsing (§5.5, §8): decode a `CanonicalRequest` from any
//! `Read` — a real pipe or `--input FILE`, both the same `Box<dyn Read>`, so
//! file-vs-pipe parity is structural. Malformed JSON is a `ParseInput` error
//! (exit 64), never a panic on external input.

use std::io::Read;

use crate::canonical::{CanonicalError, CanonicalRequest, ErrorKind};

/// Parse canonical request bytes into the one authoritative `CanonicalRequest`.
/// Malformed input is `ErrorKind::ParseInput` (→ exit 64, §8); the serde
/// message rides `CanonicalError::message` so the failure is never silent and
/// never panics.
pub fn parse(reader: &mut dyn Read) -> Result<CanonicalRequest, CanonicalError> {
    serde_json::from_reader(reader).map_err(|e| CanonicalError {
        kind: ErrorKind::ParseInput,
        message: format!("malformed canonical request: {e}"),
        provider_detail: None,
    })
}
