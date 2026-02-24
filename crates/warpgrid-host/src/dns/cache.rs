//! DNS cache with TTL expiration, LRU eviction, and round-robin address selection.
//!
//! Provides a bounded cache for DNS resolution results. Each entry stores
//! resolved IP addresses with a TTL. When multiple addresses are cached for
//! a hostname, [`DnsCache::get_round_robin`] returns them in rotating order
//! using a per-entry atomic counter (no mutex contention on the hot path).
//!
//! Cache statistics (hits, misses, evictions) are emitted as `tracing::info`
//! metrics.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Configuration for the DNS cache.
#[derive(Clone, Debug)]
pub struct DnsCacheConfig {
    /// Time-to-live for cache entries (default: 30 seconds).
    pub ttl: Duration,
    /// Maximum number of entries in the cache (default: 1024).
    pub max_entries: usize,
}

impl Default for DnsCacheConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(30),
            max_entries: 1024,
        }
    }
}

/// A single cached DNS entry with TTL tracking and round-robin state.
struct CacheEntry {
    /// Resolved IP addresses.
    addresses: Vec<IpAddr>,
    /// When this entry was inserted (for TTL calculation).
    inserted_at: Instant,
    /// Atomic counter for round-robin address selection.
    round_robin_counter: AtomicUsize,
    /// Last access time for LRU tracking (stored as nanos since cache creation).
    last_accessed_nanos: AtomicU64,
}

impl CacheEntry {
    fn new(addresses: Vec<IpAddr>, cache_epoch: Instant) -> Self {
        let now = Instant::now();
        let nanos = now.duration_since(cache_epoch).as_nanos() as u64;
        Self {
            addresses,
            inserted_at: now,
            round_robin_counter: AtomicUsize::new(0),
            last_accessed_nanos: AtomicU64::new(nanos),
        }
    }

    fn is_expired(&self, ttl: Duration) -> bool {
        self.inserted_at.elapsed() > ttl
    }

    fn touch(&self, cache_epoch: Instant) {
        let nanos = Instant::now().duration_since(cache_epoch).as_nanos() as u64;
        self.last_accessed_nanos.store(nanos, Ordering::Relaxed);
    }

    /// Get the next address in round-robin order.
    fn next_round_robin(&self) -> Option<IpAddr> {
        if self.addresses.is_empty() {
            return None;
        }
        let idx = self.round_robin_counter.fetch_add(1, Ordering::Relaxed);
        Some(self.addresses[idx % self.addresses.len()])
    }
}

/// Thread-safe DNS cache with TTL, LRU eviction, and round-robin selection.
///
/// # Concurrency model
///
/// The cache itself is **not internally synchronized** — it must be wrapped
/// in a `Mutex` (or `RwLock`) by the caller if concurrent access is needed.
/// However, the round-robin counters within entries are atomic, so once a
/// reference to an entry is obtained, round-robin selection is lock-free.
pub struct DnsCache {
    /// Hostname → cached entry.
    entries: HashMap<String, CacheEntry>,
    /// Cache configuration (TTL, max size).
    config: DnsCacheConfig,
    /// Epoch used for LRU time tracking.
    epoch: Instant,
    /// Cache statistics.
    stats: CacheStats,
}

/// Accumulated cache statistics.
struct CacheStats {
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl DnsCache {
    /// Create a new DNS cache with the given configuration.
    pub fn new(config: DnsCacheConfig) -> Self {
        Self {
            entries: HashMap::new(),
            config,
            epoch: Instant::now(),
            stats: CacheStats {
                hits: 0,
                misses: 0,
                evictions: 0,
            },
        }
    }

    /// Look up a hostname in the cache, returning all addresses if present and not expired.
    ///
    /// Returns `None` on cache miss or TTL expiration. Expired entries are
    /// removed eagerly.
    pub fn get(&mut self, hostname: &str) -> Option<&[IpAddr]> {
        let key = hostname.to_lowercase();

        // Check if entry exists and is not expired
        if let Some(entry) = self.entries.get(&key) {
            if entry.is_expired(self.config.ttl) {
                // Expired — remove and count as miss
                self.entries.remove(&key);
                self.stats.misses += 1;
                tracing::info!(
                    hostname = %hostname,
                    cache_hits = self.stats.hits,
                    cache_misses = self.stats.misses,
                    cache_evictions = self.stats.evictions,
                    "dns cache miss (expired)"
                );
                return None;
            }
        } else {
            self.stats.misses += 1;
            tracing::info!(
                hostname = %hostname,
                cache_hits = self.stats.hits,
                cache_misses = self.stats.misses,
                cache_evictions = self.stats.evictions,
                "dns cache miss"
            );
            return None;
        }

        // Entry exists and is valid — update access time and count as hit
        let entry = self.entries.get(&key).unwrap();
        entry.touch(self.epoch);
        self.stats.hits += 1;
        tracing::info!(
            hostname = %hostname,
            cache_hits = self.stats.hits,
            cache_misses = self.stats.misses,
            cache_evictions = self.stats.evictions,
            "dns cache hit"
        );
        Some(&entry.addresses)
    }

    /// Get the next address for a hostname in round-robin order.
    ///
    /// Returns `None` on cache miss or TTL expiration.
    pub fn get_round_robin(&mut self, hostname: &str) -> Option<IpAddr> {
        let key = hostname.to_lowercase();

        // Check expiration first
        if let Some(entry) = self.entries.get(&key) {
            if entry.is_expired(self.config.ttl) {
                self.entries.remove(&key);
                self.stats.misses += 1;
                tracing::info!(
                    hostname = %hostname,
                    cache_hits = self.stats.hits,
                    cache_misses = self.stats.misses,
                    cache_evictions = self.stats.evictions,
                    "dns cache miss (expired, round-robin)"
                );
                return None;
            }
        } else {
            self.stats.misses += 1;
            tracing::info!(
                hostname = %hostname,
                cache_hits = self.stats.hits,
                cache_misses = self.stats.misses,
                cache_evictions = self.stats.evictions,
                "dns cache miss (round-robin)"
            );
            return None;
        }

        let entry = self.entries.get(&key).unwrap();
        entry.touch(self.epoch);
        self.stats.hits += 1;
        let addr = entry.next_round_robin();
        tracing::info!(
            hostname = %hostname,
            address = ?addr,
            cache_hits = self.stats.hits,
            cache_misses = self.stats.misses,
            cache_evictions = self.stats.evictions,
            "dns cache hit (round-robin)"
        );
        addr
    }

    /// Insert or update a cache entry.
    ///
    /// If the cache is at capacity, the least-recently-used entry is evicted.
    pub fn insert(&mut self, hostname: &str, addresses: Vec<IpAddr>) {
        let key = hostname.to_lowercase();

        // If key already exists, replace in-place (no capacity check needed)
        if self.entries.contains_key(&key) {
            self.entries
                .insert(key, CacheEntry::new(addresses, self.epoch));
            return;
        }

        // Evict if at capacity
        if self.entries.len() >= self.config.max_entries {
            self.evict_lru();
        }

        self.entries
            .insert(key, CacheEntry::new(addresses, self.epoch));
    }

    /// Evict the least-recently-used entry.
    fn evict_lru(&mut self) {
        if self.entries.is_empty() {
            return;
        }

        // Find the entry with the smallest last_accessed_nanos
        let lru_key = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_accessed_nanos.load(Ordering::Relaxed))
            .map(|(key, _)| key.clone());

        if let Some(key) = lru_key {
            self.entries.remove(&key);
            self.stats.evictions += 1;
            tracing::info!(
                evicted_hostname = %key,
                cache_evictions = self.stats.evictions,
                "dns cache LRU eviction"
            );
        }
    }

    /// Get current cache statistics.
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.stats.hits, self.stats.misses, self.stats.evictions)
    }

    /// Get the number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::thread;

    // ── Construction ─────────────────────────────────────────────────

    #[test]
    fn new_cache_is_empty() {
        let cache = DnsCache::new(DnsCacheConfig::default());
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn default_config_values() {
        let config = DnsCacheConfig::default();
        assert_eq!(config.ttl, Duration::from_secs(30));
        assert_eq!(config.max_entries, 1024);
    }

    // ── Insert and Get ───────────────────────────────────────────────

    #[test]
    fn insert_and_get_single_address() {
        let mut cache = DnsCache::new(DnsCacheConfig::default());
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        cache.insert("myhost", vec![addr]);

        let result = cache.get("myhost");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), &[addr]);
    }

    #[test]
    fn insert_and_get_multiple_addresses() {
        let mut cache = DnsCache::new(DnsCacheConfig::default());
        let addrs = vec![
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3)),
        ];

        cache.insert("multi", addrs.clone());

        let result = cache.get("multi");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), &addrs);
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let mut cache = DnsCache::new(DnsCacheConfig::default());
        assert!(cache.get("nonexistent").is_none());
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let mut cache = DnsCache::new(DnsCacheConfig::default());
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        cache.insert("MyHost.Local", vec![addr]);

        assert!(cache.get("myhost.local").is_some());
        assert!(cache.get("MYHOST.LOCAL").is_some());
        assert!(cache.get("MyHost.Local").is_some());
    }

    #[test]
    fn insert_replaces_existing_entry() {
        let mut cache = DnsCache::new(DnsCacheConfig::default());
        let addr1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let addr2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        cache.insert("host", vec![addr1]);
        cache.insert("host", vec![addr2]);

        let result = cache.get("host").unwrap();
        assert_eq!(result, &[addr2]);
    }

    // ── TTL Expiration ───────────────────────────────────────────────

    #[test]
    fn entry_expires_after_ttl() {
        let config = DnsCacheConfig {
            ttl: Duration::from_millis(50),
            max_entries: 1024,
        };
        let mut cache = DnsCache::new(config);
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        cache.insert("expiring", vec![addr]);
        assert!(cache.get("expiring").is_some());

        // Wait for TTL to expire
        thread::sleep(Duration::from_millis(80));

        assert!(cache.get("expiring").is_none());
        // Entry should be removed
        assert!(cache.is_empty());
    }

    #[test]
    fn entry_valid_within_ttl() {
        let config = DnsCacheConfig {
            ttl: Duration::from_secs(30),
            max_entries: 1024,
        };
        let mut cache = DnsCache::new(config);
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        cache.insert("valid", vec![addr]);

        // Should still be valid immediately
        assert!(cache.get("valid").is_some());
    }

    #[test]
    fn expired_entry_removed_on_access() {
        let config = DnsCacheConfig {
            ttl: Duration::from_millis(50),
            max_entries: 1024,
        };
        let mut cache = DnsCache::new(config);
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        cache.insert("temp", vec![addr]);
        assert_eq!(cache.len(), 1);

        thread::sleep(Duration::from_millis(80));

        // Access triggers lazy removal
        cache.get("temp");
        assert_eq!(cache.len(), 0);
    }

    // ── Round-Robin Selection ────────────────────────────────────────

    #[test]
    fn round_robin_cycles_through_addresses() {
        let mut cache = DnsCache::new(DnsCacheConfig::default());
        let addrs = vec![
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3)),
        ];

        cache.insert("rr-host", addrs.clone());

        // First cycle
        assert_eq!(cache.get_round_robin("rr-host"), Some(addrs[0]));
        assert_eq!(cache.get_round_robin("rr-host"), Some(addrs[1]));
        assert_eq!(cache.get_round_robin("rr-host"), Some(addrs[2]));

        // Wraps around
        assert_eq!(cache.get_round_robin("rr-host"), Some(addrs[0]));
        assert_eq!(cache.get_round_robin("rr-host"), Some(addrs[1]));
    }

    #[test]
    fn round_robin_single_address_always_returns_same() {
        let mut cache = DnsCache::new(DnsCacheConfig::default());
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        cache.insert("single", vec![addr]);

        for _ in 0..5 {
            assert_eq!(cache.get_round_robin("single"), Some(addr));
        }
    }

    #[test]
    fn round_robin_nonexistent_returns_none() {
        let mut cache = DnsCache::new(DnsCacheConfig::default());
        assert_eq!(cache.get_round_robin("nonexistent"), None);
    }

    #[test]
    fn round_robin_expired_returns_none() {
        let config = DnsCacheConfig {
            ttl: Duration::from_millis(50),
            max_entries: 1024,
        };
        let mut cache = DnsCache::new(config);
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        cache.insert("expiring", vec![addr]);
        assert!(cache.get_round_robin("expiring").is_some());

        thread::sleep(Duration::from_millis(80));

        assert_eq!(cache.get_round_robin("expiring"), None);
    }

    #[test]
    fn round_robin_counter_resets_on_reinsert() {
        let mut cache = DnsCache::new(DnsCacheConfig::default());
        let addrs = vec![
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
        ];

        cache.insert("host", addrs.clone());

        // Advance counter
        assert_eq!(cache.get_round_robin("host"), Some(addrs[0]));

        // Re-insert resets counter
        cache.insert("host", addrs.clone());
        assert_eq!(cache.get_round_robin("host"), Some(addrs[0]));
    }

    #[test]
    fn round_robin_is_case_insensitive() {
        let mut cache = DnsCache::new(DnsCacheConfig::default());
        let addrs = vec![
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
        ];

        cache.insert("MyHost", addrs.clone());

        // Mixed case lookups share the same counter
        assert_eq!(cache.get_round_robin("myhost"), Some(addrs[0]));
        assert_eq!(cache.get_round_robin("MYHOST"), Some(addrs[1]));
        assert_eq!(cache.get_round_robin("MyHost"), Some(addrs[0])); // wraps
    }

    // ── LRU Eviction ─────────────────────────────────────────────────

    #[test]
    fn evicts_lru_when_at_capacity() {
        let config = DnsCacheConfig {
            ttl: Duration::from_secs(30),
            max_entries: 3,
        };
        let mut cache = DnsCache::new(config);

        let addr = |n: u8| IpAddr::V4(Ipv4Addr::new(10, 0, 0, n));

        // Fill to capacity
        cache.insert("host-a", vec![addr(1)]);
        thread::sleep(Duration::from_millis(5)); // ensure distinct timestamps
        cache.insert("host-b", vec![addr(2)]);
        thread::sleep(Duration::from_millis(5));
        cache.insert("host-c", vec![addr(3)]);
        assert_eq!(cache.len(), 3);

        // Access host-a to make it recently used (host-b is now LRU)
        cache.get("host-a");
        thread::sleep(Duration::from_millis(5));

        // Insert a 4th entry — should evict host-b (least recently used)
        cache.insert("host-d", vec![addr(4)]);
        assert_eq!(cache.len(), 3);

        assert!(cache.get("host-a").is_some()); // recently accessed
        assert!(cache.get("host-b").is_none()); // evicted
        assert!(cache.get("host-c").is_some()); // was accessed by the get above or still in
        assert!(cache.get("host-d").is_some()); // newly inserted
    }

    #[test]
    fn eviction_tracks_statistics() {
        let config = DnsCacheConfig {
            ttl: Duration::from_secs(30),
            max_entries: 2,
        };
        let mut cache = DnsCache::new(config);
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        cache.insert("host-1", vec![addr]);
        thread::sleep(Duration::from_millis(5));
        cache.insert("host-2", vec![addr]);
        thread::sleep(Duration::from_millis(5));

        // This triggers 1 eviction
        cache.insert("host-3", vec![addr]);

        let (_, _, evictions) = cache.stats();
        assert_eq!(evictions, 1);
    }

    #[test]
    fn no_eviction_when_replacing_existing_key() {
        let config = DnsCacheConfig {
            ttl: Duration::from_secs(30),
            max_entries: 2,
        };
        let mut cache = DnsCache::new(config);
        let addr1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let addr2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        cache.insert("host-1", vec![addr1]);
        cache.insert("host-2", vec![addr1]);

        // Replace existing key — should NOT evict
        cache.insert("host-1", vec![addr2]);

        assert_eq!(cache.len(), 2);
        let (_, _, evictions) = cache.stats();
        assert_eq!(evictions, 0);
    }

    // ── Cache Statistics ─────────────────────────────────────────────

    #[test]
    fn stats_track_hits_and_misses() {
        let mut cache = DnsCache::new(DnsCacheConfig::default());
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        cache.insert("cached", vec![addr]);

        // 2 hits
        cache.get("cached");
        cache.get("cached");

        // 2 misses
        cache.get("not-cached");
        cache.get("also-not-cached");

        let (hits, misses, _) = cache.stats();
        assert_eq!(hits, 2);
        assert_eq!(misses, 2);
    }

    #[test]
    fn stats_count_expired_as_miss() {
        let config = DnsCacheConfig {
            ttl: Duration::from_millis(50),
            max_entries: 1024,
        };
        let mut cache = DnsCache::new(config);
        let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        cache.insert("expiring", vec![addr]);

        // 1 hit
        cache.get("expiring");

        thread::sleep(Duration::from_millis(80));

        // 1 miss (expired)
        cache.get("expiring");

        let (hits, misses, _) = cache.stats();
        assert_eq!(hits, 1);
        assert_eq!(misses, 1);
    }

    #[test]
    fn initial_stats_are_zero() {
        let cache = DnsCache::new(DnsCacheConfig::default());
        let (hits, misses, evictions) = cache.stats();
        assert_eq!(hits, 0);
        assert_eq!(misses, 0);
        assert_eq!(evictions, 0);
    }

    // ── Bounded capacity ─────────────────────────────────────────────

    #[test]
    fn cache_never_exceeds_max_entries() {
        let config = DnsCacheConfig {
            ttl: Duration::from_secs(30),
            max_entries: 5,
        };
        let mut cache = DnsCache::new(config);

        for i in 0..20u8 {
            let addr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, i));
            cache.insert(&format!("host-{i}"), vec![addr]);
            assert!(cache.len() <= 5);
        }
    }

    // ── Edge cases ───────────────────────────────────────────────────

    #[test]
    fn insert_empty_addresses() {
        let mut cache = DnsCache::new(DnsCacheConfig::default());
        cache.insert("empty", vec![]);

        let result = cache.get("empty");
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn round_robin_with_empty_addresses_returns_none() {
        let mut cache = DnsCache::new(DnsCacheConfig::default());
        cache.insert("empty", vec![]);

        assert_eq!(cache.get_round_robin("empty"), None);
    }

    #[test]
    fn max_entries_of_one() {
        let config = DnsCacheConfig {
            ttl: Duration::from_secs(30),
            max_entries: 1,
        };
        let mut cache = DnsCache::new(config);
        let addr1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let addr2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        cache.insert("first", vec![addr1]);
        assert_eq!(cache.len(), 1);

        cache.insert("second", vec![addr2]);
        assert_eq!(cache.len(), 1);

        // First was evicted
        assert!(cache.get("first").is_none());
        assert!(cache.get("second").is_some());
    }
}
