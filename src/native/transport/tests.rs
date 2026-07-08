//! Integration coverage for the idle-read timeout (bl-3394, bl-9940) — the one
//! piece of the coverage-excluded `bz` shim whose behavior was reasoned but never
//! exercised (`IdleChunkReader` in the parent module). Drives the REAL
//! `HttpTransport::send` against a localhost `TcpListener` that controls response
//! timing, below the protocol layer: the timeouts act on the raw HTTP byte stream,
//! so encode/decode is irrelevant and ONE generic stall server suffices.
//!
//! The two assertions that prove the feature:
//! 1. STALL — server writes 200 + one body chunk, then sleeps. The body iterator
//!    must yield a `TimedOut` error after ~idle (exit-69 class), NOT hang.
//! 2. SLOW-BUT-LIVE — server writes a chunk every gap-under-idle for longer than a
//!    single idle window, then ends cleanly. Must complete with NO timeout: the
//!    load-bearing proof that the idle timer resets per chunk (a naive total-body
//!    cap would have wrongly truncated this).
//!
//! Config granularity is whole seconds, so the finest idle is 1s (bl-9940 decision
//! (a), no code change). Timing asserts are lower-bound ("waited ~idle") plus a
//! generous ceiling ("did not hang") — never a tight upper bound, which flakes on
//! loaded CI runners.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use brazen::{Transport, WireRequest};

use super::{error_chain, HttpTransport};

/// The chunked-response head: 200 with `Transfer-Encoding: chunked` so the server
/// can hand-frame body chunks over time and ureq dechunks them incrementally.
const HEAD: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n";

/// The slow-but-live stream: `SLOW_CHUNKS` payloads, each sent `SLOW_GAP` apart.
/// The gap is under the 1s idle window, but the total (~1.5s) outlasts it — so a
/// completed stream proves the per-chunk reset.
const SLOW_CHUNKS: usize = 6;
const SLOW_GAP: Duration = Duration::from_millis(300);

/// The i-th live payload; shared by server and assertion so they cannot drift.
fn chunk_text(i: usize) -> String {
    format!("chunk{i} ")
}

/// Bind `127.0.0.1:0`, spawn a one-shot accept thread running `handler`, and return
/// the `http://` URL. The thread is detached: it dies with the test process (the
/// stall handler outlives the assertion on purpose), arch §10.
fn serve<F>(handler: F) -> String
where
    F: FnOnce(TcpStream) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            handler(stream);
        }
    });
    format!("http://127.0.0.1:{port}/")
}

/// Best-effort drain of the request head so the client's write completes; the tiny
/// POST body fits one localhost segment, so a single read is enough and leaving the
/// body unread never deadlocks the client.
fn drain_request(stream: &mut TcpStream) {
    let mut buf = [0u8; 4096];
    let _ = stream.read(&mut buf);
}

/// Write one HTTP chunk (`<hex-len>\r\n<data>\r\n`) and flush so it leaves as its
/// own segment.
fn write_chunk(stream: &mut TcpStream, data: &str) -> io::Result<()> {
    write!(stream, "{:x}\r\n", data.len())?;
    stream.write_all(data.as_bytes())?;
    stream.write_all(b"\r\n")?;
    stream.flush()
}

/// Headers + one chunk, then hold the connection open by blocking on a read: the
/// client must abandon the stalled body read after the idle timeout. The read
/// returns (EOF/error) the moment the client drops the socket, so the handler
/// lives exactly as long as the client — no fixed sleep, no thread lingering past
/// the test.
fn stall_handler(mut stream: TcpStream) {
    drain_request(&mut stream);
    let _ = stream.write_all(HEAD);
    let _ = write_chunk(&mut stream, "hello ");
    let mut buf = [0u8; 64];
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 {
            break;
        }
    }
}

/// Headers, then a chunk every `SLOW_GAP` for `SLOW_CHUNKS` rounds, then a clean
/// chunked terminator. Never stalls longer than a single idle window.
fn slow_handler(mut stream: TcpStream) {
    drain_request(&mut stream);
    let _ = stream.write_all(HEAD);
    for i in 0..SLOW_CHUNKS {
        if write_chunk(&mut stream, &chunk_text(i)).is_err() {
            return;
        }
        thread::sleep(SLOW_GAP);
    }
    let _ = stream.write_all(b"0\r\n\r\n");
    let _ = stream.flush();
}

/// Drive `HttpTransport::send` with `idle` set, drain the body iterator, and report
/// the concatenated body, chunk count, any terminal error, and the body-drain
/// elapsed (measured from after `send`, so it is the idle window, not connect).
fn drain_body(url: &str, idle: u64) -> (Vec<u8>, usize, Option<io::Error>, Duration) {
    let mut wire = WireRequest::new(url, b"{}".to_vec());
    wire.timeouts.idle = Some(idle);
    let resp = HttpTransport::new()
        .send(wire)
        .expect("connect + response headers succeed");
    let start = Instant::now();
    let mut body = Vec::new();
    let mut chunks = 0;
    let mut err = None;
    for item in resp.body {
        match item {
            Ok(b) => {
                chunks += 1;
                body.extend_from_slice(&b);
            }
            Err(e) => {
                err = Some(e);
                break;
            }
        }
    }
    (body, chunks, err, start.elapsed())
}

#[test]
fn stall_after_first_chunk_times_out() {
    let url = serve(stall_handler);
    let (_body, chunks, err, elapsed) = drain_body(&url, 1);

    assert!(
        chunks >= 1,
        "the one pre-stall chunk should arrive: got {chunks}"
    );
    let err = err.expect("a stalled stream must yield an error, not hang");
    assert_eq!(
        err.kind(),
        io::ErrorKind::TimedOut,
        "an idle stall surfaces as TimedOut (the exit-69 class): {err}"
    );
    assert!(
        elapsed >= Duration::from_millis(900),
        "must wait ~idle (1s) before firing, waited {elapsed:?}"
    );
    // A generous ceiling: this only proves "did not hang forever", so the real
    // floor is the 900ms lower bound above. A tight figure here flakes on a loaded
    // CI runner where recv_timeout(1s) can overshoot several-fold without any
    // correctness problem; 30s still catches a true (infinite) hang.
    assert!(
        elapsed < Duration::from_secs(30),
        "must fire (not hang forever) once the stall trips: waited {elapsed:?}"
    );
}

#[test]
fn slow_but_live_stream_completes_without_timeout() {
    let url = serve(slow_handler);
    let (body, chunks, err, elapsed) = drain_body(&url, 1);

    assert!(
        err.is_none(),
        "a live stream slower-than-idle-per-chunk must NOT time out: {err:?}"
    );
    let expected: String = (0..SLOW_CHUNKS).map(chunk_text).collect();
    assert_eq!(
        body,
        expected.as_bytes(),
        "every live chunk should arrive intact (chunks read: {chunks})"
    );
    // ~1.5s total > the 1s idle window, with no timeout: the idle timer reset on
    // every chunk. A naive total-body cap would have truncated this.
    assert!(
        elapsed >= Duration::from_secs(1),
        "stream should outlast a single idle window, proving the per-chunk reset, took {elapsed:?}"
    );
}

/// A nestable error whose `source()` returns the next link — the general shape
/// ureq's own `Error` does NOT have (its `Error` impl is empty, so `source()` is
/// `None` and only its `Display` folds the wrapped cause). This lets the tests pin
/// the source-walk on a real chain, independent of ureq's flat error.
#[derive(Debug)]
struct Link {
    msg: &'static str,
    src: Option<Box<Link>>,
}

impl std::fmt::Display for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.msg)
    }
}

impl std::error::Error for Link {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.src
            .as_deref()
            .map(|s| s as &(dyn std::error::Error + 'static))
    }
}

#[test]
fn error_chain_joins_every_source_link_with_colon() {
    // The load-bearing case: a connect→TLS→cert chain collapses to one line where
    // the cert root cause is visible, not swallowed behind "connection failed".
    let err = Link {
        msg: "connection failed",
        src: Some(Box::new(Link {
            msg: "TLS handshake",
            src: Some(Box::new(Link {
                msg: "certificate not trusted",
                src: None,
            })),
        })),
    };
    assert_eq!(
        error_chain(&err),
        "connection failed: TLS handshake: certificate not trusted"
    );
}

#[test]
fn error_chain_of_a_sourceless_error_is_its_display() {
    // ureq's real shape: no `source()`, all detail already in `Display`. The walk
    // must be a clean no-op, never appending a stray ": ".
    let err = Link {
        msg: "host down",
        src: None,
    };
    assert_eq!(error_chain(&err), "host down");
}
