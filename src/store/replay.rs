//! The fail-open replay stash (ingress.md §5; the arch §2 "Not stateful"
//! exception): opaque reasoning-replay payloads a masquerade client's dialect
//! cannot carry — Anthropic thinking `signature`/`redacted_thinking`, OpenAI
//! `encrypted_content`, Google `thoughtSignature` — parked under
//! `<cache root>/brazen/replay/<key>` between turns. One file per key, no index,
//! no manifest: id is the path. A miss is `None`, never an error (absence
//! degrades fidelity, never correctness), which is what keeps this a true cache
//! and brazen honestly stateless. Per §6.5 the lib touches no ambient state: the
//! XDG cache root arrives injected (like every store seam) and pruning time
//! comes from the injected [`Clock`] — never `std::env`, never `now()`.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

use super::Clock;

/// Eviction horizon (ingress.md §5): entries whose mtime is older than this at
/// write time are unlinked. The mtime is the record — no manifest, no daemon.
const MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

/// Monotone per-process temp-name nonce: concurrent stashers of the SAME key
/// never share a temp file, so each `rename` publishes one whole payload
/// (last writer wins, readers see whole-old or whole-new, never a partial).
static TEMP_NONCE: AtomicU64 = AtomicU64::new(0);

/// The stash directory, rooted at the injected XDG cache root. Payloads are
/// opaque bytes to this module — the canonical-JSON block(s) for one turn.
pub struct ReplayStash {
    dir: PathBuf,
}

impl ReplayStash {
    /// Root the stash at `cache_root` (the injected `$XDG_CACHE_HOME`
    /// equivalent); the `brazen/replay` leaf is this module's one fact.
    pub fn new(cache_root: impl Into<PathBuf>) -> Self {
        ReplayStash {
            dir: cache_root.into().join("brazen").join("replay"),
        }
    }

    /// `key` as a path, or `None` when it cannot be a file name. Id is the path,
    /// and recall keys are echoed by the CLIENT (a tool-call id), so a separator
    /// or dot-prefix key is refused rather than resolved — a traversal key must
    /// not read or write outside the stash. Dot-prefixes are also the temp-file
    /// namespace. An unusable key is simply unstashable: recall misses (fail-open).
    fn path(&self, key: &str) -> Option<PathBuf> {
        let named = !key.is_empty() && !key.starts_with('.') && !key.contains(['/', '\\']);
        named.then(|| self.dir.join(key))
    }

    /// Write `payload` under `key`: temp name + `rename`, atomic and lock-free
    /// (ingress.md §5), then best-effort prune of stale siblings. `clock` is the
    /// §6.5-injected time source pruning compares mtimes against.
    pub fn stash(&self, key: &str, payload: &[u8], clock: &dyn Clock) -> io::Result<()> {
        let path = self.path(key).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("replay key {key:?} is not a file name"),
            )
        })?;
        fs::create_dir_all(&self.dir)?;
        let nonce = TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
        let tmp = self
            .dir
            .join(format!(".{}.{nonce}.tmp", std::process::id()));
        let mut file = fs::File::create(&tmp)?;
        file.write_all(payload)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        self.prune(clock.now());
        Ok(())
    }

    /// The stashed payload for `key`, or `None` for a missing (pruned, never
    /// written, unusable-key) entry — the fail-open path, never an error.
    pub fn recall(&self, key: &str) -> Option<Vec<u8>> {
        fs::read(self.path(key)?).ok()
    }

    /// Best-effort eviction on write: unlink every entry older than
    /// [`MAX_AGE_SECS`]. Crashed writers' orphaned `.tmp` files age out the same
    /// way. Every failure (racing unlink, unreadable metadata) is ignored — a
    /// pruned-or-not entry is at worst a stash miss, which fails open.
    fn prune(&self, now: u64) {
        // Result<ReadDir> -> entries -> Result<DirEntry> -> entries.
        for entry in fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            if let Some(stale) = stale_path(&entry, now) {
                let _ = fs::remove_file(stale);
            }
        }
    }
}

/// `entry`'s path iff its mtime is older than [`MAX_AGE_SECS`] before `now`.
/// Unreadable metadata is not stale (best-effort); a pre-epoch or future mtime
/// folds to age 0 via the saturating arithmetic.
fn stale_path(entry: &fs::DirEntry, now: u64) -> Option<PathBuf> {
    let modified = entry.metadata().ok()?.modified().ok()?;
    let secs = modified
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    (now.saturating_sub(secs) > MAX_AGE_SECS).then(|| entry.path())
}

/// The shared stash key for NON-tool assistant turns (ingress.md §5): a content
/// hash of the assistant text, the one stable thing the client echoes back.
/// BASE64URL-NOPAD(SHA-256(text)) — file-name-safe, and defined exactly once so
/// the ingress decoder and every egress encoder join on one derivation.
/// Tool-bearing turns key on the tool-call id instead (the caller's choice).
pub fn content_key(text: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(text.as_bytes()))
}
