//! Warm VM pool for instant sprite creation.
//!
//! Maintains a pool of pre-booted, unassigned VMs. When a sprite create
//! request arrives, we pop from the warm pool (near-instant) rather than
//! booting from scratch (~1-2s).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::error::{SpriteError, SpriteResult};
use crate::hypervisor::{Hypervisor, VmConfig, VmHandle};

/// Configuration for the warm pool.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Minimum number of warm (booted but unassigned) sprites to keep ready.
    pub min_warm: usize,
    /// Maximum total sprites (warm + active) per node.
    pub max_total: usize,
    /// Timeout for booting a new VM.
    pub boot_timeout: Duration,
    /// Default vCPUs per sprite.
    pub default_vcpus: u32,
    /// Default memory (MB) per sprite.
    pub default_memory_mb: u32,
    /// Path to the shared guest kernel.
    pub kernel_path: PathBuf,
    /// Path to the golden root filesystem.
    pub rootfs_path: PathBuf,
    /// Base directory for VM overlays and runtime state.
    pub runtime_dir: PathBuf,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            min_warm: 3,
            max_total: 50,
            boot_timeout: Duration::from_secs(30),
            default_vcpus: 2,
            default_memory_mb: 4096,
            kernel_path: PathBuf::from("/var/lib/warpgrid/vmlinux"),
            rootfs_path: PathBuf::from("/var/lib/warpgrid/golden.ext4"),
            runtime_dir: PathBuf::from("/var/lib/warpgrid/sprites"),
        }
    }
}

/// A warm (booted, idle) sprite ready for assignment.
#[derive(Debug)]
pub struct WarmSprite {
    pub handle: VmHandle,
    pub config: VmConfig,
}

/// Pool of pre-booted VMs for instant sprite creation.
pub struct SpritePool<H: Hypervisor> {
    warm: Arc<Mutex<VecDeque<WarmSprite>>>,
    active_count: Arc<Mutex<usize>>,
    config: PoolConfig,
    hypervisor: Arc<H>,
    next_cid: Arc<Mutex<u32>>,
}

impl<H: Hypervisor + 'static> SpritePool<H> {
    /// Create a new sprite pool.
    pub fn new(hypervisor: Arc<H>, config: PoolConfig) -> Self {
        Self {
            warm: Arc::new(Mutex::new(VecDeque::new())),
            active_count: Arc::new(Mutex::new(0)),
            config,
            hypervisor,
            next_cid: Arc::new(Mutex::new(100)), // CIDs start at 100 (0-2 are reserved).
        }
    }

    /// Allocate the next vsock CID.
    async fn alloc_cid(&self) -> u32 {
        let mut cid = self.next_cid.lock().await;
        let val = *cid;
        *cid += 1;
        val
    }

    /// Total number of VMs managed (warm + active).
    pub async fn total_count(&self) -> usize {
        let warm = self.warm.lock().await.len();
        let active = *self.active_count.lock().await;
        warm + active
    }

    /// Number of warm (idle) VMs available.
    pub async fn warm_count(&self) -> usize {
        self.warm.lock().await.len()
    }

    /// Boot a new VM and add it to the warm pool.
    async fn boot_warm_vm(&self) -> SpriteResult<()> {
        if self.total_count().await >= self.config.max_total {
            return Err(SpriteError::PoolExhausted);
        }

        let cid = self.alloc_cid().await;
        let overlay = self.config.runtime_dir.join(format!("overlay-{cid}.qcow2"));

        let vm_config = VmConfig {
            vcpus: self.config.default_vcpus,
            memory_mb: self.config.default_memory_mb,
            kernel: self.config.kernel_path.clone(),
            rootfs: self.config.rootfs_path.clone(),
            overlay,
            vsock_cid: cid,
            virtio_fs: None,
        };

        let handle = self.hypervisor.create_vm(vm_config.clone()).await?;
        self.hypervisor.start_vm(&handle).await?;

        info!(vm_id = %handle.id, cid, "warm VM booted");

        self.warm.lock().await.push_back(WarmSprite {
            handle,
            config: vm_config,
        });

        Ok(())
    }

    /// Acquire a warm VM from the pool. If none available and capacity allows, boot one.
    pub async fn acquire(&self) -> SpriteResult<WarmSprite> {
        // Try to pop from the warm pool first.
        if let Some(sprite) = self.warm.lock().await.pop_front() {
            *self.active_count.lock().await += 1;
            info!(vm_id = %sprite.handle.id, "sprite acquired from warm pool");
            return Ok(sprite);
        }

        // No warm VMs — boot one on demand if capacity allows.
        if self.total_count().await >= self.config.max_total {
            return Err(SpriteError::PoolExhausted);
        }

        let cid = self.alloc_cid().await;
        let overlay = self.config.runtime_dir.join(format!("overlay-{cid}.qcow2"));

        let vm_config = VmConfig {
            vcpus: self.config.default_vcpus,
            memory_mb: self.config.default_memory_mb,
            kernel: self.config.kernel_path.clone(),
            rootfs: self.config.rootfs_path.clone(),
            overlay,
            vsock_cid: cid,
            virtio_fs: None,
        };

        let handle = self.hypervisor.create_vm(vm_config.clone()).await?;
        self.hypervisor.start_vm(&handle).await?;

        *self.active_count.lock().await += 1;
        info!(vm_id = %handle.id, cid, "sprite booted on demand");

        Ok(WarmSprite {
            handle,
            config: vm_config,
        })
    }

    /// Release a sprite back (decrements active count). The VM should already
    /// be destroyed by the caller.
    pub async fn release(&self) {
        let mut count = self.active_count.lock().await;
        *count = count.saturating_sub(1);
    }

    /// Replenish the warm pool up to `min_warm` if below target.
    pub async fn replenish(&self) {
        let warm_count = self.warm.lock().await.len();
        let deficit = self.config.min_warm.saturating_sub(warm_count);

        for _ in 0..deficit {
            if let Err(e) = self.boot_warm_vm().await {
                warn!(error = %e, "failed to replenish warm pool");
                break;
            }
        }
    }

    /// Run the pool replenishment loop. Call this as a background task.
    pub async fn run_replenish_loop(&self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        let interval = Duration::from_secs(5);
        loop {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {
                    self.replenish().await;
                }
                _ = shutdown.changed() => {
                    info!("sprite pool replenish loop shutting down");
                    break;
                }
            }
        }
    }

    /// Drain all warm VMs (for shutdown).
    pub async fn drain(&self) {
        let mut warm = self.warm.lock().await;
        while let Some(sprite) = warm.pop_front() {
            if let Err(e) = self.hypervisor.destroy_vm(&sprite.handle).await {
                warn!(vm_id = %sprite.handle.id, error = %e, "failed to destroy warm VM");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_config_defaults() {
        let config = PoolConfig::default();
        assert_eq!(config.min_warm, 3);
        assert_eq!(config.max_total, 50);
        assert_eq!(config.default_vcpus, 2);
        assert_eq!(config.default_memory_mb, 4096);
    }
}
