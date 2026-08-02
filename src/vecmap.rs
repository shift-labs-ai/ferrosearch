//! A minimal insertion-ordered map over a flat `Vec` of pairs, used for the
//! posting lists of the inverted index.
//!
//! Posting lists are small (a handful of fields per term, and often only a
//! few documents per field), so linear search beats hashing while removing
//! the per-map overhead of a hash table — which matters when the index holds
//! one map per `(term, field)` pair. Iteration order is insertion order,
//! matching the JavaScript `Map`s of the original.
//!
//! Lookups scan from the back: documents are indexed one at a time, so the
//! entry being updated is almost always the most recently appended one.

use std::collections::hash_map::Entry;
use std::hash::Hash;

use rustc_hash::FxHashMap;

pub struct VecMap<K, V> {
    entries: Vec<(K, V)>,
}

impl<K, V> Default for VecMap<K, V> {
    fn default() -> Self {
        VecMap {
            entries: Vec::new(),
        }
    }
}

impl<K: Copy + Eq, V> VecMap<K, V> {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn position(&self, key: K) -> Option<usize> {
        self.entries.iter().rposition(|(k, _)| *k == key)
    }

    pub fn get(&self, key: K) -> Option<&V> {
        self.position(key).map(|index| &self.entries[index].1)
    }

    pub fn get_mut(&mut self, key: K) -> Option<&mut V> {
        self.position(key).map(|index| &mut self.entries[index].1)
    }

    /// Returns the value for `key`, inserting the value produced by `default`
    /// if absent. New keys are appended, preserving insertion order.
    pub fn get_or_insert_with(&mut self, key: K, default: impl FnOnce() -> V) -> &mut V {
        let index = match self.position(key) {
            Some(index) => index,
            None => {
                self.entries.push((key, default()));
                self.entries.len() - 1
            }
        };
        &mut self.entries[index].1
    }

    /// Inserts or replaces the value for `key`. Replacement keeps the entry's
    /// position, like `Map.set` on an existing key.
    pub fn insert(&mut self, key: K, value: V) {
        match self.position(key) {
            Some(index) => self.entries[index].1 = value,
            None => self.entries.push((key, value)),
        }
    }

    /// Removes the entry for `key`, preserving the order of the remaining
    /// entries, like `Map.delete`.
    pub fn remove(&mut self, key: K) -> Option<V> {
        self.position(key).map(|index| self.entries.remove(index).1)
    }

    pub fn iter(&self) -> impl Iterator<Item = &(K, V)> {
        self.entries.iter()
    }
}

impl<K: Copy + Eq + Hash, V> VecMap<K, V> {
    /// Builds a map from a pair sequence in one pass, with the exact
    /// semantics of repeated `insert`: a duplicate key keeps its first
    /// position and takes its last value. Sequential `insert` is O(n²) on a
    /// long sequence of distinct keys — the shape of a wide posting list in
    /// a serialized index — because each miss scans the whole vector.
    pub fn from_pairs_last_wins(pairs: impl IntoIterator<Item = (K, V)>) -> Self {
        let pairs = pairs.into_iter();
        let mut entries: Vec<(K, V)> = Vec::with_capacity(pairs.size_hint().0);
        let mut positions: FxHashMap<K, usize> = FxHashMap::default();
        for (key, value) in pairs {
            match positions.entry(key) {
                Entry::Occupied(entry) => entries[*entry.get()].1 = value,
                Entry::Vacant(entry) => {
                    entry.insert(entries.len());
                    entries.push((key, value));
                }
            }
        }
        VecMap { entries }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_insertion_order_through_updates_and_removals() {
        let mut map: VecMap<u32, u32> = VecMap::default();
        *map.get_or_insert_with(3, || 0) += 1;
        *map.get_or_insert_with(1, || 0) += 1;
        *map.get_or_insert_with(3, || 0) += 1;
        map.insert(2, 10);
        assert_eq!(map.len(), 3);
        assert_eq!(map.get(3), Some(&2));

        map.insert(3, 5);
        let order: Vec<u32> = map.iter().map(|&(key, _)| key).collect();
        assert_eq!(order, vec![3, 1, 2], "replacement keeps position");

        assert_eq!(map.remove(1), Some(1));
        assert_eq!(map.remove(1), None);
        let order: Vec<u32> = map.iter().map(|&(key, _)| key).collect();
        assert_eq!(order, vec![3, 2], "removal preserves order");
        assert!(!map.is_empty());
    }

    #[test]
    fn from_pairs_matches_sequential_insert() {
        let pairs = [(3u32, 1u32), (1, 2), (3, 5), (2, 10), (1, 7)];

        let mut sequential: VecMap<u32, u32> = VecMap::default();
        for &(key, value) in &pairs {
            sequential.insert(key, value);
        }
        let bulk = VecMap::from_pairs_last_wins(pairs);

        let seq: Vec<(u32, u32)> = sequential.iter().copied().collect();
        let blk: Vec<(u32, u32)> = bulk.iter().copied().collect();
        assert_eq!(blk, seq, "first position, last value");
        assert_eq!(blk, vec![(3, 5), (1, 7), (2, 10)]);
    }
}
