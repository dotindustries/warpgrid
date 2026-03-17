//! Checkpoint/restore for sprite filesystems.
//!
//! A checkpoint captures the state of a sprite's filesystem by:
//! 1. Flushing any dirty chunks to object storage
//! 2. Snapshotting the metadata database (file→chunk mappings)
//! 3. Storing the metadata snapshot in object storage
//!
//! Restore loads the metadata snapshot and lets chunks load on-demand
//! through the cache layer.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::object_store::ObjectStore;

/// Content-addressed checkpoint identifier.
pub type CheckpointId = String;

/// Information about a stored checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointInfo {
    pub id: CheckpointId,
    pub sprite_id: String,
    pub created_at: u64,
    /// Size of the metadata snapshot in bytes.
    pub metadata_size_bytes: u64,
    /// Number of data chunks referenced.
    pub chunk_count: u64,
}

/// Manages checkpoint/restore operations for sprite filesystems.
pub struct CheckpointManager<S: ObjectStore> {
    store: S,
    /// Object store prefix for checkpoint data.
    prefix: String,
}

impl<S: ObjectStore> CheckpointManager<S> {
    pub fn new(store: S, prefix: String) -> Self {
        Self { store, prefix }
    }

    /// Create a checkpoint for a sprite.
    ///
    /// In a full implementation, this would:
    /// 1. Signal JuiceFS to flush dirty chunks
    /// 2. Snapshot the SQLite metadata DB
    /// 3. Upload the metadata snapshot to object storage
    /// 4. Return a content-addressed checkpoint ID
    pub async fn checkpoint(&self, sprite_id: &str) -> anyhow::Result<CheckpointInfo> {
        let now = epoch_secs();
        let checkpoint_id = format!("{sprite_id}-{now}");
        let key = format!("{}/checkpoints/{checkpoint_id}/metadata.db", self.prefix);

        // Placeholder: in reality we'd read and upload the JuiceFS metadata DB.
        let metadata = serde_json::to_vec(&serde_json::json!({
            "sprite_id": sprite_id,
            "created_at": now,
            "version": 1,
        }))?;

        self.store.put(&key, &metadata).await?;

        let info = CheckpointInfo {
            id: checkpoint_id.clone(),
            sprite_id: sprite_id.to_string(),
            created_at: now,
            metadata_size_bytes: metadata.len() as u64,
            chunk_count: 0,
        };

        // Store checkpoint index entry.
        let index_key = format!(
            "{}/checkpoints/{checkpoint_id}/info.json",
            self.prefix
        );
        let index_data = serde_json::to_vec(&info)?;
        self.store.put(&index_key, &index_data).await?;

        info!(
            sprite_id,
            checkpoint_id,
            "checkpoint created"
        );

        Ok(info)
    }

    /// Restore a sprite from a checkpoint.
    ///
    /// Downloads the metadata snapshot; chunks load on-demand through the cache.
    pub async fn restore(&self, checkpoint_id: &str) -> anyhow::Result<CheckpointInfo> {
        let index_key = format!(
            "{}/checkpoints/{checkpoint_id}/info.json",
            self.prefix
        );

        let data = self
            .store
            .get(&index_key)
            .await?
            .ok_or_else(|| anyhow::anyhow!("checkpoint not found: {checkpoint_id}"))?;

        let info: CheckpointInfo = serde_json::from_slice(&data)?;

        info!(
            checkpoint_id,
            sprite_id = %info.sprite_id,
            "checkpoint restored"
        );

        Ok(info)
    }

    /// List all checkpoints for a sprite.
    pub async fn list_checkpoints(&self, sprite_id: &str) -> anyhow::Result<Vec<CheckpointInfo>> {
        let prefix = format!("{}/checkpoints/{sprite_id}-", self.prefix);
        let keys = self.store.list(&prefix).await?;

        let mut checkpoints = Vec::new();
        for key in keys {
            if key.ends_with("/info.json") {
                if let Some(data) = self.store.get(&key).await? {
                    if let Ok(info) = serde_json::from_slice::<CheckpointInfo>(&data) {
                        checkpoints.push(info);
                    }
                }
            }
        }

        checkpoints.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(checkpoints)
    }

    /// Delete a checkpoint and its data.
    pub async fn delete_checkpoint(&self, checkpoint_id: &str) -> anyhow::Result<()> {
        let metadata_key = format!(
            "{}/checkpoints/{checkpoint_id}/metadata.db",
            self.prefix
        );
        let index_key = format!(
            "{}/checkpoints/{checkpoint_id}/info.json",
            self.prefix
        );

        self.store.delete(&metadata_key).await?;
        self.store.delete(&index_key).await?;

        info!(checkpoint_id, "checkpoint deleted");
        Ok(())
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object_store::FsObjectStore;

    #[tokio::test]
    async fn checkpoint_and_restore() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsObjectStore::new(dir.path().to_path_buf());
        let mgr = CheckpointManager::new(store, "test".to_string());

        let info = mgr.checkpoint("sprite-100").await.unwrap();
        assert!(info.id.starts_with("sprite-100-"));
        assert_eq!(info.sprite_id, "sprite-100");

        let restored = mgr.restore(&info.id).await.unwrap();
        assert_eq!(restored.id, info.id);
        assert_eq!(restored.sprite_id, "sprite-100");
    }

    #[tokio::test]
    async fn delete_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsObjectStore::new(dir.path().to_path_buf());
        let mgr = CheckpointManager::new(store, "test".to_string());

        let info = mgr.checkpoint("sprite-100").await.unwrap();
        mgr.delete_checkpoint(&info.id).await.unwrap();

        // Restore should fail now.
        assert!(mgr.restore(&info.id).await.is_err());
    }
}
