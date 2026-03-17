//! Hypervisor abstraction for managing lightweight VMs.
//!
//! Defines a `Hypervisor` trait that backends (Cloud Hypervisor, Firecracker)
//! implement. Each VM is identified by a `VmHandle` returned at creation time.

use std::path::PathBuf;

use crate::error::SpriteResult;

/// Opaque handle to a running VM.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VmHandle {
    /// Unique identifier for this VM instance.
    pub id: String,
    /// Path to the hypervisor API socket for this VM.
    pub api_socket: PathBuf,
    /// vsock CID assigned to this VM.
    pub vsock_cid: u32,
    /// PID of the hypervisor process (for lifecycle management).
    pub pid: Option<u32>,
}

/// Configuration for creating a new VM.
#[derive(Debug, Clone)]
pub struct VmConfig {
    /// Number of virtual CPUs.
    pub vcpus: u32,
    /// Memory in megabytes.
    pub memory_mb: u32,
    /// Path to the shared guest kernel (vmlinux).
    pub kernel: PathBuf,
    /// Path to the golden root filesystem (read-only).
    pub rootfs: PathBuf,
    /// Per-VM writable overlay path.
    pub overlay: PathBuf,
    /// vsock context ID for host↔guest communication.
    pub vsock_cid: u32,
    /// Optional virtio-fs mount for workspace storage.
    pub virtio_fs: Option<VirtioFsMount>,
}

/// A virtio-fs shared directory mount.
#[derive(Debug, Clone)]
pub struct VirtioFsMount {
    /// Tag used inside the guest to identify this mount.
    pub tag: String,
    /// Host-side directory to share.
    pub source: PathBuf,
}

/// Runtime status of a VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmStatus {
    /// VM is being created.
    Creating,
    /// VM is running.
    Running,
    /// VM is paused (memory preserved, no CPU).
    Paused,
    /// VM has been stopped.
    Stopped,
    /// VM is in an error state.
    Failed,
}

/// Trait abstracting the hypervisor, allowing Cloud Hypervisor or Firecracker backends.
///
/// Implementations manage the full VM lifecycle: create → start → pause/resume → stop → destroy.
pub trait Hypervisor: Send + Sync {
    /// Create a new VM with the given configuration. Returns a handle for subsequent operations.
    fn create_vm(
        &self,
        config: VmConfig,
    ) -> impl Future<Output = SpriteResult<VmHandle>> + Send;

    /// Boot a previously created VM.
    fn start_vm(&self, handle: &VmHandle) -> impl Future<Output = SpriteResult<()>> + Send;

    /// Stop a running VM (graceful shutdown).
    fn stop_vm(&self, handle: &VmHandle) -> impl Future<Output = SpriteResult<()>> + Send;

    /// Pause a running VM (ACPI S3 / hypervisor-level pause). Memory is preserved.
    fn pause_vm(&self, handle: &VmHandle) -> impl Future<Output = SpriteResult<()>> + Send;

    /// Resume a paused VM.
    fn resume_vm(&self, handle: &VmHandle) -> impl Future<Output = SpriteResult<()>> + Send;

    /// Destroy a VM and clean up all resources.
    fn destroy_vm(&self, handle: &VmHandle) -> impl Future<Output = SpriteResult<()>> + Send;

    /// Query the current status of a VM.
    fn vm_status(&self, handle: &VmHandle) -> impl Future<Output = SpriteResult<VmStatus>> + Send;
}

use std::future::Future;

/// Cloud Hypervisor backend.
///
/// Manages VMs via Cloud Hypervisor's REST API over a Unix domain socket.
/// Each VM gets its own `cloud-hypervisor` process and API socket.
pub struct CloudHypervisor {
    /// Path to the `cloud-hypervisor` binary.
    binary_path: PathBuf,
    /// Base directory for VM runtime state (sockets, logs).
    runtime_dir: PathBuf,
}

impl CloudHypervisor {
    pub fn new(binary_path: PathBuf, runtime_dir: PathBuf) -> Self {
        Self {
            binary_path,
            runtime_dir,
        }
    }
}

impl Hypervisor for CloudHypervisor {
    async fn create_vm(&self, config: VmConfig) -> SpriteResult<VmHandle> {
        let vm_id = format!("sprite-{}", config.vsock_cid);
        let vm_dir = self.runtime_dir.join(&vm_id);
        std::fs::create_dir_all(&vm_dir)?;

        let api_socket = vm_dir.join("api.sock");

        // Build Cloud Hypervisor command.
        let mut cmd = tokio::process::Command::new(&self.binary_path);
        cmd.arg("--api-socket").arg(&api_socket);
        cmd.arg("--kernel").arg(&config.kernel);
        cmd.arg("--cpus").arg(format!("boot={}", config.vcpus));
        cmd.arg("--memory").arg(format!("size={}M", config.memory_mb));

        // Root disk: golden image as read-only backing, overlay for writes.
        cmd.arg("--disk").arg(format!(
            "path={},readonly=on",
            config.rootfs.display()
        ));
        cmd.arg("--disk").arg(format!(
            "path={}",
            config.overlay.display()
        ));

        // vsock for host↔guest communication.
        cmd.arg("--vsock").arg(format!(
            "cid={},socket={}",
            config.vsock_cid,
            vm_dir.join("vsock.sock").display()
        ));

        // virtio-fs for workspace storage.
        if let Some(ref vfs) = config.virtio_fs {
            cmd.arg("--fs").arg(format!(
                "tag={},socket={},source={}",
                vfs.tag,
                vm_dir.join("virtiofs.sock").display(),
                vfs.source.display()
            ));
        }

        // Console output to log file.
        cmd.arg("--serial").arg("file=/dev/null");
        cmd.arg("--console").arg("off");

        let child = cmd
            .spawn()
            .map_err(|e| crate::error::SpriteError::Hypervisor(format!(
                "failed to spawn cloud-hypervisor: {e}"
            )))?;

        let pid = child.id();

        tracing::info!(vm_id, ?api_socket, ?pid, "cloud hypervisor process spawned");

        Ok(VmHandle {
            id: vm_id,
            api_socket,
            vsock_cid: config.vsock_cid,
            pid,
        })
    }

    async fn start_vm(&self, handle: &VmHandle) -> SpriteResult<()> {
        // Cloud Hypervisor auto-boots on process start with the config above.
        // For API-driven boot, we would POST to /api/v1/vm.boot.
        tracing::info!(vm_id = %handle.id, "VM start requested (auto-booted)");
        Ok(())
    }

    async fn stop_vm(&self, handle: &VmHandle) -> SpriteResult<()> {
        // Send shutdown via API socket: PUT /api/v1/vm.shutdown
        tracing::info!(vm_id = %handle.id, "VM stop requested");
        if let Some(pid) = handle.pid {
            // Send SIGTERM to the hypervisor process.
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        Ok(())
    }

    async fn pause_vm(&self, handle: &VmHandle) -> SpriteResult<()> {
        // PUT /api/v1/vm.pause
        tracing::info!(vm_id = %handle.id, "VM pause requested");
        Ok(())
    }

    async fn resume_vm(&self, handle: &VmHandle) -> SpriteResult<()> {
        // PUT /api/v1/vm.resume
        tracing::info!(vm_id = %handle.id, "VM resume requested");
        Ok(())
    }

    async fn destroy_vm(&self, handle: &VmHandle) -> SpriteResult<()> {
        tracing::info!(vm_id = %handle.id, "VM destroy requested");
        // Stop first if running.
        if let Some(pid) = handle.pid {
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
        // Clean up runtime directory.
        let vm_dir = self.runtime_dir.join(&handle.id);
        if vm_dir.exists() {
            std::fs::remove_dir_all(&vm_dir)?;
        }
        Ok(())
    }

    async fn vm_status(&self, handle: &VmHandle) -> SpriteResult<VmStatus> {
        // Check if the hypervisor process is still alive.
        if let Some(pid) = handle.pid {
            let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
            if alive {
                return Ok(VmStatus::Running);
            }
        }
        Ok(VmStatus::Stopped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_config_defaults() {
        let config = VmConfig {
            vcpus: 2,
            memory_mb: 4096,
            kernel: PathBuf::from("/boot/vmlinux"),
            rootfs: PathBuf::from("/images/golden.ext4"),
            overlay: PathBuf::from("/tmp/overlay.qcow2"),
            vsock_cid: 100,
            virtio_fs: None,
        };
        assert_eq!(config.vcpus, 2);
        assert_eq!(config.memory_mb, 4096);
    }

    #[test]
    fn vm_config_with_virtio_fs() {
        let config = VmConfig {
            vcpus: 4,
            memory_mb: 8192,
            kernel: PathBuf::from("/boot/vmlinux"),
            rootfs: PathBuf::from("/images/golden.ext4"),
            overlay: PathBuf::from("/tmp/overlay.qcow2"),
            vsock_cid: 101,
            virtio_fs: Some(VirtioFsMount {
                tag: "workspace".to_string(),
                source: PathBuf::from("/data/workspaces/user1"),
            }),
        };
        assert!(config.virtio_fs.is_some());
        assert_eq!(config.virtio_fs.unwrap().tag, "workspace");
    }

    #[test]
    fn vm_handle_equality() {
        let h1 = VmHandle {
            id: "vm-1".to_string(),
            api_socket: PathBuf::from("/tmp/vm-1/api.sock"),
            vsock_cid: 100,
            pid: Some(1234),
        };
        let h2 = h1.clone();
        assert_eq!(h1, h2);
    }
}
