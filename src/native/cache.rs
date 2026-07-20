//! The XDG-backed model cache (model-discovery §5.1, arch §6.5): one JSON file per
//! provider under `$XDG_CACHE_HOME/brazen/models/<provider>.json`, in the
//! `{"models":[{id,default}],"last_used":id}` shape (the `CachedModels` serde: the
//! `list-models --json` list plus the §4 rung-2 pointer beside it). The sibling of [`XdgCredStore`](super::creds): same atomic temp+rename,
//! but FORGIVING on read — a missing/corrupt/garbage file is `None`, never an error,
//! so the generation path degrades to `select_model`'s verbatim path and self-heals
//! on the next `bz --list-models`. Coverage-excluded with the rest of the `bz` shim.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use brazen::{CachedModels, ModelCache};

/// One JSON file per provider under `$XDG_CACHE_HOME/brazen/models`. `get` is `None`
/// on any miss or unreadable/unparseable file (the cold-cache path). `dir` is `None`
/// when no cache dir resolves — `get` then misses and `put` no-ops (best-effort).
pub struct XdgModelCache {
    pub(super) dir: Option<PathBuf>,
}

impl XdgModelCache {
    pub fn new() -> Self {
        XdgModelCache { dir: models_dir() }
    }

    fn path(&self, provider: &str) -> Option<PathBuf> {
        self.dir
            .as_ref()
            .map(|d| d.join(format!("{provider}.json")))
    }

    /// The atomic write half of `put`, returning its `io::Error` so the caller can
    /// warn once: create the dir, write a temp file, `rename` it into place (a
    /// concurrent reader sees the whole old or whole new file, never a partial). The
    /// document is `CachedModels`' own serde — never a second representation, so the
    /// pointer and the list are written and read by one shape.
    fn write(&self, provider: &str, cached: &CachedModels) -> io::Result<()> {
        let path = self
            .path(provider)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no cache dir for models"))?;
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no models directory"))?;
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{provider}.json.tmp"));
        let bytes = serde_json::to_vec_pretty(cached)?;
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, &path)
    }
}

impl ModelCache for XdgModelCache {
    fn get(&self, provider: &str) -> Option<CachedModels> {
        let bytes = fs::read(self.path(provider)?).ok()?;
        // Project the `{"models":[…],"last_used":…}` document back through its own serde
        // — a missing `models` key, a wrong shape, or garbage is `None` (forgiving). An
        // absent/null `last_used` is `None` by `serde(default)`, so a pre-pointer cache
        // reads clean and §4 simply falls through to the provider's suggestion.
        serde_json::from_slice(&bytes).ok()
    }

    fn put(&self, provider: &str, cached: &CachedModels) {
        if let Err(e) = self.write(provider, cached) {
            eprintln!("warning: could not write model cache for {provider}: {e}");
        }
    }
}

/// `$XDG_CACHE_HOME/brazen/models` (Unix), `~/Library/Caches/brazen/models` (macOS),
/// `%LOCALAPPDATA%\brazen\models` (Windows) — the cache sibling of `credentials_dir`.
fn models_dir() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("brazen").join("models"))
}

#[cfg(target_os = "macos")]
pub(super) fn cache_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("Caches"))
}

#[cfg(target_os = "windows")]
pub(super) fn cache_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn cache_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
}
