//! The `ModelCache` double (arch §9.1, model-discovery §5.1): an in-process map
//! backing the serve-cache-lookup and `list-models` tests, the sibling of the
//! [`MemoryCredStore`](super::store) double. `new`/`with` build an empty or primed
//! cache; `puts` reads back what `list-models` wrote. Mutex-backed, not `RefCell`,
//! so the same double serves the `--serve` accept loop's `Sync`-bounded seams.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::canonical::Model;
use crate::store::ModelCache;

/// An in-process `ModelCache` (arch §9.1). A primed entry models a cache a prior
/// `bz --list-models` wrote; an unknown provider is `None` (the cold-cache path). Every
/// `put` is recorded in order so a test asserts a write fired — either the `list-models`
/// verb's wholesale replace or the data plane's learn-on-success append (§5.4).
#[derive(Default)]
pub struct MemoryModelCache {
    entries: Mutex<HashMap<String, Vec<Model>>>,
    puts: Mutex<Vec<(String, Vec<Model>)>>,
}

impl MemoryModelCache {
    /// An empty (cold) cache.
    pub fn new() -> Self {
        MemoryModelCache::default()
    }

    /// A cache primed with one provider's list — as if `bz --list-models` had run.
    pub fn with(provider: &str, models: Vec<Model>) -> Self {
        let cache = MemoryModelCache::new();
        if let Ok(mut entries) = cache.entries.lock() {
            entries.insert(provider.to_owned(), models);
        }
        cache
    }

    /// Prime a SECOND provider's list — `with(..).and(..)` is how a routing test
    /// models the multi-provider cache the fall-through tier reads (config §7).
    pub fn and(self, provider: &str, models: Vec<Model>) -> Self {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(provider.to_owned(), models);
        }
        self
    }

    /// Every `(provider, models)` `put` this cache recorded, in order — the assertion
    /// that a write fired (and what it wrote): `list-models` or learn-on-success (§5.4).
    pub fn puts(&self) -> Vec<(String, Vec<Model>)> {
        self.puts.lock().ok().map(|p| p.clone()).unwrap_or_default()
    }
}

impl ModelCache for MemoryModelCache {
    fn get(&self, provider: &str) -> Option<Vec<Model>> {
        self.entries.lock().ok()?.get(provider).cloned()
    }

    fn put(&self, provider: &str, models: &[Model]) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(provider.to_owned(), models.to_vec());
        }
        if let Ok(mut puts) = self.puts.lock() {
            puts.push((provider.to_owned(), models.to_vec()));
        }
    }
}
