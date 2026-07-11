//! The native `--serve` bind seam (ingress §7): `std::net::TcpListener` behind
//! the lib's [`Bind`]/[`Listener`] traits, plus the replay stash's cache root.
//! Coverage-excluded with the rest of the shim; the accept loop itself lives in
//! the lib (`brazen::serve`) and is tested through in-memory doubles. The loop
//! runs until SIGINT/SIGTERM ends the process — the default dispositions, the
//! same convention as the rest of the binary (only SIGPIPE is touched, §5.8).

use std::io;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;

use brazen::{Bind, Listener, ServeConn};

/// `std::net::TcpListener::bind` behind the seam; `main` wires it, nothing else.
pub struct TcpBind;

impl Bind for TcpBind {
    fn bind(&self, addr: SocketAddr) -> io::Result<Box<dyn Listener>> {
        Ok(Box::new(TcpAccept(TcpListener::bind(addr)?)))
    }
}

struct TcpAccept(TcpListener);

impl Listener for TcpAccept {
    fn accept(&self) -> Option<Box<dyn ServeConn>> {
        loop {
            match self.0.accept() {
                Ok((stream, _)) => return Some(Box::new(stream)),
                // A transient per-connection failure (ECONNABORTED and kin) must
                // not kill the listener; a real accept failure ends the loop.
                Err(e) if e.kind() == io::ErrorKind::ConnectionAborted => {}
                Err(_) => return None,
            }
        }
    }
}

/// The replay stash's cache root (ingress §5): the same per-OS cache directory
/// the model cache lives under; a resolutionless environment degrades to the
/// temp dir — the stash is fail-open, so a bad root only costs replay fidelity.
pub fn stash_root() -> PathBuf {
    super::cache::cache_dir().unwrap_or_else(std::env::temp_dir)
}
