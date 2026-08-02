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
use std::sync::mpsc::{Receiver, Sender};
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
        Self::serve_inner(200, content_type, body, None)
    }

    /// Like [`serve`](Self::serve) but with an arbitrary HTTP `status` — drives the
    /// real transport's non-2xx status→exit mapping (e.g. `401` → exit 77).
    pub fn serve_status(status: u16, content_type: &'static str, body: Vec<u8>) -> Self {
        Self::serve_inner(status, content_type, body, None)
    }

    /// Like [`serve`](Self::serve) but also handing back each request's BODY on the
    /// returned channel, so a test can assert what actually hit the wire (the
    /// `sim_media` suite pins the `-f` attachment's encoded form). The reply is
    /// written only after the send, so by the time `bz` has exited every request it
    /// made is already in the channel — `try_recv` after the wait is race-free.
    pub fn capture(content_type: &'static str, body: Vec<u8>) -> (Self, Receiver<Vec<u8>>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Self::serve_inner(200, content_type, body, Some(tx)), rx)
    }

    fn serve_inner(
        status: u16,
        content_type: &'static str,
        body: Vec<u8>,
        capture: Option<Sender<Vec<u8>>>,
    ) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        thread::spawn(move || {
            // `.flatten()` skips a failed accept (a client that hung up mid-handshake)
            // and serves the next connection; a handler error is likewise ignored.
            for stream in listener.incoming().flatten() {
                let _ = handle(stream, status, content_type, &body, capture.as_ref());
            }
        });
        FakeProvider { port }
    }

    /// The `base_url` to drop into a provider row so `bz` targets this server.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// One simulated provider: its registry shape (as a `--config` row) and the golden
/// fixture the fake server replays. ONE table for every sim suite (`sim_conformance`,
/// `sim_media`) — the row data lives here so the two cannot drift.
pub struct Sim {
    /// Provider row name + the `--provider` value.
    pub name: &'static str,
    pub protocol: &'static str,
    pub auth: &'static str,
    /// The full `api_header = { … }` TOML line, or `""` for keyless (`auth = "none"`).
    pub api_header: &'static str,
    /// Extra row lines (e.g. `body_defaults` / `beta_headers`), or `""`.
    pub extra: &'static str,
    pub model: &'static str,
    pub fixture: &'static str,
    pub content_type: &'static str,
}

pub const PROVIDERS: &[Sim] = &[
    Sim {
        name: "anthropic",
        protocol: "anthropic_messages",
        auth: "api_key",
        api_header: r#"api_header = { name = "x-api-key", scheme = "raw" }"#,
        extra: "body_defaults = { max_tokens = 4096 }",
        model: "claude-sim",
        fixture: "anthropic_messages_basic.sse",
        content_type: "text/event-stream",
    },
    Sim {
        name: "openai",
        protocol: "openai_chat",
        auth: "bearer",
        api_header: r#"api_header = { name = "Authorization", scheme = "bearer" }"#,
        extra: "",
        model: "gpt-sim",
        fixture: "openai_chat_basic.sse",
        content_type: "text/event-stream",
    },
    Sim {
        name: "openai-responses",
        protocol: "openai_responses",
        auth: "bearer",
        api_header: r#"api_header = { name = "Authorization", scheme = "bearer" }"#,
        extra: "",
        model: "gpt-sim",
        fixture: "openai_responses_basic.sse",
        content_type: "text/event-stream",
    },
    Sim {
        name: "google",
        protocol: "google_generative_ai",
        auth: "api_key",
        api_header: r#"api_header = { name = "x-goog-api-key", scheme = "raw" }"#,
        extra: "",
        model: "gemini-sim",
        fixture: "google_genai_basic.sse",
        content_type: "text/event-stream",
    },
    Sim {
        name: "ollama",
        protocol: "ollama_chat",
        auth: "none",
        api_header: "",
        extra: "",
        model: "llama-sim",
        fixture: "ollama_chat_basic.ndjson",
        content_type: "application/x-ndjson",
    },
];

/// Read a golden fixture from `tests/fixtures/`.
pub fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {path}: {e}"))
}

/// Write a single-provider config whose `base_url` targets `server`, returning the
/// kept-alive temp file (dropping it deletes the file).
pub fn config_for(p: &Sim, base_url: &str) -> tempfile::NamedTempFile {
    let body = format!(
        "[[provider]]\nname = \"{}\"\nbase_url = \"{}\"\nprotocol = \"{}\"\nauth = \"{}\"\n{}\n{}\n",
        p.name, base_url, p.protocol, p.auth, p.api_header, p.extra
    );
    let mut f = tempfile::NamedTempFile::new().expect("temp config");
    f.write_all(body.as_bytes()).expect("write config");
    f.flush().expect("flush config");
    f
}

/// Drain the request (so the client's write half completes), hand its body to
/// `capture` if asked, then write the canned response. `Connection: close` makes
/// the body's end unambiguous to `ureq`.
fn handle(
    mut stream: TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    capture: Option<&Sender<Vec<u8>>>,
) -> std::io::Result<()> {
    let request = drain_request(&mut stream)?;
    if let Some(tx) = capture {
        let _ = tx.send(request); // a dropped receiver is the test's choice, not an error
    }
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// Read the request head (up to the blank line) and its `Content-Length` body, so
/// the socket is fully consumed before we reply; the body is returned for capture.
/// Small requests only (test bodies).
fn drain_request(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte)? == 0 {
            return Ok(Vec::new()); // client closed before sending a full head
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
    let mut rest = vec![0u8; len];
    stream.read_exact(&mut rest)?;
    Ok(rest)
}
