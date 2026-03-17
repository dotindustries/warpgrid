//! S3-compatible object storage client for sprite data.
//!
//! Wraps MinIO/SeaweedFS with a simple get/put/delete/list interface.
//! All sprite filesystem chunks and metadata snapshots live here.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Configuration for the object store backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectStoreConfig {
    /// S3-compatible endpoint URL (e.g., "http://minio.internal:9000").
    pub endpoint: String,
    /// Bucket name for sprite data.
    pub bucket: String,
    /// Access key (resolved from environment variable).
    pub access_key: String,
    /// Secret key (resolved from environment variable).
    pub secret_key: String,
    /// Optional region (default: "us-east-1" for MinIO compatibility).
    pub region: Option<String>,
}

/// Trait for object storage operations.
pub trait ObjectStore: Send + Sync {
    /// Upload data to the given key.
    fn put(
        &self,
        key: &str,
        data: &[u8],
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    /// Download data by key. Returns None if not found.
    fn get(
        &self,
        key: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<Option<Vec<u8>>>> + Send;

    /// Delete an object by key.
    fn delete(
        &self,
        key: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    /// List objects under a prefix.
    fn list(
        &self,
        prefix: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<String>>> + Send;

    /// Check if an object exists.
    fn exists(
        &self,
        key: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<bool>> + Send;
}

/// S3-compatible object store implementation (MinIO, SeaweedFS, AWS S3).
pub struct S3ObjectStore {
    config: ObjectStoreConfig,
}

impl S3ObjectStore {
    pub fn new(config: ObjectStoreConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &ObjectStoreConfig {
        &self.config
    }

    /// Build the URL for an object key.
    fn object_url(&self, key: &str) -> String {
        format!("{}/{}/{}", self.config.endpoint, self.config.bucket, key)
    }
}

impl ObjectStore for S3ObjectStore {
    async fn put(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
        tracing::debug!(
            key,
            size = data.len(),
            url = %self.object_url(key),
            "object store put"
        );
        // Real implementation would use S3 PutObject API with SigV4 auth.
        // For now, structural placeholder.
        Ok(())
    }

    async fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        tracing::debug!(key, url = %self.object_url(key), "object store get");
        // Real implementation would use S3 GetObject API.
        Ok(None)
    }

    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        tracing::debug!(key, url = %self.object_url(key), "object store delete");
        Ok(())
    }

    async fn list(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        tracing::debug!(prefix, "object store list");
        Ok(Vec::new())
    }

    async fn exists(&self, key: &str) -> anyhow::Result<bool> {
        tracing::debug!(key, "object store exists check");
        Ok(false)
    }
}

/// Filesystem-backed object store for testing and development.
pub struct FsObjectStore {
    root: PathBuf,
}

impl FsObjectStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl ObjectStore for FsObjectStore {
    async fn put(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
        let path = self.root.join(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, data).await?;
        Ok(())
    }

    async fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let path = self.root.join(key);
        match tokio::fs::read(&path).await {
            Ok(data) => Ok(Some(data)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        let path = self.root.join(key);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn list(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        let dir = self.root.join(prefix);
        let mut keys = Vec::new();
        if dir.exists() {
            let mut entries = tokio::fs::read_dir(&dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                if let Some(name) = entry.file_name().to_str() {
                    keys.push(format!("{prefix}/{name}"));
                }
            }
        }
        Ok(keys)
    }

    async fn exists(&self, key: &str) -> anyhow::Result<bool> {
        let path = self.root.join(key);
        Ok(path.exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_store_config() {
        let config = ObjectStoreConfig {
            endpoint: "http://minio:9000".to_string(),
            bucket: "sprites".to_string(),
            access_key: "minioadmin".to_string(),
            secret_key: "minioadmin".to_string(),
            region: None,
        };
        let store = S3ObjectStore::new(config);
        assert_eq!(
            store.object_url("chunks/abc123"),
            "http://minio:9000/sprites/chunks/abc123"
        );
    }

    #[tokio::test]
    async fn fs_object_store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsObjectStore::new(dir.path().to_path_buf());

        // Put and get.
        store.put("test/key1", b"hello").await.unwrap();
        let data = store.get("test/key1").await.unwrap();
        assert_eq!(data, Some(b"hello".to_vec()));

        // Exists.
        assert!(store.exists("test/key1").await.unwrap());
        assert!(!store.exists("test/nonexistent").await.unwrap());

        // Delete.
        store.delete("test/key1").await.unwrap();
        assert!(!store.exists("test/key1").await.unwrap());

        // Get non-existent returns None.
        let data = store.get("test/key1").await.unwrap();
        assert!(data.is_none());
    }
}
