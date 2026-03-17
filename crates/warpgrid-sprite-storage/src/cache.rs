//! NVMe read-through cache for sprite filesystem chunks.
//!
//! Chunks are content-addressed and immutable, so cache invalidation
//! is never needed — a chunk either exists in cache or must be fetched.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::object_store::ObjectStore;

/// Cache configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Local directory for the chunk cache (ideally on NVMe).
    pub path: PathBuf,
    /// Maximum cache size in bytes.
    pub max_size_bytes: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/var/lib/warpgrid/cache"),
            max_size_bytes: 200 * 1024 * 1024 * 1024, // 200 GB
        }
    }
}

/// Read-through chunk cache backed by local filesystem (NVMe) and remote object store.
pub struct ChunkCache<S: ObjectStore> {
    config: CacheConfig,
    store: S,
}

impl<S: ObjectStore> ChunkCache<S> {
    pub fn new(config: CacheConfig, store: S) -> Self {
        Self { config, store }
    }

    /// Get a chunk by key. Checks local cache first, falls back to object store.
    pub async fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let cache_path = self.config.path.join(key);

        // Check local cache.
        if cache_path.exists() {
            debug!(key, "cache hit");
            let data = tokio::fs::read(&cache_path).await?;
            return Ok(Some(data));
        }

        // Cache miss — fetch from object store.
        debug!(key, "cache miss, fetching from object store");
        match self.store.get(key).await? {
            Some(data) => {
                // Write to local cache.
                if let Some(parent) = cache_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&cache_path, &data).await?;
                debug!(key, size = data.len(), "cached chunk locally");
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }

    /// Put a chunk into both the local cache and remote object store.
    pub async fn put(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
        // Write to remote store.
        self.store.put(key, data).await?;

        // Write to local cache.
        let cache_path = self.config.path.join(key);
        if let Some(parent) = cache_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&cache_path, data).await?;

        Ok(())
    }

    /// Evict a chunk from the local cache (does not delete from object store).
    pub async fn evict(&self, key: &str) -> anyhow::Result<()> {
        let cache_path = self.config.path.join(key);
        match tokio::fs::remove_file(&cache_path).await {
            Ok(()) => {
                debug!(key, "evicted from cache");
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object_store::FsObjectStore;

    #[tokio::test]
    async fn cache_hit_and_miss() {
        let cache_dir = tempfile::tempdir().unwrap();
        let store_dir = tempfile::tempdir().unwrap();

        let store = FsObjectStore::new(store_dir.path().to_path_buf());
        let cache = ChunkCache::new(
            CacheConfig {
                path: cache_dir.path().to_path_buf(),
                max_size_bytes: 1024 * 1024,
            },
            store,
        );

        // Put data through cache.
        cache.put("chunks/abc123", b"chunk data").await.unwrap();

        // Should be a cache hit.
        let data = cache.get("chunks/abc123").await.unwrap();
        assert_eq!(data, Some(b"chunk data".to_vec()));

        // Evict and re-fetch (should fetch from remote).
        cache.evict("chunks/abc123").await.unwrap();
        let data = cache.get("chunks/abc123").await.unwrap();
        assert_eq!(data, Some(b"chunk data".to_vec()));
    }
}
