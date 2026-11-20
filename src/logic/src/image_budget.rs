//! Byte-budgeted least-recently-used bookkeeping.
//!
//! The desktop shell decodes images at display size and keeps the decoded
//! pixels in memory so a re-render does not decode again. Those pixels are the
//! app's largest controllable allocation, so the shell caps them with this
//! budget. The module is pure: the shell owns the images and passes their
//! weights in, and calls [`ByteBudget::evict_to_budget`] with the set of keys
//! the current view still needs. Eviction is least-recently-used first, and a
//! protected key is never evicted: a screen that needs more than the budget
//! may temporarily exceed it instead of flickering.

use std::collections::HashMap;
use std::hash::Hash;

/// One tracked key.
#[derive(Debug, Clone, Copy)]
struct Entry {
    /// Decoded bytes this key is accountable for.
    weight: u64,
    /// Caller clock (`now`) of the last [`ByteBudget::touch`] or insert.
    last_use: u64,
    /// Insertion order, the deterministic tie-breaker when `last_use` is equal.
    sequence: u64,
}

/// Weighted keys with a total-byte budget and least-recently-used eviction.
#[derive(Debug, Default)]
pub struct ByteBudget<K> {
    entries: HashMap<K, Entry>,
    total: u64,
    next_sequence: u64,
}

impl<K> ByteBudget<K>
where
    K: Eq + Hash + Clone,
{
    /// An empty budget.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            total: 0,
            next_sequence: 0,
        }
    }

    /// Insert `key` with `weight`, or replace its weight if it already exists.
    ///
    /// Returns the previous weight, if the key was tracked. In both cases the
    /// key's last-use clock becomes `now`. The insertion order is kept across
    /// an update so a key that keeps being touched does not move to the back
    /// of the eviction order.
    pub fn insert(&mut self, key: K, weight: u64, now: u64) -> Option<u64> {
        match self.entries.get_mut(&key) {
            Some(entry) => {
                let previous = entry.weight;
                self.total = self.total.saturating_sub(previous);
                entry.weight = weight;
                entry.last_use = now;
                self.total = self.total.saturating_add(weight);
                Some(previous)
            }
            None => {
                let sequence = self.next_sequence;
                self.next_sequence = self.next_sequence.wrapping_add(1);
                self.entries.insert(
                    key,
                    Entry {
                        weight,
                        last_use: now,
                        sequence,
                    },
                );
                self.total = self.total.saturating_add(weight);
                None
            }
        }
    }

    /// Update an existing key's weight. Returns false when the key is absent.
    pub fn set_weight(&mut self, key: &K, weight: u64) -> bool {
        let Some(entry) = self.entries.get_mut(key) else {
            return false;
        };
        self.total = self.total.saturating_sub(entry.weight);
        entry.weight = weight;
        self.total = self.total.saturating_add(weight);
        true
    }

    /// Mark `key` as used at `now`. Returns false when the key is absent.
    pub fn touch(&mut self, key: &K, now: u64) -> bool {
        let Some(entry) = self.entries.get_mut(key) else {
            return false;
        };
        entry.last_use = now;
        true
    }

    /// Forget `key`, returning the weight it accounted for.
    pub fn remove(&mut self, key: &K) -> Option<u64> {
        let entry = self.entries.remove(key)?;
        self.total = self.total.saturating_sub(entry.weight);
        Some(entry.weight)
    }

    /// Whether `key` is tracked.
    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// The weight of `key`, if tracked.
    pub fn weight(&self, key: &K) -> Option<u64> {
        self.entries.get(key).map(|entry| entry.weight)
    }

    /// The last-use clock recorded for `key`, if tracked.
    pub fn last_use(&self, key: &K) -> Option<u64> {
        self.entries.get(key).map(|entry| entry.last_use)
    }

    /// Number of tracked keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no key is tracked.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Sum of the tracked weights.
    pub fn total_bytes(&self) -> u64 {
        self.total
    }

    /// The tracked keys, in no particular order.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.keys()
    }

    /// Empty the budget, returning how many keys it held.
    pub fn clear(&mut self) -> usize {
        let held = self.entries.len();
        self.entries.clear();
        self.total = 0;
        held
    }

    /// Evict least-recently-used unprotected keys until the total fits
    /// `budget`, returning the evicted keys in eviction order.
    ///
    /// `protected` answers whether the current view still needs a key; those
    /// keys are never evicted, so the returned total may still exceed the
    /// budget when the protected set alone is larger. Equal `last_use` clocks
    /// evict the earliest inserted key first, so the order is deterministic.
    pub fn evict_to_budget(&mut self, budget: u64, protected: impl Fn(&K) -> bool) -> Vec<K> {
        let mut evicted = Vec::new();
        while self.total > budget {
            let mut victim: Option<(&K, Entry)> = None;
            for (key, entry) in &self.entries {
                if protected(key) {
                    continue;
                }
                let better = match victim {
                    None => true,
                    Some((_, current)) => {
                        (entry.last_use, entry.sequence) < (current.last_use, current.sequence)
                    }
                };
                if better {
                    victim = Some((key, *entry));
                }
            }
            let Some((key, _)) = victim else {
                // Everything left is protected: the view needs more than the
                // budget, keep it rather than flicker.
                break;
            };
            let key = key.clone();
            self.remove(&key);
            evicted.push(key);
        }
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(keys: &[(&'static str, u64, u64)]) -> ByteBudget<&'static str> {
        let mut budget = ByteBudget::new();
        for (key, weight, now) in keys {
            budget.insert(*key, *weight, *now);
        }
        budget
    }

    #[test]
    fn insert_replace_and_remove_track_the_total() {
        let mut budget = ByteBudget::new();
        assert_eq!(budget.insert("a", 10, 1), None);
        assert_eq!(budget.insert("b", 20, 2), None);
        assert_eq!(budget.total_bytes(), 30);
        assert_eq!(budget.insert("a", 15, 3), Some(10));
        assert_eq!(budget.total_bytes(), 35);
        assert_eq!(budget.weight(&"a"), Some(15));
        assert_eq!(budget.remove(&"a"), Some(15));
        assert_eq!(budget.total_bytes(), 20);
        assert!(!budget.contains(&"a"));
        assert_eq!(budget.len(), 1);
    }

    #[test]
    fn touch_reorders_eviction_but_duplicate_insert_keeps_place() {
        let mut budget = budget(&[("a", 10, 1), ("b", 10, 2), ("c", 10, 3)]);
        assert!(budget.touch(&"a", 4));
        // b is now the least recently used.
        let evicted = budget.evict_to_budget(25, |_| false);
        assert_eq!(evicted, vec!["b"]);
        assert_eq!(budget.total_bytes(), 20);
    }

    #[test]
    fn equal_last_use_evicts_earliest_inserted_first() {
        let mut budget = budget(&[("a", 10, 7), ("b", 10, 7), ("c", 10, 7)]);
        let evicted = budget.evict_to_budget(10, |_| false);
        assert_eq!(evicted, vec!["a", "b"]);
        assert_eq!(budget.total_bytes(), 10);
    }

    #[test]
    fn protected_keys_are_never_evicted_and_may_exceed_budget() {
        let mut budget = budget(&[("a", 10, 1), ("b", 10, 2)]);
        let evicted = budget.evict_to_budget(5, |_| true);
        assert!(evicted.is_empty());
        assert_eq!(budget.total_bytes(), 20);
        // Unprotecting one key lets it go.
        let evicted = budget.evict_to_budget(15, |key| *key != "b");
        assert_eq!(evicted, vec!["b"]);
        assert_eq!(budget.total_bytes(), 10);
    }

    #[test]
    fn a_zero_budget_evicts_every_unprotected_key() {
        let mut budget = budget(&[("a", 10, 1), ("b", 20, 2)]);
        let evicted = budget.evict_to_budget(0, |_| false);
        assert_eq!(evicted, vec!["a", "b"]);
        assert!(budget.is_empty());
        assert_eq!(budget.total_bytes(), 0);
    }

    #[test]
    fn eviction_stops_when_the_budget_is_met() {
        let mut budget = budget(&[("a", 10, 1), ("b", 20, 2), ("c", 30, 3)]);
        let evicted = budget.evict_to_budget(45, |_| false);
        assert_eq!(evicted, vec!["a", "b"]);
        assert_eq!(budget.len(), 1);
    }

    #[test]
    fn set_weight_resizes_without_touching_or_reordering() {
        let mut budget = budget(&[("a", 10, 1), ("b", 10, 2)]);
        assert!(budget.set_weight(&"a", 100));
        assert!(!budget.set_weight(&"missing", 10));
        assert_eq!(budget.weight(&"a"), Some(100));
        assert_eq!(budget.last_use(&"a"), Some(1));
        assert_eq!(budget.total_bytes(), 110);
        // a was inserted first and never touched: its heavier weight does not
        // change the least-recently-used order.
        let evicted = budget.evict_to_budget(20, |_| false);
        assert_eq!(evicted, vec!["a"]);
    }

    #[test]
    fn clear_empties_the_budget() {
        let mut budget = budget(&[("a", 10, 1)]);
        assert_eq!(budget.clear(), 1);
        assert!(budget.is_empty());
        assert_eq!(budget.total_bytes(), 0);
    }

    #[test]
    fn huge_weights_saturate_instead_of_overflowing() {
        let mut budget = ByteBudget::new();
        budget.insert("a", u64::MAX, 1);
        budget.insert("b", u64::MAX, 2);
        assert_eq!(budget.total_bytes(), u64::MAX);
        budget.remove(&"a");
        budget.remove(&"b");
        assert_eq!(budget.total_bytes(), 0);
    }

    #[test]
    fn a_long_touch_sequence_keeps_invariants() {
        let mut budget = ByteBudget::new();
        for step in 0..10_000u64 {
            let key = format!("k{}", step % 500);
            if step % 3 == 0 {
                budget.insert(key.clone(), 1_000, step);
            } else {
                budget.touch(&key, step);
            }
            assert_eq!(
                budget.total_bytes(),
                budget.weight_sum_for_test(),
                "the running total must match the tracked weights"
            );
        }
        let evicted = budget.evict_to_budget(64_000, |_| false);
        assert!(!evicted.is_empty());
        assert!(budget.total_bytes() <= 64_000);
        assert_eq!(budget.total_bytes(), budget.weight_sum_for_test());
    }

    impl<K> ByteBudget<K>
    where
        K: Eq + Hash + Clone,
    {
        #[cfg(test)]
        fn weight_sum_for_test(&self) -> u64 {
            self.entries.values().map(|entry| entry.weight).sum()
        }
    }
}
