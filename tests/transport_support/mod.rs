//! The two conformance INSTRUMENTS (transport spec §8) plus the plumbing that
//! points the real `bz` binary at them.
//!
//! - [`HttpObserver`] records what a server actually receives: the request head
//!   VERBATIM — request line, every header name in its own casing and order — then
//!   answers one canned response.
//! - [`clienthello`] records the TLS first flight and projects it onto a JA3-form
//!   fingerprint. No handshake is completed, so the instrument needs no certificate
//!   and no TLS server: the ClientHello IS the fingerprint.
//!
//! Neither instrument infers the other's layer (spec §2.2): the application
//! projection is taken from the observed bytes by [`HttpObservation::application`],
//! and the transport projection by [`HttpObservation::transport`], from the same
//! capture, so the difference between the layers is exactly the normalization.
#![allow(dead_code)]

pub mod temp;
pub mod tls;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{channel, Receiver};
use std::thread;

/// One captured request head, verbatim, plus the body bytes that followed it.
pub struct HttpObservation {
    pub head: String,
    pub body: Vec<u8>,
}

impl HttpObservation {
    /// The APPLICATION-wire projection (spec §8.1): method, target PATH, the headers
    /// Brazen itself put on the `WireRequest` (lower-cased, sorted — order is a
    /// transport fact, not an application one), and the body. Everything the client
    /// stack generated is dropped, because none of it is the application's.
    pub fn application(&self, brazen_headers: &[&str]) -> String {
        let mut lines = self.head.lines();
        let start = lines.next().unwrap_or_default();
        let mut parts: Vec<String> = start
            .split_whitespace()
            .take(2)
            .map(str::to_owned)
            .collect();
        let mut headers: Vec<String> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.trim().to_lowercase(), v.trim().to_owned()))
            .filter(|(n, _)| brazen_headers.contains(&n.as_str()))
            .map(|(n, v)| format!("{n}: {v}"))
            .collect();
        headers.sort();
        parts.append(&mut headers);
        parts.push(String::from_utf8_lossy(&self.body).into_owned());
        parts.join("\n")
    }

    /// The TRANSPORT-wire projection (spec §8.2): the head VERBATIM — casing, order,
    /// HTTP version, framing headers and every generated header included — with the
    /// ephemeral port normalized so the capture is stable across runs.
    pub fn transport(&self, port: u16) -> String {
        self.head.replace(&port.to_string(), "PORT")
    }
}

/// A loopback HTTP server that records the first request it receives and answers it
/// with one canned response.
pub struct HttpObserver {
    pub port: u16,
    rx: Receiver<HttpObservation>,
}

impl HttpObserver {
    /// Bind an ephemeral `127.0.0.1` port; every connection gets `status` with
    /// `body`, and the first request's head/body are recorded for [`Self::observed`].
    pub fn start(status: u16, extra_header: &'static str, body: &'static str) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let (tx, rx) = channel();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let Ok(observation) = read_request(&mut stream) else {
                    continue;
                };
                let _ = tx.send(observation);
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: text/event-stream\r\n\
                     Content-Length: {}\r\n{extra_header}Connection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body.as_bytes());
                let _ = stream.flush();
            }
        });
        HttpObserver { port, rx }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// The recorded request. Panics if none arrived — a silent miss would make an
    /// equality assertion vacuous.
    pub fn observed(&self) -> HttpObservation {
        self.rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("no request reached the observer")
    }
}

/// Read one request head plus its `Content-Length` body, verbatim.
fn read_request(stream: &mut std::net::TcpStream) -> std::io::Result<HttpObservation> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && !head.ends_with(b"\n\n") {
        if stream.read(&mut byte)? == 0 {
            break;
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let len = text
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(n, _)| n.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut body)?;
    }
    Ok(HttpObservation {
        head: text.trim_end().to_owned(),
        body,
    })
}

/// A loopback listener that captures the TLS **first flight** and hands back the raw
/// ClientHello bytes, then drops the connection. No certificate, no handshake, no TLS
/// server implementation — the fingerprint lives entirely in the client's first
/// record (spec §8.2). Returns `(port, receiver)`.
pub fn clienthello_observer() -> (u16, Receiver<Vec<u8>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let (tx, rx) = channel();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            buf.truncate(n);
            let _ = tx.send(buf);
        }
    });
    (port, rx)
}
