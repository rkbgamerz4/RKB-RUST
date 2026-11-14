//! Byte-budgeted LRU cache in front of any [`AssetSource`].

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::path::{AssetId, AssetPath, ResourceError};
use crate::source::AssetSource;

/// Cache counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes_cached: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

#[derive(Clone)]
struct CacheEntry {
    bytes: Arc<Vec<u8>>,
    last_used: u64,
}

/// Loads assets through a source while keeping recently used bytes in memory.
#[derive(Clone)]
pub struct ResourceManager {
    source: Arc<dyn AssetSource>,
    cache: BTreeMap<AssetId, CacheEntry>,
    budget_bytes: usize,
    bytes_cached: usize,
    clock: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl fmt::Debug for ResourceManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResourceManager")
            .field("budget_bytes", &self.budget_bytes)
            .field("bytes_cached", &self.bytes_cached)
            .field("entries", &self.cache.len())
            .finish_non_exhaustive()
    }
}

impl ResourceManager {
    #[must_use]
    pub fn new(source: impl AssetSource + 'static, budget_bytes: usize) -> Self {
        Self {
            source: Arc::new(source),
            cache: BTreeMap::new(),
            budget_bytes,
            bytes_cached: 0,
            clock: 0,
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    #[must_use]
    pub fn source(&self) -> &dyn AssetSource {
        self.source.as_ref()
    }

    /// Loads an asset, returning an `Arc` so callers can hold it cheaply.
    pub fn load(&mut self, path: &AssetPath) -> Result<Arc<Vec<u8>>, ResourceError> {
        self.clock = self.clock.wrapping_add(1);
        let id = path.id();
        if let Some(entry) = self.cache.get_mut(&id) {
            entry.last_used = self.clock;
            self.hits += 1;
            return Ok(Arc::clone(&entry.bytes));
        }

        self.misses += 1;
        let bytes = Arc::new(self.source.read(path)?);
        if bytes.len() <= self.budget_bytes && self.budget_bytes > 0 {
            self.make_room(bytes.len());
            self.bytes_cached += bytes.len();
            self.cache.insert(
                id,
                CacheEntry {
                    bytes: Arc::clone(&bytes),
                    last_used: self.clock,
                },
            );
        }
        Ok(bytes)
    }

    /// Loads an asset and validates it as UTF-8.
    pub fn load_text(&mut self, path: &AssetPath) -> Result<String, ResourceError> {
        let bytes = self.load(path)?;
        String::from_utf8(bytes.as_ref().clone())
            .map_err(|_| ResourceError::InvalidUtf8(path.to_string()))
    }

    pub fn preload<'a, I>(&mut self, paths: I) -> Result<(), ResourceError>
    where
        I: IntoIterator<Item = &'a AssetPath>,
    {
        for path in paths {
            let _ = self.load(path)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn is_cached(&self, path: &AssetPath) -> bool {
        self.cache.contains_key(&path.id())
    }

    #[must_use]
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.cache.len(),
            bytes_cached: self.bytes_cached,
            hits: self.hits,
            misses: self.misses,
            evictions: self.evictions,
        }
    }

    pub fn set_budget(&mut self, budget_bytes: usize) {
        self.budget_bytes = budget_bytes;
        self.evict_to_fit(0);
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.bytes_cached = 0;
    }

    fn make_room(&mut self, incoming: usize) {
        self.evict_to_fit(incoming);
    }

    fn evict_to_fit(&mut self, incoming: usize) {
        while self.bytes_cached + incoming > self.budget_bytes {
            let Some(oldest) = self
                .cache
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(id, _)| *id)
            else {
                break;
            };
            if let Some(entry) = self.cache.remove(&oldest) {
                self.bytes_cached = self.bytes_cached.saturating_sub(entry.bytes.len());
                self.evictions += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemorySource;

    fn manager(budget: usize) -> ResourceManager {
        let source = MemorySource::from_entries([
            ("a.bin", vec![1, 2, 3, 4]),
            ("b.bin", vec![5, 6, 7, 8]),
            ("text.txt", b"hello".to_vec()),
            ("bad.txt", vec![0xff]),
        ])
        .unwrap();
        ResourceManager::new(source, budget)
    }

    #[test]
    fn repeated_loads_hit_cache() {
        let mut manager = manager(1024);
        let path = AssetPath::new("a.bin").unwrap();
        let first = manager.load(&path).unwrap();
        let second = manager.load(&path).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(manager.stats().hits, 1);
        assert_eq!(manager.stats().misses, 1);
        assert!(manager.is_cached(&path));
    }

    #[test]
    fn evicts_least_recently_used() {
        let mut manager = manager(5);
        let a = AssetPath::new("a.bin").unwrap();
        let b = AssetPath::new("b.bin").unwrap();
        manager.load(&a).unwrap();
        manager.load(&a).unwrap();
        manager.load(&b).unwrap();
        assert!(!manager.is_cached(&a));
        assert!(manager.is_cached(&b));
        assert_eq!(manager.stats().evictions, 1);
    }

    #[test]
    fn loads_text_and_rejects_invalid_utf8() {
        let mut manager = manager(1024);
        assert_eq!(
            manager
                .load_text(&AssetPath::new("text.txt").unwrap())
                .unwrap(),
            "hello"
        );
        assert!(matches!(
            manager.load_text(&AssetPath::new("bad.txt").unwrap()),
            Err(ResourceError::InvalidUtf8(_))
        ));
    }

    #[test]
    fn zero_budget_returns_uncached_assets() {
        let mut manager = manager(0);
        let path = AssetPath::new("a.bin").unwrap();
        assert_eq!(manager.load(&path).unwrap().as_slice(), &[1, 2, 3, 4]);
        assert!(!manager.is_cached(&path));
        assert_eq!(manager.stats().entries, 0);
    }

    #[test]
    fn shrinking_budget_evicts_entries() {
        let mut manager = manager(1024);
        manager.load(&AssetPath::new("a.bin").unwrap()).unwrap();
        manager.load(&AssetPath::new("b.bin").unwrap()).unwrap();
        assert_eq!(manager.stats().entries, 2);
        manager.set_budget(4);
        assert_eq!(manager.stats().entries, 1);
        assert_eq!(manager.stats().bytes_cached, 4);
        manager.clear();
        assert_eq!(manager.stats().bytes_cached, 0);
    }
}
