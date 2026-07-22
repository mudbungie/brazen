//! A reference transport delegate (transport spec §5) — the operator-selectable
//! seam, demonstrated in ~100 lines.
//!
//! Point a provider row at it and `bz` performs no HTTP of its own:
//!
//! ```toml
//! [[provider]]
//! name = "relayed"
//! base_url = "http://127.0.0.1:8080"
//! protocol = "anthropic_messages"
//! auth = "none"
//!
//!   [provider.transport]
//!   program = "target/debug/examples/stdio_transport"
//! ```
//!
//! The contract, whole: read ONE HTTP/1.1 request message from stdin (absolute-form
//! target, headers verbatim, body framed by stdin EOF), perform it, and write ONE
//! HTTP/1.1 response message to stdout (status line, headers, blank line, body
//! streamed to EOF). One request per process. Never retry — `bz` guarantees its
//! caller exactly one upstream request, and a retrying delegate would launder that
//! away. Never buffer the body to end — stream it, or a streamed generation stops
//! being one.
//!
//! It speaks **plaintext `http://` only**, deliberately: the whole point of the seam
//! is that the HTTPS/TLS stack is the operator's to choose (a real delegate wraps
//! `curl-impersonate`, Bun, Go, a corporate proxy library…). This one brings no
//! dependency at all, so the contract is legible without a TLS stack in the way.
//!
//! An example, not a binary: `cargo install brazen` never ships it.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn main() {
    if let Err(e) = relay() {
        // stderr is the delegate's diagnostic channel: `bz` folds it into the
        // transport error when the child exits nonzero (spec §6).
        eprintln!("stdio_transport: {e}");
        std::process::exit(1);
    }
}

fn relay() -> io::Result<()> {
    let mut request = Vec::new();
    io::stdin().read_to_end(&mut request)?;
    let (head, body) = split_head(&request)?;
    let mut lines = head.lines();
    let start = lines.next().unwrap_or_default();
    let url = start
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| io::Error::other("request line carries no target"))?;
    let (authority, path) = split_url(url)?;

    // The bounds ride the environment (spec §5.3), absent when unset.
    let secs = |name: &str| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .map(Duration::from_secs)
    };
    let mut socket = match secs("BZ_TRANSPORT_CONNECT_TIMEOUT") {
        Some(budget) => {
            let addr = std::net::ToSocketAddrs::to_socket_addrs(&authority)?
                .next()
                .ok_or_else(|| io::Error::other("host resolved to no address"))?;
            TcpStream::connect_timeout(&addr, budget)?
        }
        None => TcpStream::connect(&authority)?,
    };
    socket.set_read_timeout(secs("BZ_TRANSPORT_IDLE_TIMEOUT"))?;

    // THIS is the transport identity the operator owns: which headers are generated,
    // in what order and casing, and how the body is framed. `bz` supplied only the
    // application headers, verbatim and in order — everything else is ours.
    let mut out = format!("{} {path} HTTP/1.1\r\nHost: {authority}\r\n", verb(start)).into_bytes();
    for line in lines.filter(|l| !l.is_empty()) {
        out.extend_from_slice(line.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    out.extend_from_slice(b"Connection: close\r\n\r\n");
    out.extend_from_slice(body);
    socket.write_all(&out)?;
    socket.flush()?;

    // Response straight through, chunk by chunk — never drained to end, so a
    // streaming generation stays streaming (spec §5.2).
    let mut stdout = io::stdout();
    let mut buf = [0u8; 8192];
    loop {
        match socket.read(&mut buf)? {
            0 => return stdout.flush(),
            n => {
                stdout.write_all(&buf[..n])?;
                stdout.flush()?;
            }
        }
    }
}

/// The request line's method, defaulting to `GET` on an empty line (which
/// `split_head` has already made unreachable — total, not special-cased).
fn verb(start: &str) -> &str {
    start.split_whitespace().next().unwrap_or("GET")
}

/// Split the message at the blank line: `(head as text, body bytes)`. `CRLFCRLF` or
/// a bare `LFLF`, whichever ends first.
fn split_head(msg: &[u8]) -> io::Result<(String, &[u8])> {
    let lf = msg.windows(2).position(|w| w == b"\n\n").map(|i| i + 2);
    let crlf = msg.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
    let end = lf
        .into_iter()
        .chain(crlf)
        .min()
        .ok_or_else(|| io::Error::other("request head has no blank line"))?;
    Ok((
        String::from_utf8_lossy(&msg[..end]).into_owned(),
        &msg[end..],
    ))
}

/// `http://host:port/path?query` → `("host:port", "/path?query")`. An `https://`
/// target is refused: TLS is the operator's stack to bring, not this example's.
fn split_url(url: &str) -> io::Result<(String, String)> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| io::Error::other(format!("only http:// targets are relayed: {url}")))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let authority = match authority.contains(':') {
        true => authority.to_owned(),
        false => format!("{authority}:80"),
    };
    Ok((authority, path.to_owned()))
}
