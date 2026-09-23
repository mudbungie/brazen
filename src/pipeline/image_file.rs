//! A model-RETURNED image in text mode is a FILE, never stdout bytes (architecture
//! §5.3, bl-0987): the shared leaf both text sinks drive so their file bytes are
//! provably identical. Per open `Image` block the base64 fragments accumulate; at the
//! block's stop they decode ONCE and land at `<dir>/bz-<sha256 hex[..12]>.<ext>` —
//! content-addressed, so the name is pure (no clock, no counter, no flag), idempotent
//! for the same image, and never a collision for a different one. `dir` is the seam:
//! `run` passes `.` (the OS resolves it — the lib never reads the working directory),
//! tests pass a tempdir.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sha2::{Digest, Sha256};

use super::input::extension;

/// The open image blocks of one stream, keyed by canonical index, each holding its
/// media type (identity at `ContentStart`) and the base64 text so far.
pub(crate) struct ImageBlocks {
    dir: PathBuf,
    open: BTreeMap<u32, (String, String)>,
}

impl ImageBlocks {
    pub(crate) fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_owned(),
            open: BTreeMap::new(),
        }
    }

    /// `ContentStart{Image{media_type}}`: open the accumulator for `index`.
    pub(crate) fn start(&mut self, index: u32, media_type: &str) {
        self.open
            .insert(index, (media_type.to_owned(), String::new()));
    }

    /// `ImageDelta`: append a fragment (dropped when no image block is open at
    /// `index` — an `image_delta` only ever rides an `image` block).
    pub(crate) fn delta(&mut self, index: u32, frag: &str) {
        if let Some((_, b64)) = self.open.get_mut(&index) {
            b64.push_str(frag);
        }
    }

    /// `ContentStop`: take the block at `index` and write it, returning the path.
    /// `None` when no image block is open there (a text/tool stop) or when it carried
    /// NO fragments — a block truncated before its one frame arrived has nothing to
    /// decode, and a zero-byte file is not an image.
    pub(crate) fn stop(&mut self, index: u32) -> io::Result<Option<PathBuf>> {
        match self.open.remove(&index) {
            Some((media_type, b64)) if !b64.is_empty() => {
                write_image(&self.dir, &media_type, &b64).map(Some)
            }
            _ => Ok(None),
        }
    }

    /// The terminal flush (`Error`/`End`): an open block is DROPPED, never written —
    /// a partial base64 string decodes to a corrupt file, and the in-band `Error`
    /// already says why (interactive-output §5).
    pub(crate) fn drop_all(&mut self) {
        self.open.clear();
    }
}

/// `bz-<first 12 hex of sha256(bytes)>.<ext>` — the content-addressed name.
pub(crate) fn file_name(media_type: &str, bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("bz-{hex}.{}", extension(media_type))
}

/// Decode the whole base64 once (padding is only valid on the whole) and write the
/// bytes under `dir`. A malformed base64 is `InvalidData`; either failure is an
/// `io::Error` out of the sink's `write`, mapped by `pump` like any stdout failure.
pub(crate) fn write_image(dir: &Path, media_type: &str, b64: &str) -> io::Result<PathBuf> {
    let bytes = STANDARD
        .decode(b64)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let path = dir.join(file_name(media_type, &bytes));
    fs::write(&path, bytes)?;
    Ok(path)
}
