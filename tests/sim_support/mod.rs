//! A simulated provider HTTP server for the end-to-end conformance suite (bl-7d5d).
//!
//! It replays a canned wire body — the golden `tests/fixtures/*.sse` / `*.ndjson`
//! captures — for ANY request on an ephemeral loopback port. A test points the real
//! `bz` binary at `http://127.0.0.1:PORT` (via a temp `--config`) and asserts the
//! normalized output, so the REAL `HttpTransport` (the `ureq` round-trip) is
//! exercised end to end — the one path `MockTransport` cannot reach. No real
//! provider, no key, no network beyond loopback; runs in plain `cargo test`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

/// A localhost HTTP/1.1 server that answers every request with one canned response.
/// The accept loop runs on a daemon thread that dies when the test process exits.
pub struct FakeProvider {
    port: u16,
}

impl FakeProvider {
    /// Bind an ephemeral `127.0.0.1` port and serve a `200 OK` with
    /// `(content_type, body)` verbatim for every request. Returns once bound, so the
    /// port is live before the caller launches `bz`.
    pub fn serve(content_type: &'static str, body: Vec<u8>) -> Self {
        Self::serve_status(200, content_type, body)
    }

    /// Like [`serve`](Self::serve) but with an arbitrary HTTP `status` — drives the
    /// real transport's non-2xx status→exit mapping (e.g. `401` → exit 77).
    pub fn serve_status(status: u16, content_type: &'static str, body: Vec<u8>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        thread::spawn(move || {
            // `.flatten()` skips a failed accept (a client that hung up mid-handshake)
            // and serves the next connection; a handler error is likewise ignored.
            for stream in listener.incoming().flatten() {
                let _ = handle(stream, status, content_type, &body);
            }
        });
        FakeProvider { port }
    }

    /// The `base_url` to drop into a provider row so `bz` targets this server.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// Drain the request (so the client's write half completes), then write the canned
/// `200` response. `Connection: close` makes the body's end unambiguous to `ureq`.
fn handle(
    mut stream: TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    drain_request(&mut stream)?;
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// Read the request head (up to the blank line) and its `Content-Length` body, so
/// the socket is fully consumed before we reply. Small requests only (test bodies).
fn drain_request(stream: &mut TcpStream) -> std::io::Result<()> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte)? == 0 {
            return Ok(()); // client closed before sending a full head
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head);
    let len = text
        .lines()
        .find_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    if len > 0 {
        let mut rest = vec![0u8; len];
        stream.read_exact(&mut rest)?;
    }
    Ok(())
}
