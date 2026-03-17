//! Guest-side vsock communication with the host.
//!
//! Listens on the vsock control port for messages from the host-side
//! sprite manager and sends status updates back.

use serde::{Deserialize, Serialize};
use tracing::debug;

/// Messages the host can send to the guest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMessage {
    Checkpoint,
    Sleep,
    Wake,
    Exec {
        command: String,
        env: Vec<(String, String)>,
    },
    InjectEnv {
        env: Vec<(String, String)>,
    },
}

/// Messages the guest sends to the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GuestMessage {
    Ready,
    ActivityPing,
    ActivityTimeout,
    CheckpointReady,
    PortBound { port: u16, proto: String },
    LogLine { stream: String, line: String },
    MetricsSnapshot { cpu_pct: f32, mem_bytes: u64 },
    ExecResult { exit_code: i32, stdout: String, stderr: String },
}

/// vsock listener for the guest side.
pub struct VsockListener {
    port: u32,
}

impl VsockListener {
    pub fn new(port: u32) -> Self {
        Self { port }
    }

    /// Receive a message from the host (blocking until available).
    pub async fn recv(&self) -> Result<Option<HostMessage>, std::io::Error> {
        // Real implementation would accept connections on AF_VSOCK.
        // For structural purposes, this sleeps and returns None.
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        Ok(None)
    }

    /// Send a message to the host.
    async fn send(&self, msg: &GuestMessage) {
        debug!(port = self.port, ?msg, "sending to host");
        // Real implementation would write to the AF_VSOCK connection.
    }

    pub async fn send_ready(&self) {
        self.send(&GuestMessage::Ready).await;
    }

    pub async fn send_activity_ping(&self) {
        self.send(&GuestMessage::ActivityPing).await;
    }

    pub async fn send_activity_timeout(&self) {
        self.send(&GuestMessage::ActivityTimeout).await;
    }

    pub async fn send_checkpoint_ready(&self) {
        self.send(&GuestMessage::CheckpointReady).await;
    }

    pub async fn send_exec_result(&self, exit_code: i32, stdout: String, stderr: String) {
        self.send(&GuestMessage::ExecResult {
            exit_code,
            stdout,
            stderr,
        })
        .await;
    }
}
