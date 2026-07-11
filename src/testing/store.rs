//! The `CredStore` double (arch §9.1): an in-process map backing the data-plane
//! auth tests, with `new`/`with` constructors for an empty or preloaded store, and
//! an optional ambient cred so the §5.5 discovery arm is driven with no real file.
//! Mutex-backed so the `--serve` loop's `Sync`-bounded seams take the same double.

use std::collections::HashMap;
use std::io;
use std::sync::Mutex;

use crate::store::{AmbientSpec, Cred, CredStore};

/// An in-process `CredStore` (arch §9.1) backing the data-plane auth tests. The
/// optional `ambient` cred is what [`CredStore::discover`] returns, modelling a
/// foreign credential file present on the box without touching disk (auth §5.5).
#[derive(Default)]
pub struct MemoryCredStore {
    creds: Mutex<HashMap<String, Cred>>,
    ambient: Option<Cred>,
}

impl MemoryCredStore {
    /// An empty store.
    pub fn new() -> Self {
        MemoryCredStore::default()
    }

    /// A store preloaded with one provider's cred.
    pub fn with(provider: &str, cred: Cred) -> Self {
        let store = MemoryCredStore::new();
        if let Ok(mut creds) = store.creds.lock() {
            creds.insert(provider.to_owned(), cred);
        }
        store
    }

    /// A store with no stored cred but an ambient one `discover` will return — the
    /// zero-setup path (login-free), as if Claude Code's file were on the box.
    pub fn with_ambient(cred: Cred) -> Self {
        MemoryCredStore {
            ambient: Some(cred),
            ..MemoryCredStore::default()
        }
    }
}

impl CredStore for MemoryCredStore {
    fn get(&self, provider: &str) -> Option<Cred> {
        self.creds.lock().ok()?.get(provider).cloned()
    }

    fn put(&self, provider: &str, cred: &Cred) -> io::Result<()> {
        if let Ok(mut creds) = self.creds.lock() {
            creds.insert(provider.to_owned(), cred.clone());
        }
        Ok(())
    }

    fn discover(&self, _spec: &AmbientSpec) -> Option<Cred> {
        self.ambient.clone()
    }
}
