//! The `ModelCache` double (arch §9.1, model-discovery §5.1): an in-process map
//! backing the serve-cache-lookup and `list-models` tests, the sibling of the
//! [`MemoryCredStore`](super::store) double. `new`/`with` build an empty or primed
//! cache; `puts` reads back what `list-models` wrote. Mutex-backed, not `RefCell`,
//! so the same double serves the `--serve` accept loop's `Sync`-bounded seams.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::canonical::{CachedModels, Model};
use crate::store::ModelCache;

/// An in-process `ModelCache` (arch §9.1). A primed entry models a cache a prior
/// `bz --list-models` wrote; an unknown provider is `None` (the cold-cache path). Every
/// `put` is recorded in order so a test asserts a write fired — either the `list-models`
/// verb's wholesale replace or the data plane's learn-on-success append (§5.4).
#[derive(Default)]
pub struct MemoryModelCache {
    entries: Mutex<HashMap<String, CachedModels>>,
    puts: Mutex<Vec<(String, CachedModels)>>,
}

impl MemoryModelCache {
    /// An empty (cold) cache.
    pub fn new() -> Self {
        MemoryModelCache::default()
    }

    /// A cache primed with one provider's list — as if `bz --list-models` had run.
    pub fn with(provider: &str, models: Vec<Model>) -> Self {
        MemoryModelCache::new().and(provider, models)
    }

    /// Prime a SECOND provider's list — `with(..).and(..)` is how a routing test
    /// models the multi-provider cache the fall-through tier reads (config §7).
    pub fn and(self, provider: &str, models: Vec<Model>) -> Self {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(
                provider.to_owned(),
                CachedModels {
                    models,
                    last_used: None,
                },
            );
        }
        self
    }

    /// Prime a provider's `last_used` pointer — as if a prior 2xx had used that id
    /// (model-discovery §5.4). The §4 rung-2 fixture, applied to the last-primed row.
    pub fn last_used(self, provider: &str, id: &str) -> Self {
        if let Ok(mut entries) = self.entries.lock() {
            entries.entry(provider.to_owned()).or_default().last_used = Some(id.to_owned());
        }
        self
    }

    /// Every `(provider, models)` `put` this cache recorded, in order — the assertion
    /// that a write fired (and what it wrote): `list-models` or learn-on-success (§5.4).
    pub fn puts(&self) -> Vec<(String, CachedModels)> {
        self.puts.lock().ok().map(|p| p.clone()).unwrap_or_default()
    }
}

impl ModelCache for MemoryModelCache {
    fn get(&self, provider: &str) -> Option<CachedModels> {
        self.entries.lock().ok()?.get(provider).cloned()
    }

    fn put(&self, provider: &str, cached: &CachedModels) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(provider.to_owned(), cached.clone());
        }
        if let Ok(mut puts) = self.puts.lock() {
            puts.push((provider.to_owned(), cached.clone()));
        }
    }
}
