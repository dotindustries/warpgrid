//! Node agent — client-side cluster participation.
//!
//! The agent runs on each worker node and connects to the control
//! plane's `ClusterService` to join, send heartbeats, and receive
//! commands.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::watch;
use tonic::transport::Channel;
use tracing::{debug, info, warn};

use crate::proto;
use crate::proto::cluster_service_client::ClusterServiceClient;

/// Configuration for the node agent.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Address of the control plane's gRPC endpoint.
    pub control_plane_addr: String,
    /// This node's advertised address.
    pub address: String,
    /// This node's advertised port.
    pub port: u16,
    /// Labels for scheduling affinity.
    pub labels: HashMap<String, String>,
    /// Total memory capacity in bytes.
    pub capacity_memory_bytes: u64,
    /// Total CPU weight capacity.
    pub capacity_cpu_weight: u32,
}

/// The node agent that maintains cluster membership.
pub struct NodeAgent {
    config: AgentConfig,
    /// Assigned node ID (set after join).
    node_id: Option<String>,
    /// Heartbeat interval (set by control plane).
    heartbeat_interval: Duration,
}

impl NodeAgent {
    /// Create a new node agent.
    pub fn new(config: AgentConfig) -> Self {
        Self {
            config,
            node_id: None,
            heartbeat_interval: Duration::from_secs(5),
        }
    }

    /// Join the cluster.
    ///
    /// Connects to the control plane and registers this node.
    pub async fn join(&mut self) -> anyhow::Result<String> {
        let mut client = self.connect().await?;

        let response = client
            .join(proto::JoinRequest {
                address: self.config.address.clone(),
                port: self.config.port as u32,
                labels: self.config.labels.clone(),
                capacity_memory_bytes: self.config.capacity_memory_bytes,
                capacity_cpu_weight: self.config.capacity_cpu_weight,
            })
            .await?;

        let resp = response.into_inner();
        self.node_id = Some(resp.node_id.clone());
        self.heartbeat_interval =
            Duration::from_secs(resp.heartbeat_interval_secs as u64);

        info!(
            node_id = %resp.node_id,
            members = resp.members.len(),
            heartbeat_interval = ?self.heartbeat_interval,
            "joined cluster"
        );

        Ok(resp.node_id)
    }

    /// Leave the cluster gracefully.
    pub async fn leave(&self) -> anyhow::Result<()> {
        let node_id = self.node_id.as_ref().ok_or_else(|| {
            anyhow::anyhow!("not joined — call join() first")
        })?;

        let mut client = self.connect().await?;

        client
            .leave(proto::LeaveRequest {
                node_id: node_id.clone(),
            })
            .await?;

        info!(%node_id, "left cluster");
        Ok(())
    }

    /// Run the heartbeat loop.
    ///
    /// Sends periodic heartbeats to the control plane and processes
    /// any commands received in the response.
    pub async fn run_heartbeat(
        &self,
        used_memory_bytes: u64,
        used_cpu_weight: u32,
        mut shutdown: watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        let node_id = self.node_id.as_ref().ok_or_else(|| {
            anyhow::anyhow!("not joined — call join() first")
        })?;

        let mut client = self.connect().await?;

        info!(%node_id, interval = ?self.heartbeat_interval, "heartbeat loop started");

        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.heartbeat_interval) => {
                    match client.heartbeat(proto::HeartbeatRequest {
                        node_id: node_id.clone(),
                        used_memory_bytes,
                        used_cpu_weight,
                        active_instances: 0, // Updated by caller.
                    }).await {
                        Ok(resp) => {
                            let inner = resp.into_inner();
                            debug!(%node_id, ack = inner.acknowledged, "heartbeat sent");

                            for cmd in &inner.commands {
                                info!(
                                    %node_id,
                                    command = %cmd.command_type,
                                    "received command from control plane"
                                );
                            }
                        }
                        Err(e) => {
                            warn!(%node_id, error = %e, "heartbeat failed");
                        }
                    }
                }
                _ = shutdown.changed() => {
                    info!(%node_id, "heartbeat loop shutting down");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Get the assigned node ID (None if not yet joined).
    pub fn node_id(&self) -> Option<&str> {
        self.node_id.as_deref()
    }

    /// Connect to the control plane.
    async fn connect(&self) -> anyhow::Result<ClusterServiceClient<Channel>> {
        let addr = format!("http://{}", self.config.control_plane_addr);
        let client = ClusterServiceClient::connect(addr).await?;
        Ok(client)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> AgentConfig {
        AgentConfig {
            control_plane_addr: "127.0.0.1:50051".to_string(),
            address: "10.0.0.1".to_string(),
            port: 8443,
            labels: HashMap::new(),
            capacity_memory_bytes: 8_000_000_000,
            capacity_cpu_weight: 1000,
        }
    }

    #[test]
    fn agent_creation() {
        let agent = NodeAgent::new(test_config());
        assert!(agent.node_id().is_none());
    }

    #[test]
    fn agent_config_with_labels() {
        let mut config = test_config();
        config.labels.insert("region".to_string(), "us-east-1".to_string());

        let agent = NodeAgent::new(config);
        assert!(agent.node_id().is_none());
    }
}
