//! A least-recently-used map bounded by a byte budget.
//!
//! Used for rendered tiles (GPU textures in the UI). Entries touched in the
//! current frame are never evicted, so a frame always has what it is drawing
//! even when the budget is momentarily exceeded.

use std::collections::HashMap;
use std::hash::Hash;

struct Entry<V> {
    value: V,
    bytes: usize,
    last_used: u64,
}

pub struct ByteLru<K, V> {
    map: HashMap<K, Entry<V>>,
    budget: usize,
    used: usize,
    clock: u64,
    /// Entries used at or after this tick are pinned for the current frame.
    frame_start: u64,
}

impl<K: Hash + Eq + Clone, V> ByteLru<K, V> {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            budget: budget_bytes,
            used: 0,
            clock: 1,
            frame_start: 1,
        }
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    pub fn set_budget(&mut self, budget_bytes: usize) {
        self.budget = budget_bytes;
        self.evict();
    }

    pub fn used_bytes(&self) -> usize {
        self.used
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Marks the start of a frame: entries used from now on are pinned.
    pub fn begin_frame(&mut self) {
        self.clock += 1;
        self.frame_start = self.clock;
    }

    pub fn contains(&self, k: &K) -> bool {
        self.map.contains_key(k)
    }

    /// Looks up and marks as used.
    pub fn get(&mut self, k: &K) -> Option<&V> {
        let clock = self.clock;
        self.map.get_mut(k).map(|e| {
            e.last_used = clock;
            &e.value
        })
    }

    /// Looks up without touching recency.
    pub fn peek(&self, k: &K) -> Option<&V> {
        self.map.get(k).map(|e| &e.value)
    }

    pub fn insert(&mut self, k: K, value: V, bytes: usize) {
        if let Some(old) = self.map.insert(
            k,
            Entry {
                value,
                bytes,
                last_used: self.clock,
            },
        ) {
            self.used -= old.bytes;
        }
        self.used += bytes;
        self.evict();
    }

    pub fn remove(&mut self, k: &K) -> Option<V> {
        self.map.remove(k).map(|e| {
            self.used -= e.bytes;
            e.value
        })
    }

    /// Removes every entry matching `pred`.
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let used = &mut self.used;
        self.map.retain(|k, e| {
            let k2 = keep(k);
            if !k2 {
                *used -= e.bytes;
            }
            k2
        });
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.map.keys()
    }

    fn evict(&mut self) {
        if self.used <= self.budget {
            return;
        }
        let mut victims: Vec<(u64, K)> = self
            .map
            .iter()
            .filter(|(_, e)| e.last_used < self.frame_start)
            .map(|(k, e)| (e.last_used, k.clone()))
            .collect();
        victims.sort_unstable_by_key(|v| v.0);
        for (_, k) in victims {
            if self.used <= self.budget {
                break;
            }
            if let Some(e) = self.map.remove(&k) {
                self.used -= e.bytes;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recent_first() {
        let mut c = ByteLru::new(30);
        c.insert(1, "a", 10);
        c.begin_frame();
        c.insert(2, "b", 10);
        c.begin_frame();
        c.insert(3, "c", 10);
        c.begin_frame();
        assert!(c.get(&1).is_some()); // 1 is now the most recent
        c.begin_frame();
        c.insert(4, "d", 10);
        assert!(c.contains(&1) && !c.contains(&2) && c.contains(&3) && c.contains(&4));
        assert_eq!(c.used_bytes(), 30);
    }

    #[test]
    fn current_frame_is_pinned() {
        let mut c = ByteLru::new(15);
        c.begin_frame();
        c.insert(1, (), 10);
        c.insert(2, (), 10);
        // Over budget, but both were used this frame.
        assert_eq!(c.len(), 2);
        c.begin_frame();
        c.insert(3, (), 1);
        // One older entry goes; that is enough to get back under budget.
        assert_eq!(c.len(), 2);
        assert!(c.contains(&3));
        assert_eq!(c.used_bytes(), 11);
    }

    #[test]
    fn replace_and_remove_keep_accounting() {
        let mut c = ByteLru::new(100);
        c.insert("k", 1, 40);
        c.insert("k", 2, 10);
        assert_eq!(c.used_bytes(), 10);
        assert_eq!(c.peek(&"k"), Some(&2));
        assert_eq!(c.remove(&"k"), Some(2));
        assert_eq!(c.used_bytes(), 0);
        c.insert("a", 1, 5);
        c.insert("b", 2, 5);
        c.retain(|k| *k == "a");
        assert_eq!(c.used_bytes(), 5);
        c.begin_frame();
        c.set_budget(0);
        assert!(c.is_empty());
    }
}
