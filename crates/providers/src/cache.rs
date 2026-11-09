// SPDX-License-Identifier: AGPL-3.0-only
//! Small bounded in-memory cache with TTL (FIFO eviction). Keeps memory flat
//! regardless of listening history size; persistent caching lives in the
//! storage crate.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct BoundedCache<K, V> {
    capacity: usize,
    map: HashMap<K, (V, Instant)>,
    order: VecDeque<K>,
}

impl<K: Eq + Hash + Clone, V: Clone> BoundedCache<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Get a value if present and younger than `ttl`.
    pub fn get(&self, key: &K, ttl: Duration) -> Option<V> {
        self.map
            .get(key)
            .filter(|(_, at)| at.elapsed() < ttl)
            .map(|(v, _)| v.clone())
    }

    pub fn insert(&mut self, key: K, value: V) {
        if self
            .map
            .insert(key.clone(), (value, Instant::now()))
            .is_none()
        {
            self.order.push_back(key);
        }
        while self.map.len() > self.capacity {
            match self.order.pop_front() {
                Some(old) => {
                    self.map.remove(&old);
                }
                None => break,
            }
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_oldest_and_respects_ttl() {
        let mut c = BoundedCache::new(2);
        c.insert("a", 1);
        c.insert("b", 2);
        c.insert("c", 3);
        assert_eq!(c.len(), 2);
        assert_eq!(c.get(&"a", Duration::from_secs(60)), None);
        assert_eq!(c.get(&"c", Duration::from_secs(60)), Some(3));
        assert_eq!(c.get(&"c", Duration::ZERO), None);
    }

    #[test]
    fn reinsert_updates_without_growing() {
        let mut c = BoundedCache::new(2);
        c.insert("a", 1);
        c.insert("a", 2);
        assert_eq!(c.len(), 1);
        assert_eq!(c.get(&"a", Duration::from_secs(60)), Some(2));
    }
}
