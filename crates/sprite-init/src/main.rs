//! sprite-init — PID 1 supervisor for WarpGrid sprite VMs.
//!
//! Runs as the init process inside a sprite VM's root namespace.
//! Responsibilities:
//! 1. Mount filesystems (proc, sys, dev, workspace via virtio-fs)
//! 2. Set up inner namespace for user workload (Claude Code)
//! 3. Communicate with host via vsock control channel
//! 4. Monitor activity for auto-sleep decisions
//! 5. Forward logs to host
//! 6. Detect bound ports for service proxy registration
//! 7. Handle checkpoint signals

mod container;
mod mounts;
mod vsock_guest;

use std::time::{Duration, Instant};

use tracing::{error, info, warn};

/// Default vsock port for the control channel.
const CONTROL_PORT: u32 = 5000;

/// Default inactivity timeout before signaling the host.
const IDLE_TIMEOUT: Duration = Duration::from_secs(600);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,sprite_init=debug")
        .init();

    info!("sprite-init starting as PID 1");

    // Mount essential filesystems.
    if let Err(e) = mounts::mount_essential() {
        warn!(error = %e, "failed to mount some filesystems (may be running outside VM)");
    }

    // Mount workspace via virtio-fs if available.
    if let Err(e) = mounts::mount_workspace() {
        warn!(error = %e, "workspace mount not available");
    }

    // Set up vsock listener for host communication.
    let vsock = vsock_guest::VsockListener::new(CONTROL_PORT);
    info!(port = CONTROL_PORT, "vsock control channel ready");

    // Notify host we're ready.
    vsock.send_ready().await;

    // Start the inner container with user workload.
    let container_config = container::ContainerConfig::from_env();
    let _child = container::spawn_inner(&container_config).await?;

    // Main event loop: handle vsock messages and track activity.
    let mut last_activity = Instant::now();

    loop {
        tokio::select! {
            msg = vsock.recv() => {
                match msg {
                    Ok(Some(message)) => {
                        last_activity = Instant::now();
                        handle_message(&vsock, message).await;
                    }
                    Ok(None) => {
                        // Connection closed, reconnect.
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    Err(e) => {
                        warn!(error = %e, "vsock recv error");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                // Periodic activity check.
                let idle_duration = last_activity.elapsed();
                if idle_duration >= IDLE_TIMEOUT {
                    info!(idle_secs = idle_duration.as_secs(), "idle timeout reached, notifying host");
                    vsock.send_activity_timeout().await;
                } else {
                    // Send periodic activity ping.
                    vsock.send_activity_ping().await;
                }
            }
        }
    }
}

/// Handle an incoming message from the host.
async fn handle_message(vsock: &vsock_guest::VsockListener, message: vsock_guest::HostMessage) {
    match message {
        vsock_guest::HostMessage::Checkpoint => {
            info!("checkpoint requested, flushing buffers");
            // Sync filesystems.
            unsafe { libc::sync(); }
            vsock.send_checkpoint_ready().await;
        }
        vsock_guest::HostMessage::Sleep => {
            info!("sleep requested, preparing for suspension");
            // Sync and prepare for pause.
            unsafe { libc::sync(); }
        }
        vsock_guest::HostMessage::Wake => {
            info!("woken from sleep");
        }
        vsock_guest::HostMessage::Exec { command, env } => {
            info!(command, "exec requested");
            let result = container::exec_command(&command, &env).await;
            match result {
                Ok((exit_code, stdout, stderr)) => {
                    vsock.send_exec_result(exit_code, stdout, stderr).await;
                }
                Err(e) => {
                    error!(error = %e, "exec failed");
                    vsock
                        .send_exec_result(-1, String::new(), e.to_string())
                        .await;
                }
            }
        }
        vsock_guest::HostMessage::InjectEnv { env } => {
            info!(count = env.len(), "injecting environment variables");
            for (key, value) in env {
                // SAFETY: sprite-init is single-threaded at this point during env setup.
                unsafe { std::env::set_var(&key, &value); }
            }
        }
    }
}
