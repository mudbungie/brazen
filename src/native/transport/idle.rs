//! The inter-chunk idle bound on a streaming body (config §4, bl-9940): ureq's
//! `timeout_recv_body` is a TOTAL cap (wrong for a long generation), so the bound
//! lives here instead — a worker thread owns the blocking `Read` and the timer
//! restarts on every chunk, so only a mid-stream *stall* trips it.

use std::io::{self, Read};
use std::time::Duration;

use brazen::Bytes;

/// A `ChunkReader` with an INTER-CHUNK idle bound. A blocking `read` can't be
/// interrupted in place, so a worker thread owns the `Read` and pushes each chunk
/// down a rendezvous channel; `next` waits at most `idle` for the next one. The
/// timer restarts on every received chunk, so total stream length is unbounded —
/// only a *stall* trips it, surfacing as a `TimedOut` item (`run` → Transport, 69).
/// On a stall the worker is abandoned (it dies with the one-shot process, arch
/// §10); a `None`/error from the channel means the worker reached EOF or dropped.
pub(super) struct IdleChunkReader {
    rx: std::sync::mpsc::Receiver<io::Result<Bytes>>,
    idle: Duration,
    done: bool,
}

impl IdleChunkReader {
    pub(super) fn spawn<R: Read + Send + 'static>(reader: R, idle: Duration) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<io::Result<Bytes>>(0);
        std::thread::spawn(move || {
            let mut reader = reader;
            loop {
                let mut buf = vec![0u8; 8192];
                let item = match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.truncate(n);
                        Ok(buf)
                    }
                    Err(e) => Err(e),
                };
                let is_err = item.is_err();
                // A send error means `next` stopped pulling (stall abandon / drop);
                // the worker then exits. An error chunk ends the stream too.
                if tx.send(item).is_err() || is_err {
                    break;
                }
            }
        });
        IdleChunkReader {
            rx,
            idle,
            done: false,
        }
    }
}

impl Iterator for IdleChunkReader {
    type Item = io::Result<Bytes>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.rx.recv_timeout(self.idle) {
            Ok(item) => {
                self.done = item.is_err();
                Some(item)
            }
            // No chunk within `idle`: the stream stalled mid-body.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.done = true;
                Some(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "stream stalled: no data within the idle-read timeout",
                )))
            }
            // The worker reached EOF (or died): a clean end of body.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                self.done = true;
                None
            }
        }
    }
}
