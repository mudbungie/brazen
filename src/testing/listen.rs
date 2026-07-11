//! `Bind`/`Listener` doubles for the `--serve` accept loop (ingress §7, §14):
//! `MemConn` is one in-memory client connection — canned request bytes to read,
//! a shared buffer collecting the response, an optional write budget so a
//! mid-stream client disconnect is one canned connection; `ScriptedListener`
//! yields a finite queue of them (drained ⇒ `accept() → None` ⇒ the loop's
//! testable shutdown); `ScriptedBind` hands the listener out once and records
//! the address the lib asked to bind.

use core::net::SocketAddr;
use std::collections::VecDeque;
use std::io::{self, Cursor, Read, Write};
use std::sync::{Arc, Mutex};

use crate::run::{Bind, Listener, ServeConn};

/// The shared response buffer a [`MemConn`] writes into; read it after `serve`
/// returns (the scoped accept loop joins every connection first).
pub type Wrote = Arc<Mutex<Vec<u8>>>;

/// One in-memory connection: `read` drains the canned client bytes (EOF after —
/// the keep-alive hang-up), `write` appends to the shared buffer until the
/// optional byte budget runs out, after which every write fails (the client
/// vanished mid-stream).
pub struct MemConn {
    read: Cursor<Vec<u8>>,
    wrote: Wrote,
    budget: Option<usize>,
}

impl MemConn {
    /// A connection that reads `request` and accepts unlimited response bytes.
    pub fn new(request: &[u8]) -> (MemConn, Wrote) {
        Self::with_budget(request, None)
    }

    /// A connection whose writes fail after `budget` accepted bytes — the
    /// mid-stream client disconnect (ingress §7).
    pub fn failing_after(request: &[u8], budget: usize) -> (MemConn, Wrote) {
        Self::with_budget(request, Some(budget))
    }

    fn with_budget(request: &[u8], budget: Option<usize>) -> (MemConn, Wrote) {
        let wrote: Wrote = Arc::default();
        let conn = MemConn {
            read: Cursor::new(request.to_vec()),
            wrote: Arc::clone(&wrote),
            budget,
        };
        (conn, wrote)
    }
}

impl Read for MemConn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read.read(buf)
    }
}

impl Write for MemConn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(budget) = &mut self.budget {
            if *budget < buf.len() {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            *budget -= buf.len();
        }
        if let Ok(mut wrote) = self.wrote.lock() {
            wrote.extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A finite accept queue: each `accept` pops the next canned connection; a
/// drained queue is `None` — the loop's shutdown, standing in for the signal
/// that ends the native process.
pub struct ScriptedListener {
    conns: Mutex<VecDeque<Box<dyn ServeConn>>>,
}

impl ScriptedListener {
    pub fn new(conns: Vec<Box<dyn ServeConn>>) -> Self {
        ScriptedListener {
            conns: Mutex::new(conns.into()),
        }
    }
}

impl Listener for ScriptedListener {
    fn accept(&self) -> Option<Box<dyn ServeConn>> {
        self.conns.lock().ok()?.pop_front()
    }
}

/// The bind double: yields its listener once (a second bind fails, like a port
/// already held) and records the address the lib resolved from `[ingress]`.
pub struct ScriptedBind {
    listener: Mutex<Option<Box<dyn Listener>>>,
    bound: Mutex<Option<SocketAddr>>,
}

impl ScriptedBind {
    pub fn new(listener: Box<dyn Listener>) -> Self {
        ScriptedBind {
            listener: Mutex::new(Some(listener)),
            bound: Mutex::new(None),
        }
    }

    /// The address `serve` asked to bind, once it has.
    pub fn bound(&self) -> Option<SocketAddr> {
        self.bound.lock().ok().and_then(|b| *b)
    }
}

impl Bind for ScriptedBind {
    fn bind(&self, addr: SocketAddr) -> io::Result<Box<dyn Listener>> {
        if let Ok(mut bound) = self.bound.lock() {
            *bound = Some(addr);
        }
        self.listener
            .lock()
            .ok()
            .and_then(|mut l| l.take())
            .ok_or_else(|| io::Error::new(io::ErrorKind::AddrInUse, "address already bound"))
    }
}
