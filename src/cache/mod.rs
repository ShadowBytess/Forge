//! Tiny TTL cache used to avoid hammering the AUR RPC and other slow
//! sources while typing in the search box.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

#[derive(Debug)]
struct Entry<V> {
    value: V,
    inserted: Instant,
}

/// In-memory cache with per-entry expiry and a maximum size (oldest entries
/// are evicted first). Cheap enough that backends can use it liberally.
#[derive(Debug)]
pub struct MemoryCache<K, V> {
    map: HashMap<K, Entry<V>>,
    ttl: Duration,
    capacity: usize,
}

impl<K: Eq + Hash + Clone, V: Clone> Default for MemoryCache<K, V> {
    fn default() -> Self {
        Self::new(Duration::from_secs(60), 512)
    }
}

impl<K: Eq + Hash + Clone, V: Clone> MemoryCache<K, V> {
    pub fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            map: HashMap::new(),
            ttl,
            capacity: capacity.max(1),
        }
    }

    pub fn get(&self, key: &K) -> Option<V> {
        let entry = self.map.get(key)?;
        if entry.inserted.elapsed() > self.ttl {
            return None;
        }
        Some(entry.value.clone())
    }

    pub fn insert(&mut self, key: K, value: V) {
        if self.map.len() >= self.capacity && !self.map.contains_key(&key) {
            self.evict_one();
        }
        self.map.insert(
            key,
            Entry {
                value,
                inserted: Instant::now(),
            },
        );
    }

    /// Drops expired entries; called opportunistically.
    pub fn sweep(&mut self) {
        self.map.retain(|_, e| e.inserted.elapsed() <= self.ttl);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn evict_one(&mut self) {
        let oldest = self
            .map
            .iter()
            .min_by_key(|(_, e)| e.inserted)
            .map(|(k, _)| k.clone());
        if let Some(k) = oldest {
            self.map.remove(&k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_returns_values() {
        let mut c = MemoryCache::new(Duration::from_secs(10), 4);
        c.insert("k", 42);
        assert_eq!(c.get(&"k"), Some(42));
        assert_eq!(c.get(&"missing"), None);
    }

    #[test]
    fn expires_after_ttl() {
        let mut c = MemoryCache::new(Duration::from_millis(20), 4);
        c.insert("k", 1);
        assert_eq!(c.get(&"k"), Some(1));
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(c.get(&"k"), None);
        c.sweep();
        assert!(c.is_empty());
    }

    #[test]
    fn evicts_oldest_at_capacity() {
        let mut c = MemoryCache::new(Duration::from_secs(60), 2);
        c.insert("a", 1);
        std::thread::sleep(Duration::from_millis(5));
        c.insert("b", 2);
        std::thread::sleep(Duration::from_millis(5));
        c.insert("c", 3);
        assert!(c.len() <= 2);
        assert_eq!(c.get(&"a"), None, "oldest entry should have been evicted");
        assert!(c.get(&"b").is_some() || c.get(&"c").is_some());
    }
}
