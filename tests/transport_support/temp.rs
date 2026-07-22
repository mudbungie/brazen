//! Self-deleting temp files for the transport suite: the `--config` a test points
//! `bz` at, and the tiny shell-script delegates the failure tests spawn. No
//! temp-file dependency — the suite already owns its own cleanup discipline.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct TempPath(PathBuf);

impl TempPath {
    pub fn to_str(&self) -> Option<&str> {
        self.0.to_str()
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl AsRef<Path> for TempPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

fn unique(ext: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("bz_transport_{}_{n}.{ext}", std::process::id()))
}

/// A temp `--config` file.
pub fn config(contents: &str) -> TempPath {
    let path = unique("toml");
    std::fs::write(&path, contents).expect("write temp config");
    TempPath(path)
}

/// An executable `/bin/sh` delegate stub — the failure-path transports. `sh` is the
/// one interpreter every unix has; the suite is `#![cfg(unix)]` for exactly this.
pub fn script(body: &str) -> TempPath {
    use std::os::unix::fs::PermissionsExt;
    let path = unique("sh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write delegate stub");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
        .expect("chmod delegate stub");
    TempPath(path)
}
