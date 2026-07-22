//! The exec transport kind (claude-code spec §3) — the subprocess sibling of the
//! HTTP round-trip, routed from [`HttpTransport::send`](super::transport) when the
//! wire declares an [`ExecSpec`]. Spawn `program args…`, body → child stdin (own
//! thread, then closed), child stdout → the body iterator, stderr drained
//! concurrently. Status is **200 at spawn** (the spawn IS the handshake; every later
//! failure is in-band/mid-stream, spec §3.2). The one silence budget (`timeouts.idle`)
//! bounds inter-chunk stdout stalls — a breach KILLS the child; the child is always
//! reaped (wait on EOF, kill+wait on stall, a `Drop` backstop on abandonment), so no
//! zombie survives (spec §3.3). Impure by nature, so it lives in the coverage-excluded
//! shim like the rest of `src/native/`.

use std::io::{self, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use brazen::{
    envelope_request, Bytes, CanonicalError, Envelope, ErrorKind, ExecSpec, TransportResponse,
    WireRequest,
};

/// The ONE subprocess entry point behind the transport seam. The [`Envelope`] on the
/// spec says what the child's pipes carry, and it is read HERE and nowhere else
/// (transport spec §4.1):
///
/// - [`Envelope::Body`] — the child IS the provider (claude-code §3): stdin is the
///   dialect's prompt, stdout its own stream, status 200 at spawn.
/// - [`Envelope::Http`] — the child IS the transport (transport spec §5): stdin is
///   the whole HTTP/1.1 request message, stdout the whole response message, whose
///   head the relay parses for the real status and `retry-after`.
pub(super) fn send_exec(
    spec: &ExecSpec,
    wire: &WireRequest,
) -> Result<TransportResponse, CanonicalError> {
    match spec.envelope {
        Envelope::Body => Ok(TransportResponse {
            status: 200,
            body: Box::new(spawn(spec, wire, wire.body.clone())?),
            retry_after: None,
        }),
        Envelope::Http => super::relay::respond(spawn(spec, wire, envelope_request(wire))?),
    }
}

/// Spawn `program args…`, feed `stdin_bytes`, and hand back the child's stdout as
/// the seam's incremental body stream. A spawn failure (binary missing/not
/// executable) is the exec analogue of an unreachable host — a `Transport` error
/// (→69) naming the program and the OS error.
fn spawn(
    spec: &ExecSpec,
    wire: &WireRequest,
    stdin_bytes: Vec<u8>,
) -> Result<ExecBody, CanonicalError> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if spec.envelope == Envelope::Http {
        // The resolved budgets reach a TRANSPORT delegate as environment, never argv
        // (visible in `ps`) and never pseudo-headers (which would pollute the header
        // list it forwards verbatim) — transport spec §5.3. Each is absent when
        // unset: the empty set, never a sentinel. Honouring them is the delegate's
        // job; ENFORCING the idle bound stays ours (the stall kill below).
        let t = wire.timeouts;
        for (name, secs) in [
            ("BZ_TRANSPORT_CONNECT_TIMEOUT", t.connect),
            ("BZ_TRANSPORT_RESPONSE_TIMEOUT", t.response),
            ("BZ_TRANSPORT_IDLE_TIMEOUT", t.idle),
        ] {
            if let Some(secs) = secs {
                command.env(name, secs.to_string());
            }
        }
    }
    let mut child = command
        .spawn()
        .map_err(|e| spawn_error(&spec.program, &e))?;

    // Body → child stdin from its own thread (never a pipe deadlock), then the drop
    // closes it so the child sees EOF on its prompt.
    let body = stdin_bytes;
    if let Some(mut stdin) = child.stdin.take() {
        thread::spawn(move || {
            let _ = stdin.write_all(&body);
        });
    }
    // Drain stderr concurrently; the text is folded into a trailing in-band error
    // only when the child exits nonzero (spec §3.2).
    let stderr = child.stderr.take();
    let stderr_thread = thread::spawn(move || {
        let mut buf = String::new();
        if let Some(mut pipe) = stderr {
            let _ = pipe.read_to_string(&mut buf);
        }
        buf
    });
    // Child stdout → a bounded channel a reader thread fills; the body iterator
    // pulls with the silence budget as its per-chunk timeout.
    let stdout = child.stdout.take();
    let (tx, rx) = sync_channel::<io::Result<Bytes>>(4);
    thread::spawn(move || {
        let Some(mut pipe) = stdout else { return };
        loop {
            let mut buf = vec![0u8; 8192];
            match pipe.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => {
                    buf.truncate(n);
                    if tx.send(Ok(buf)).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            }
        }
    });

    Ok(ExecBody {
        rx,
        child,
        stderr_thread: Some(stderr_thread),
        idle: wire.timeouts.idle.map(Duration::from_secs),
        done: false,
    })
}

/// The child's stdout as the seam's incremental body stream. EOF reaps the child and
/// — when it exited nonzero with a non-empty stderr — yields one trailing `Err`
/// carrying the exit and the stderr text, so a flag/usage failure stays diagnosable
/// (spec §3.2). A silence-budget breach kills the child and yields the timeout `Err`.
pub(super) struct ExecBody {
    rx: Receiver<io::Result<Bytes>>,
    child: Child,
    stderr_thread: Option<JoinHandle<String>>,
    idle: Option<Duration>,
    done: bool,
}

impl ExecBody {
    /// Reap the child (kill first when `kill` — stall/abandonment) and collect its
    /// stderr; idempotent via `done`.
    ///
    /// The stderr thread is joined ONLY on the clean-EOF path, which is the only
    /// path that reads the text. On the kill path it is detached instead: a killed
    /// child's stderr pipe can still be held open by a GRANDCHILD it spawned (a `sh`
    /// stub's `sleep`, a relay's own worker), so joining there would make the
    /// silence-budget kill wait for the very process it just abandoned — the budget
    /// would bound nothing (bl-a0ea, caught by the delegate stall test).
    fn reap(&mut self, kill: bool) -> (Option<i32>, String) {
        self.done = true;
        if kill {
            let _ = self.child.kill();
            self.stderr_thread.take();
            return (self.child.wait().ok().and_then(|s| s.code()), String::new());
        }
        let code = self.child.wait().ok().and_then(|s| s.code());
        let stderr = self
            .stderr_thread
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        (code, stderr)
    }
}

impl Iterator for ExecBody {
    type Item = io::Result<Bytes>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let received = match self.idle {
            Some(budget) => self.rx.recv_timeout(budget),
            None => self.rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match received {
            Ok(Ok(bytes)) => Some(Ok(bytes)),
            Ok(Err(e)) => {
                self.reap(true);
                Some(Err(e))
            }
            Err(RecvTimeoutError::Timeout) => {
                let (_, _) = self.reap(true);
                Some(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "child produced no output within the silence budget; killed",
                )))
            }
            Err(RecvTimeoutError::Disconnected) => {
                // Clean stdout EOF: reap, and surface a nonzero exit's stderr as the
                // trailing in-band error (empty stderr adds nothing — the stream
                // verdict, when present, already told the truth).
                let (code, stderr) = self.reap(false);
                let failed = code != Some(0);
                let text = stderr.trim();
                if failed && !text.is_empty() {
                    Some(Err(io::Error::other(format!(
                        "child exited with status {}: {text}",
                        code.map_or_else(|| "signal".to_owned(), |c| c.to_string()),
                    ))))
                } else {
                    None
                }
            }
        }
    }
}

impl Drop for ExecBody {
    /// The zombie backstop (spec §3.3): an abandoned stream still kills and reaps.
    fn drop(&mut self) {
        if !self.done {
            self.reap(true);
        }
    }
}

/// A spawn failure as a `Transport`-kind error (→69): the exec analogue of an
/// unreachable host, naming the program and the OS error.
fn spawn_error(program: &str, e: &io::Error) -> CanonicalError {
    CanonicalError {
        kind: ErrorKind::Transport,
        message: format!("exec transport: failed to spawn `{program}`: {e}"),
        provider_detail: None,
        retry_after_seconds: None,
    }
}
