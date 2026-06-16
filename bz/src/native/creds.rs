//! The XDG-backed credential store (arch §6.4, auth §5.2): one 0600 JSON file per
//! provider, written atomically (temp file created 0600, then `rename`) so a
//! concurrent reader sees either the whole old or whole new file, never a partial
//! write. Coverage-excluded with the rest of the `bz` shim; its IO invariants are
//! pinned by `super::tests`.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use brazen::{Cred, CredStore};

/// One 0600 JSON file per provider under the platform data dir. `get` is `None` on
/// any miss (the no-creds path). The `dir` field is `pub(super)` so `super::tests`
/// can root the real store at a tempdir, bypassing the env lookup.
pub struct XdgCredStore {
    pub(super) dir: Option<PathBuf>,
}

impl XdgCredStore {
    pub fn new() -> Self {
        XdgCredStore {
            dir: credentials_dir(),
        }
    }

    fn path(&self, provider: &str) -> Option<PathBuf> {
        self.dir
            .as_ref()
            .map(|d| d.join(format!("{provider}.json")))
    }
}

impl CredStore for XdgCredStore {
    fn get(&self, provider: &str) -> Option<Cred> {
        let bytes = fs::read(self.path(provider)?).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn put(&self, provider: &str, cred: &Cred) -> io::Result<()> {
        let path = self.path(provider).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no data dir for credentials")
        })?;
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no credentials directory"))?;
        fs::create_dir_all(dir)?;
        set_dir_mode(dir)?;
        let tmp = dir.join(format!(".{provider}.json.tmp"));
        let bytes = serde_json::to_vec_pretty(cred)?;
        write_owner_only(&tmp, &bytes)?;
        fs::rename(&tmp, &path)
    }
}

/// Create `path` 0600 at create time on Unix (never a create-then-chmod window),
/// write `bytes`, and `sync_all` before the caller renames it into place.
fn write_owner_only(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// The credentials directory is `0700` on Unix; the user-profile ACL stands
/// elsewhere (a documented limitation, auth §5.2).
#[cfg(unix)]
fn set_dir_mode(dir: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_dir_mode(_dir: &std::path::Path) -> io::Result<()> {
    Ok(())
}

/// `$XDG_DATA_HOME/brazen/credentials` (Unix), `~/Library/Application
/// Support/brazen/credentials` (macOS), `%APPDATA%\brazen\credentials` (Windows).
fn credentials_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("brazen").join("credentials"))
}

#[cfg(target_os = "macos")]
fn data_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("Application Support"))
}

#[cfg(target_os = "windows")]
fn data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn data_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
}
