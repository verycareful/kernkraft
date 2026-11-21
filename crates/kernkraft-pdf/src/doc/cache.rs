// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A bounded cache that drops the entry used longest ago when it is full.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

/// Holds at most `capacity` values. Each lookup and insert marks its entry
/// as the most recently used; an insert into a full cache drops the least
/// recently used entry first. A capacity of 0 holds nothing.
#[derive(Debug)]
pub(crate) struct Lru<K, V> {
    capacity: usize,
    /// Each key's value and the tick of its last use.
    entries: HashMap<K, (V, u64)>,
    /// The keys by the tick of their last use, the oldest first.
    by_use: BTreeMap<u64, K>,
    tick: u64,
}

impl<K: Copy + Eq + Hash, V: Clone> Lru<K, V> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
            by_use: BTreeMap::new(),
            tick: 0,
        }
    }

    pub(crate) fn get(&mut self, key: &K) -> Option<V> {
        let tick = self.next_tick();
        let (value, used) = self.entries.get_mut(key)?;
        self.by_use.remove(used);
        *used = tick;
        self.by_use.insert(tick, *key);
        Some(value.clone())
    }

    pub(crate) fn insert(&mut self, key: K, value: V) {
        if self.capacity == 0 {
            return;
        }
        let tick = self.next_tick();
        if let Some((_, used)) = self.entries.insert(key, (value, tick)) {
            self.by_use.remove(&used);
        } else if self.entries.len() > self.capacity {
            if let Some((_, oldest)) = self.by_use.pop_first() {
                self.entries.remove(&oldest);
            }
        }
        self.by_use.insert(tick, key);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    fn next_tick(&mut self) -> u64 {
        // A u64 counted up by one per use does not wrap in practice; were
        // it to, ordering would be lost and nothing else.
        self.tick = self.tick.wrapping_add(1);
        self.tick
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn drops_the_least_recently_used() {
        let mut cache = Lru::new(2);
        cache.insert(1, "a");
        cache.insert(2, "b");
        assert_eq!(cache.get(&1), Some("a"));
        cache.insert(3, "c");
        assert_eq!(cache.get(&2), None);
        assert_eq!(cache.get(&1), Some("a"));
        assert_eq!(cache.get(&3), Some("c"));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn insert_of_a_held_key_replaces_and_refreshes() {
        let mut cache = Lru::new(2);
        cache.insert(1, "a");
        cache.insert(2, "b");
        cache.insert(1, "z");
        cache.insert(3, "c");
        assert_eq!(cache.get(&1), Some("z"));
        assert_eq!(cache.get(&2), None);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn capacity_zero_holds_nothing() {
        let mut cache = Lru::new(0);
        cache.insert(1, "a");
        assert_eq!(cache.get(&1), None);
        assert_eq!(cache.len(), 0);
    }
}
