//! Hand-rolled minimal HTTP/1.1 (ingress.md §7): request line + headers +
//! `Content-Length` body in; status line + headers + body out — a
//! `Content-Length` aggregate, or chunked SSE flushed per event so frames reach
//! the client as they flow. No TLS, no HTTP/2, no multipart: the clients are
//! well-behaved SDKs on localhost, and a server framework would be a deep
//! dependency for this shallow need. Parsing is total over bytes — every
//! malformed shape is an `Err(detail)` the connection loop re-encodes as the
//! dialect's 400 envelope (§9) before closing the connection.

use std::io::{BufRead, Write};

use super::Respond;

/// One parsed request. Header names arrive lowercased (HTTP headers are
/// case-insensitive; one spelling at rest beats a per-read fold).
pub(super) struct HttpRequest {
    pub method: String,
    pub path: String,
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// The first value of `name` (already-lowercased ask), or `None`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Did the client ask to close after this response? (HTTP/1.1 defaults to
    /// keep-alive; `Connection: close` is the explicit opt-out.)
    pub fn wants_close(&self) -> bool {
        self.header("connection")
            .is_some_and(|v| v.eq_ignore_ascii_case("close"))
    }
}

/// Read one request off a keep-alive connection: `Ok(None)` is a clean EOF
/// between requests (the client hung up — the keep-alive loop's exit), any
/// malformed shape is `Err(detail)` for the §9 400 envelope.
pub(super) fn read_request(r: &mut dyn BufRead) -> Result<Option<HttpRequest>, String> {
    let mut line = String::new();
    if read_line(r, &mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (method, path, version) = (parts.next(), parts.next(), parts.next());
    let (Some(method), Some(path), Some(version)) = (method, path, version) else {
        return Err(format!("bad request line `{}`", line.trim_end()));
    };
    if !version.starts_with("HTTP/1") {
        return Err(format!("unsupported protocol version `{version}`"));
    }
    let (method, path) = (method.to_owned(), path.to_owned());
    let mut headers = Vec::new();
    loop {
        line.clear();
        if read_line(r, &mut line)? == 0 {
            return Err("connection closed inside the header block".into());
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        let Some((name, value)) = trimmed.split_once(':') else {
            return Err(format!("bad header line `{trimmed}`"));
        };
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
    }
    let req = HttpRequest {
        method,
        path,
        headers,
        body: Vec::new(),
    };
    let length: usize = match req.header("content-length") {
        None => 0,
        Some(v) => v.parse().map_err(|_| format!("bad content-length `{v}`"))?,
    };
    let mut body = vec![0; length];
    r.read_exact(&mut body)
        .map_err(|e| format!("body shorter than content-length: {e}"))?;
    Ok(Some(HttpRequest { body, ..req }))
}

/// A line read whose IO failure (including invalid UTF-8) is a malformed-HTTP
/// detail, not a panic; the byte count distinguishes EOF from an empty line.
fn read_line(r: &mut dyn BufRead, buf: &mut String) -> Result<usize, String> {
    r.read_line(buf).map_err(|e| format!("read failed: {e}"))
}

/// The HTTP half of [`Respond`] (ingress §7, §10): SSE begins immediately —
/// status line, `text/event-stream`, chunked — and each event's bytes go out as
/// one flushed chunk; the aggregate shape buffers the folded body so the
/// `Content-Length` header can precede it. `dead()` reports a write failure so
/// the connection loop can stop reusing the socket.
pub(super) struct HttpRespond<'a> {
    w: &'a mut dyn Write,
    sse: bool,
    status: u16,
    body: Vec<u8>,
    dead: bool,
}

impl<'a> HttpRespond<'a> {
    pub fn new(w: &'a mut dyn Write) -> Self {
        HttpRespond {
            w,
            sse: false,
            status: 200,
            body: Vec::new(),
            dead: false,
        }
    }

    /// Did any write fail? A dead client ends the keep-alive loop (§7).
    pub fn dead(&self) -> bool {
        self.dead
    }

    /// Remember a write failure in `dead` and pass it up.
    fn track(&mut self, out: std::io::Result<()>) -> std::io::Result<()> {
        self.dead |= out.is_err();
        out
    }
}

impl Respond for HttpRespond<'_> {
    fn begin(&mut self, status: u16, sse: bool) -> std::io::Result<()> {
        self.status = status;
        self.sse = sse;
        if sse {
            let out = write!(
                self.w,
                "HTTP/1.1 {status} {}\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\ntransfer-encoding: chunked\r\n\r\n",
                reason(status)
            );
            self.track(out)?;
        }
        Ok(())
    }

    fn chunk(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.sse {
            // One chunked-encoding frame per encoded event, flushed so the
            // client sees it NOW — streaming is the point (§10).
            let out = write!(self.w, "{:x}\r\n", bytes.len())
                .and_then(|()| self.w.write_all(bytes))
                .and_then(|()| self.w.write_all(b"\r\n"))
                .and_then(|()| self.w.flush());
            self.track(out)
        } else {
            self.body.extend_from_slice(bytes);
            Ok(())
        }
    }

    fn end(&mut self) -> std::io::Result<()> {
        if self.sse {
            let out = self.w.write_all(b"0\r\n\r\n").and_then(|()| self.w.flush());
            return self.track(out);
        }
        let (status, len) = (self.status, self.body.len());
        let body = std::mem::take(&mut self.body);
        let out = write!(
            self.w,
            "HTTP/1.1 {status} {}\r\ncontent-type: application/json\r\ncontent-length: {len}\r\n\r\n",
            reason(status)
        )
        .and_then(|()| self.w.write_all(&body))
        .and_then(|()| self.w.flush());
        self.track(out)
    }
}

/// The reason phrase for the statuses this edge emits; anything else (an exotic
/// upstream status carried through §9) goes bare — clients read the number.
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "",
    }
}
