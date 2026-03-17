//! Filesystem mount operations for the sprite VM.

use tracing::{debug, warn};

/// Mount essential pseudo-filesystems (proc, sys, dev, tmp).
pub fn mount_essential() -> anyhow::Result<()> {
    let mounts = [
        ("proc", "/proc", "proc", ""),
        ("sysfs", "/sys", "sysfs", ""),
        ("devtmpfs", "/dev", "devtmpfs", ""),
        ("devpts", "/dev/pts", "devpts", ""),
        ("tmpfs", "/tmp", "tmpfs", "size=1G"),
        ("tmpfs", "/run", "tmpfs", "size=256M"),
    ];

    for (source, target, fstype, options) in &mounts {
        // Ensure mount point exists.
        let _ = std::fs::create_dir_all(target);

        debug!(source, target, fstype, "mounting");

        // Use libc::mount for real VM execution.
        // This will fail gracefully outside a real VM context.
        let result = unsafe {
            let src = std::ffi::CString::new(*source)?;
            let tgt = std::ffi::CString::new(*target)?;
            let fst = std::ffi::CString::new(*fstype)?;
            let opts = std::ffi::CString::new(*options)?;

            libc::mount(
                src.as_ptr(),
                tgt.as_ptr(),
                fst.as_ptr(),
                0,
                opts.as_ptr() as *const libc::c_void,
            )
        };

        if result != 0 {
            let err = std::io::Error::last_os_error();
            warn!(target, error = %err, "mount failed (may be expected outside VM)");
        }
    }

    Ok(())
}

/// Mount the workspace directory via virtio-fs.
pub fn mount_workspace() -> anyhow::Result<()> {
    let target = "/workspace";
    let _ = std::fs::create_dir_all(target);

    debug!("mounting workspace via virtio-fs");

    let result = unsafe {
        let src = std::ffi::CString::new("workspace")?;
        let tgt = std::ffi::CString::new(target)?;
        let fst = std::ffi::CString::new("virtiofs")?;

        libc::mount(
            src.as_ptr(),
            tgt.as_ptr(),
            fst.as_ptr(),
            0,
            std::ptr::null(),
        )
    };

    if result != 0 {
        let err = std::io::Error::last_os_error();
        anyhow::bail!("workspace mount failed: {err}");
    }

    Ok(())
}
