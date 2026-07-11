//! Shared test doubles for the injected seams (arch §9.1), one submodule per seam:
//! `transport` (`MockTransport`/`ScriptedTransport` + `Chunk`), `store`
//! (`MemoryCredStore`), `cache` (`MemoryModelCache`), `clock` (`FakeClock`), and
//! `login` (the `bz --login` control-plane doubles). Always compiled — a plain
//! `pub mod` (not `#[cfg(test)]`, lib.rs) so the integration suite and the `bz`
//! bin share one set of doubles — and free of `unwrap`/`expect`/`panic` so they
//! pass the data-path lint even under `not(test)`.

mod cache;
mod clock;
mod listen;
mod login;
mod store;
mod transport;

pub use cache::MemoryModelCache;
pub use clock::FakeClock;
pub use listen::{MemConn, ScriptedBind, ScriptedListener, Wrote};
pub use login::{FakeBrowserLauncher, FakeCodeReceiver, FakePacer};
pub use store::MemoryCredStore;
pub use transport::{Chunk, MockTransport, ScriptedTransport};
