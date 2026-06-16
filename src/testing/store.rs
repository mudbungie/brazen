//! The `CredStore` double (arch §9.1): an in-process map backing the data-plane
//! auth tests, with `new`/`with` constructors for an empty or preloaded store.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;

use crate::store::{Cred, CredStore};

/// An in-process `CredStore` (arch §9.1) backing the data-plane auth tests.
#[derive(Default)]
pub struct MemoryCredStore {
    creds: RefCell<HashMap<String, Cred>>,
}

impl MemoryCredStore {
    /// An empty store.
    pub fn new() -> Self {
        MemoryCredStore::default()
    }

    /// A store preloaded with one provider's cred.
    pub fn with(provider: &str, cred: Cred) -> Self {
        let store = MemoryCredStore::new();
        store.creds.borrow_mut().insert(provider.to_owned(), cred);
        store
    }
}

impl CredStore for MemoryCredStore {
    fn get(&self, provider: &str) -> Option<Cred> {
        self.creds.borrow().get(provider).cloned()
    }

    fn put(&self, provider: &str, cred: &Cred) -> io::Result<()> {
        self.creds
            .borrow_mut()
            .insert(provider.to_owned(), cred.clone());
        Ok(())
    }
}
