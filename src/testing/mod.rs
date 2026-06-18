//! Shared test doubles for the injected seams (arch §9.1), one submodule per seam:
//! [`transport`] (`MockTransport`/`ScriptedTransport` + `Chunk`), [`store`]
//! (`MemoryCredStore`), [`cache`] (`MemoryModelCache`), [`clock`] (`FakeClock`), and
//! [`login`] (the `bz login` control-plane doubles). Always compiled (the crate is
//! internal, `publish = false`) and free of `unwrap`/`expect`/`panic` so they pass
//! the data-path lint even under `not(test)`.

mod cache;
mod clock;
mod login;
mod store;
mod transport;

pub use cache::MemoryModelCache;
pub use clock::FakeClock;
pub use login::{FakeBrowserLauncher, FakeCodeReceiver, FakePacer};
pub use store::MemoryCredStore;
pub use transport::{Chunk, MockTransport, ScriptedTransport};
