//! A byte-capped RAM tier on moka.
use memex_types::CacheTierStats;
use moka::notification::RemovalCause;
use moka::policy::EvictionPolicy;
use moka::sync::Cache;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// What moka's own bookkeeping takes per entry beyond the key and value: its nodes, hash table slot, timestamps and `Arc`s. Set from the RSS the load test measures, rather than from its source.
const ENTRY_OVERHEAD: u64 = 512;
/// The overshoot the spec allows, and so the largest entry a tier accepts. An insert can only go over the cap by the entry going in.
const MAX_ENTRY_FRACTION: u64 = 20;
/// moka empties its queues in bounded steps, so one call may not reach the cap when many writes are waiting.
const MAX_EVICTION_ROUNDS: usize = 16;

struct Entry<V> {
    value: Arc<V>,
    weight: u32,
}

// By hand, as the derive would ask for `V: Clone` when only the `Arc` is cloned.
impl<V> Clone for Entry<V> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            weight: self.weight,
        }
    }
}

#[derive(Default)]
struct Counters {
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
    rejected: AtomicU64,
    size: AtomicU64,
    entries: AtomicU64,
}

/// A cap of 0 turns the tier off: it holds nothing, and every read is a miss that isn't counted.
pub struct RamTier<V> {
    name: &'static str,
    cap: u64,
    cache: Option<Cache<String, Entry<V>>>,
    counters: Arc<Counters>,
    /// Inserts take turns, so `size` can only pass the cap by one entry. Reads don't take it.
    writer: Mutex<()>,
}

impl<V: Send + Sync + 'static> RamTier<V> {
    pub fn new(name: &'static str, cap: u64) -> Self {
        let counters = Arc::new(Counters::default());
        let on_removal = counters.clone();
        let cache = (cap > 0).then(|| {
            Cache::builder()
                .max_capacity(cap)
                // Keys hold watermarks and versions, so an old entry is never read again however popular it was. TinyLFU would keep those and refuse new ones, and LRU lets them age out.
                .eviction_policy(EvictionPolicy::lru())
                .weigher(|_: &String, e: &Entry<V>| e.weight)
                .support_invalidation_closures()
                .eviction_listener(move |_key, entry: Entry<V>, cause| {
                    on_removal.size.fetch_sub(u64::from(entry.weight), Ordering::SeqCst);
                    on_removal.entries.fetch_sub(1, Ordering::SeqCst);
                    if cause == RemovalCause::Size {
                        on_removal.evictions.fetch_add(1, Ordering::Relaxed);
                    }
                })
                .build()
        });
        Self {
            name,
            cap,
            cache,
            counters,
            writer: Mutex::new(()),
        }
    }

    pub fn is_on(&self) -> bool {
        self.cache.is_some()
    }

    pub fn get(&self, key: &str) -> Option<Arc<V>> {
        let cache = self.cache.as_ref()?;
        match cache.get(key) {
            Some(e) => {
                self.counters.hits.fetch_add(1, Ordering::Relaxed);
                Some(e.value)
            }
            None => {
                self.counters.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// `value_bytes` is what the caller measured. An entry too big for the tier isn't kept, and the cache simply misses it next time.
    pub fn insert(&self, key: String, value: V, value_bytes: u64) {
        let Some(cache) = &self.cache else { return };
        let total = (key.len() as u64).saturating_add(value_bytes).saturating_add(ENTRY_OVERHEAD);
        let Ok(weight) = u32::try_from(total) else {
            self.counters.rejected.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if total > self.cap / MAX_ENTRY_FRACTION {
            self.counters.rejected.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let _turn = self.writer.lock().expect("lock");
        // Added first, so a reader of `size` never sees less than what's held. Replacing a key takes off the old weight in the listener.
        self.counters.size.fetch_add(total, Ordering::SeqCst);
        self.counters.entries.fetch_add(1, Ordering::SeqCst);
        cache.insert(
            key,
            Entry {
                value: Arc::new(value),
                weight,
            },
        );
        for _ in 0..MAX_EVICTION_ROUNDS {
            if self.counters.size.load(Ordering::SeqCst) <= self.cap {
                break;
            }
            cache.run_pending_tasks();
        }
    }

    /// Removes every entry whose key starts with this.
    pub fn purge(&self, prefix: &str) {
        let Some(cache) = &self.cache else { return };
        let prefix = prefix.to_string();
        if cache.invalidate_entries_if(move |k, _| k.starts_with(&prefix)).is_ok() {
            let _turn = self.writer.lock().expect("lock");
            cache.run_pending_tasks();
        }
    }

    pub fn stats(&self) -> CacheTierStats {
        let c = &self.counters;
        CacheTierStats {
            tier: self.name.to_string(),
            cap_bytes: self.cap,
            size_bytes: c.size.load(Ordering::SeqCst),
            entries: c.entries.load(Ordering::SeqCst),
            hits: c.hits.load(Ordering::Relaxed),
            misses: c.misses.load(Ordering::Relaxed),
            evictions: c.evictions.load(Ordering::Relaxed),
            rejected: c.rejected.load(Ordering::Relaxed),
            corrupt: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier(cap: u64) -> RamTier<Vec<u8>> {
        RamTier::new("t", cap)
    }

    #[test]
    fn a_value_comes_back_and_counts_as_a_hit() {
        let t = tier(1 << 20);
        assert!(t.get("k").is_none());
        t.insert("k".into(), vec![1, 2, 3], 3);
        assert_eq!(*t.get("k").unwrap(), vec![1, 2, 3]);
        let s = t.stats();
        assert_eq!((s.hits, s.misses, s.entries), (1, 1, 1));
        assert_eq!(s.size_bytes, 1 + 3 + ENTRY_OVERHEAD);
    }

    #[test]
    fn replacing_a_key_does_not_count_it_twice() {
        let t = tier(1 << 20);
        for _ in 0..50 {
            t.insert("k".into(), vec![0; 100], 100);
        }
        t.cache.as_ref().unwrap().run_pending_tasks();
        let s = t.stats();
        assert_eq!((s.entries, s.size_bytes), (1, 1 + 100 + ENTRY_OVERHEAD));
    }

    #[test]
    fn a_cap_of_zero_holds_nothing_and_counts_nothing() {
        let t = tier(0);
        assert!(!t.is_on());
        t.insert("k".into(), vec![1], 1);
        assert!(t.get("k").is_none());
        t.purge("k");
        let s = t.stats();
        assert_eq!((s.hits, s.misses, s.entries, s.size_bytes), (0, 0, 0, 0));
    }

    #[test]
    fn an_entry_over_a_twentieth_of_the_cap_is_refused() {
        let t = tier(20_000);
        t.insert("big".into(), vec![0; 5000], 5000);
        assert!(t.get("big").is_none());
        assert_eq!(t.stats().rejected, 1);
        t.insert("ok".into(), vec![0; 100], 100);
        assert!(t.get("ok").is_some());
    }

    #[test]
    fn an_impossible_weight_is_refused_not_wrapped() {
        let t = tier(u64::MAX);
        t.insert("k".into(), vec![], u64::MAX);
        assert!(t.get("k").is_none());
        assert_eq!(t.stats().rejected, 1);
    }

    #[test]
    fn eviction_holds_the_cap_and_is_counted() {
        let cap = 100_000;
        let t = tier(cap);
        for i in 0..5000 {
            t.insert(format!("key{i}"), vec![0; 1000], 1000);
            assert!(t.stats().size_bytes <= cap + cap / MAX_ENTRY_FRACTION);
        }
        let s = t.stats();
        assert!(s.evictions > 4000, "{s:?}");
        assert!(s.entries < 100, "{s:?}");
    }

    #[test]
    fn the_oldest_goes_first_so_dead_keys_age_out() {
        let t = tier(100_000);
        for _ in 0..20 {
            t.insert("popular-but-dead".into(), vec![0; 1000], 1000);
            t.get("popular-but-dead");
        }
        for i in 0..500 {
            t.insert(format!("new{i}"), vec![0; 1000], 1000);
        }
        assert!(t.get("popular-but-dead").is_none());
        assert!(t.get("new499").is_some());
    }

    #[test]
    fn purging_removes_only_that_prefix() {
        let t = tier(1 << 20);
        t.insert("a|1".into(), vec![1], 1);
        t.insert("a|2".into(), vec![1], 1);
        t.insert("b|1".into(), vec![1], 1);
        t.purge("a|");
        assert!(t.get("a|1").is_none() && t.get("a|2").is_none());
        assert!(t.get("b|1").is_some());
    }
}
