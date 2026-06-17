//! Process + network plumbing for the live suite (bl-04dc): spawn the real `bz`
//! binary, TCP-probe a keyless endpoint, and locate a stored credential — the
//! impure edges the black-box harness drives. No lib linkage.

use std::io::Write;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Spawn `bz` with `args`, feed `stdin_body`, return (exit, stdout, stderr).
pub fn run_bz(args: &[String], stdin_body: &str) -> (i32, String, String) {
    let bz = env!("CARGO_BIN_EXE_bz");
    let mut child = Command::new(bz)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn `bz`");
    child
        .stdin
        .take()
        .expect("bz stdin")
        .write_all(stdin_body.as_bytes())
        .expect("write request to bz");
    let out = child.wait_with_output().expect("wait for `bz`");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// TCP-connect probe for a keyless `host:port` — dependency-free readiness, like
/// `ollama_smoke.rs`. Any resolve/connect failure → not reachable → skip.
pub fn connectable(addr: &str) -> bool {
    use std::net::ToSocketAddrs;
    addr.to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .and_then(|a| TcpStream::connect_timeout(&a, Duration::from_secs(3)).ok())
        .is_some()
}

/// `<data>/brazen/credentials/<provider>.json` if it exists, mirroring
/// `XdgCredStore`'s path (`$XDG_DATA_HOME` else `~/.local/share`). Presence ⇒ a
/// stored `Cred` (an OAuth2 login, etc.) `bz` will read for itself.
pub fn cred_file(provider: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    let p = base
        .join("brazen")
        .join("credentials")
        .join(format!("{provider}.json"));
    p.exists().then_some(p)
}
