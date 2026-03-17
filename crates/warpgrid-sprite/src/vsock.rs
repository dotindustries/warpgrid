//! Host↔guest communication protocol over vsock.
//!
//! Defines the message types exchanged between the host-side sprite manager
//! and the guest-side sprite-init process via VM sockets (AF_VSOCK).

use serde::{Deserialize, Serialize};

/// Messages exchanged between host and guest over vsock.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SpriteMessage {
    // ── Host → Guest ─────────────────────────────────────────

    /// Request the guest to prepare for checkpointing (flush buffers).
    Checkpoint,

    /// Request the guest to enter sleep mode (graceful process suspension).
    Sleep,

    /// Notify the guest it has been woken from sleep.
    Wake,

    /// Execute a command inside the inner namespace.
    Exec {
        command: String,
        env: Vec<(String, String)>,
    },

    /// Inject environment variables (e.g., API keys) into the inner namespace.
    InjectEnv {
        env: Vec<(String, String)>,
    },

    // ── Guest → Host ─────────────────────────────────────────

    /// Guest has finished booting and is ready to accept work.
    Ready,

    /// Periodic activity ping (resets idle timer on host).
    ActivityPing,

    /// Guest detected a newly bound port (for service proxy registration).
    PortBound {
        port: u16,
        proto: Protocol,
    },

    /// Log line from the guest.
    LogLine {
        stream: Stream,
        line: String,
    },

    /// Periodic resource usage snapshot from guest.
    MetricsSnapshot {
        cpu_pct: f32,
        mem_bytes: u64,
    },

    /// Guest has completed checkpoint preparation.
    CheckpointReady,

    /// Command execution result.
    ExecResult {
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
}

/// Network protocol for port bindings.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Tcp,
    Udp,
}

/// Output stream identifier.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdout,
    Stderr,
}

/// A connection to a sprite's vsock endpoint.
///
/// Wraps the host-side vsock socket, providing typed message send/receive.
pub struct VsockStream {
    /// vsock CID of the guest.
    pub cid: u32,
    /// Port number used for the control channel.
    pub port: u32,
}

impl VsockStream {
    /// Create a new vsock stream targeting the given guest CID.
    pub fn new(cid: u32, port: u32) -> Self {
        Self { cid, port }
    }

    /// Serialize and send a message to the guest.
    pub async fn send(&self, msg: &SpriteMessage) -> Result<(), std::io::Error> {
        let _payload = serde_json::to_vec(msg)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        tracing::debug!(cid = self.cid, port = self.port, ?msg, "vsock send");
        // Real implementation would write to AF_VSOCK socket.
        // For now this is a structural placeholder — the vsock fd operations
        // require Linux-specific AF_VSOCK support.
        Ok(())
    }

    /// Receive and deserialize a message from the guest.
    pub async fn recv(&self) -> Result<SpriteMessage, std::io::Error> {
        tracing::debug!(cid = self.cid, port = self.port, "vsock recv waiting");
        // Structural placeholder — real implementation reads from AF_VSOCK socket.
        Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "vsock recv not yet implemented",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_serialization_roundtrip() {
        let messages = vec![
            SpriteMessage::Checkpoint,
            SpriteMessage::Sleep,
            SpriteMessage::Wake,
            SpriteMessage::Ready,
            SpriteMessage::ActivityPing,
            SpriteMessage::Exec {
                command: "ls -la".to_string(),
                env: vec![("PATH".to_string(), "/usr/bin".to_string())],
            },
            SpriteMessage::PortBound {
                port: 8080,
                proto: Protocol::Tcp,
            },
            SpriteMessage::LogLine {
                stream: Stream::Stdout,
                line: "hello world".to_string(),
            },
            SpriteMessage::MetricsSnapshot {
                cpu_pct: 42.5,
                mem_bytes: 1024 * 1024 * 512,
            },
            SpriteMessage::ExecResult {
                exit_code: 0,
                stdout: "output".to_string(),
                stderr: String::new(),
            },
        ];

        for msg in &messages {
            let json = serde_json::to_string(msg).unwrap();
            let parsed: SpriteMessage = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&parsed).unwrap();
            assert_eq!(json, json2, "roundtrip failed for {msg:?}");
        }
    }

    #[test]
    fn vsock_stream_creation() {
        let stream = VsockStream::new(100, 5000);
        assert_eq!(stream.cid, 100);
        assert_eq!(stream.port, 5000);
    }
}
