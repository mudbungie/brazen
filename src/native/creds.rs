//! The XDG-backed credential store (arch §6.4, auth §5.2): one 0600 JSON file per
//! provider, written atomically (temp file created 0600, then `rename`) so a
//! concurrent reader sees either the whole old or whole new file, never a partial
//! write. Coverage-excluded with the rest of the `bz` shim; its IO invariants are
//! pinned by `super::tests`.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use brazen::{parse_ambient, AmbientFormat, AmbientSpec, Cred, CredStore};

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

    /// Read a foreign credential source named by `spec` into a brazen `Cred` (auth
    /// §5.5). The format picks the SOURCE the shim reads (its impurity, like
    /// `restore_sigpipe`): a file (`~/` expanded against `$HOME`) for `ClaudeCode`, or
    /// the process env var `spec.path` names for `ApiKeyEnv` — both then handed to the
    /// pure `parse_ambient`. Any miss — no `$HOME`, no file/var, foreign/malformed
    /// contents — is `None`, the no-creds path like `get`.
    fn discover(&self, spec: &AmbientSpec) -> Option<Cred> {
        let bytes = match spec.format {
            AmbientFormat::ApiKeyEnv => std::env::var(&spec.path).ok()?.into_bytes(),
            AmbientFormat::ClaudeCode => fs::read(expand_home(&spec.path)?).ok()?,
        };
        parse_ambient(spec.format, &bytes)
    }
}

/// Expand a leading `~/` in an ambient credential path against `$HOME` (auth §5.5):
/// the lone place this ambient env input is read, then delegated to the pure
/// [`expand_home_with`] so the policy is testable without touching process env.
pub(super) fn expand_home(path: &str) -> Option<PathBuf> {
    expand_home_with(path, std::env::var_os("HOME"))
}

/// Pure tilde/home expansion: `~/` joins `home` (the caller's `$HOME` value),
/// `None` when `~/` is named but `home` is `None` (so discovery degrades to the
/// no-creds path), an absolute/relative path passed through verbatim otherwise.
pub(super) fn expand_home_with(path: &str, home: Option<std::ffi::OsString>) -> Option<PathBuf> {
    match path.strip_prefix("~/") {
        Some(rest) => home.map(|h| PathBuf::from(h).join(rest)),
        None => Some(PathBuf::from(path)),
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
