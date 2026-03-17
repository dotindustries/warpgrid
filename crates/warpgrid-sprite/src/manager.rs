//! Top-level sprite lifecycle manager.
//!
//! Coordinates the hypervisor, warm pool, vsock communication, and state store
//! to manage the full lifecycle of sprite VMs.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::error::{SpriteError, SpriteResult};
use crate::hypervisor::{Hypervisor, VmHandle};
use crate::pool::{PoolConfig, SpritePool};
use crate::vsock::VsockStream;
use warpgrid_state::{SpriteResources, SpriteSpec, SpriteStatus, StateStore};

/// Configuration for the sprite manager.
#[derive(Debug, Clone)]
pub struct SpriteManagerConfig {
    /// Pool configuration.
    pub pool: PoolConfig,
    /// Duration of inactivity before light sleep (VM pause).
    pub idle_pause_after: Duration,
    /// Duration of inactivity before deep sleep (checkpoint + destroy).
    pub idle_sleep_after: Duration,
    /// This node's ID (for placement tracking).
    pub node_id: String,
}

impl Default for SpriteManagerConfig {
    fn default() -> Self {
        Self {
            pool: PoolConfig::default(),
            idle_pause_after: Duration::from_secs(600),  // 10 minutes
            idle_sleep_after: Duration::from_secs(3600), // 1 hour
            node_id: "standalone".to_string(),
        }
    }
}

/// Tracks a running sprite and its associated resources.
struct ActiveSprite {
    handle: VmHandle,
    vsock: VsockStream,
    spec: SpriteSpec,
}

/// Manages the lifecycle of sprite VMs on a single node.
pub struct SpriteManager<H: Hypervisor> {
    pool: Arc<SpritePool<H>>,
    hypervisor: Arc<H>,
    state: StateStore,
    config: SpriteManagerConfig,
    active: Arc<RwLock<HashMap<String, ActiveSprite>>>,
}

impl<H: Hypervisor + 'static> SpriteManager<H> {
    /// Create a new sprite manager.
    pub fn new(hypervisor: Arc<H>, state: StateStore, config: SpriteManagerConfig) -> Self {
        let pool = Arc::new(SpritePool::new(hypervisor.clone(), config.pool.clone()));
        Self {
            pool,
            hypervisor,
            state,
            config,
            active: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get a reference to the warm pool.
    pub fn pool(&self) -> &Arc<SpritePool<H>> {
        &self.pool
    }

    /// Create a new sprite. Acquires a warm VM, sets up storage, and registers in state store.
    pub async fn create_sprite(
        &self,
        owner: String,
        name: String,
        resources: SpriteResources,
        env: HashMap<String, String>,
    ) -> SpriteResult<SpriteSpec> {
        // Acquire a VM from the warm pool.
        let warm = self.pool.acquire().await?;

        let now = epoch_secs();
        let sprite_id = format!("sprite-{}", warm.handle.vsock_cid);

        let spec = SpriteSpec {
            id: sprite_id.clone(),
            owner,
            name,
            image_version: "latest".to_string(),
            resources,
            storage_url: format!("s3://warpgrid-sprites/{sprite_id}"),
            checkpoint_id: None,
            status: SpriteStatus::Running,
            node_id: Some(self.config.node_id.clone()),
            created_at: now,
            last_active_at: now,
        };

        // Persist to state store.
        self.state
            .put_sprite(&spec)
            .map_err(|e| SpriteError::Other(e.to_string()))?;

        // Set up vsock communication.
        let vsock = VsockStream::new(warm.handle.vsock_cid, 5000);

        // Inject environment variables.
        if !env.is_empty() {
            let env_pairs: Vec<(String, String)> = env.into_iter().collect();
            let _ = vsock
                .send(&crate::vsock::SpriteMessage::InjectEnv { env: env_pairs })
                .await;
        }

        // Track as active.
        self.active.write().await.insert(
            sprite_id.clone(),
            ActiveSprite {
                handle: warm.handle,
                vsock,
                spec: spec.clone(),
            },
        );

        info!(sprite_id, "sprite created");
        Ok(spec)
    }

    /// Destroy a sprite, freeing all resources.
    pub async fn destroy_sprite(&self, sprite_id: &str) -> SpriteResult<()> {
        let sprite = self
            .active
            .write()
            .await
            .remove(sprite_id)
            .ok_or_else(|| SpriteError::VmNotFound(sprite_id.to_string()))?;

        // Destroy the VM.
        self.hypervisor.destroy_vm(&sprite.handle).await?;
        self.pool.release().await;

        // Remove from state store.
        let _ = self.state.delete_sprite(sprite_id);

        info!(sprite_id, "sprite destroyed");
        Ok(())
    }

    /// Pause a running sprite (light sleep — memory preserved).
    pub async fn pause_sprite(&self, sprite_id: &str) -> SpriteResult<()> {
        let active = self.active.read().await;
        let sprite = active
            .get(sprite_id)
            .ok_or_else(|| SpriteError::VmNotFound(sprite_id.to_string()))?;

        // Notify guest of impending sleep.
        let _ = sprite.vsock.send(&crate::vsock::SpriteMessage::Sleep).await;

        // Pause the VM.
        self.hypervisor.pause_vm(&sprite.handle).await?;

        // Update state.
        let mut spec = sprite.spec.clone();
        spec.status = SpriteStatus::Paused;
        let _ = self.state.put_sprite(&spec);

        info!(sprite_id, "sprite paused");
        Ok(())
    }

    /// Resume a paused sprite.
    pub async fn wake_sprite(&self, sprite_id: &str) -> SpriteResult<()> {
        let active = self.active.read().await;
        let sprite = active
            .get(sprite_id)
            .ok_or_else(|| SpriteError::VmNotFound(sprite_id.to_string()))?;

        // Resume the VM.
        self.hypervisor.resume_vm(&sprite.handle).await?;

        // Notify guest of wake.
        let _ = sprite.vsock.send(&crate::vsock::SpriteMessage::Wake).await;

        // Update state.
        let mut spec = sprite.spec.clone();
        spec.status = SpriteStatus::Running;
        spec.last_active_at = epoch_secs();
        let _ = self.state.put_sprite(&spec);

        info!(sprite_id, "sprite woken");
        Ok(())
    }

    /// Execute a command inside a sprite's inner namespace.
    pub async fn exec_in_sprite(
        &self,
        sprite_id: &str,
        command: String,
        env: Vec<(String, String)>,
    ) -> SpriteResult<()> {
        let active = self.active.read().await;
        let sprite = active
            .get(sprite_id)
            .ok_or_else(|| SpriteError::VmNotFound(sprite_id.to_string()))?;

        sprite
            .vsock
            .send(&crate::vsock::SpriteMessage::Exec { command, env })
            .await
            .map_err(|e| SpriteError::Vsock(e.to_string()))?;

        Ok(())
    }

    /// List all active sprites on this node.
    pub async fn list_active(&self) -> Vec<SpriteSpec> {
        self.active
            .read()
            .await
            .values()
            .map(|s| s.spec.clone())
            .collect()
    }

    /// Shutdown the manager, draining all sprites.
    pub async fn shutdown(&self) {
        let sprite_ids: Vec<String> = self.active.read().await.keys().cloned().collect();
        for id in sprite_ids {
            if let Err(e) = self.destroy_sprite(&id).await {
                warn!(sprite_id = %id, error = %e, "failed to destroy sprite during shutdown");
            }
        }
        self.pool.drain().await;
        info!("sprite manager shut down");
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
