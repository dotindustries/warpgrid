//! Membership manager — tracks cluster node state.
//!
//! Manages the set of nodes in the cluster, their status, and
//! detects dead nodes based on missed heartbeats.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tracing::{debug, info, warn};

use warpgrid_state::*;

/// Status of a node in the cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberStatus {
    Joining,
    Ready,
    Draining,
    Left,
    Dead,
}

/// In-memory view of a cluster member.
#[derive(Debug, Clone)]
pub struct Member {
    pub node_id: String,
    pub address: String,
    pub port: u16,
    pub status: MemberStatus,
    pub labels: HashMap<String, String>,
    pub capacity_memory_bytes: u64,
    pub capacity_cpu_weight: u32,
    pub used_memory_bytes: u64,
    pub used_cpu_weight: u32,
    pub last_heartbeat: u64,
}

/// Manages cluster membership state.
///
/// Persists node information to the `StateStore` and provides
/// in-memory lookups for fast scheduling decisions.
pub struct MembershipManager {
    state: StateStore,
    /// Dead node detection threshold.
    dead_timeout: Duration,
    /// Heartbeat interval expected from agents.
    heartbeat_interval: Duration,
}

impl MembershipManager {
    /// Create a new membership manager.
    pub fn new(state: StateStore) -> Self {
        Self {
            state,
            dead_timeout: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(5),
        }
    }

    /// Set the dead node detection timeout.
    pub fn with_dead_timeout(mut self, timeout: Duration) -> Self {
        self.dead_timeout = timeout;
        self
    }

    /// Set the expected heartbeat interval.
    pub fn with_heartbeat_interval(mut self, interval: Duration) -> Self {
        self.heartbeat_interval = interval;
        self
    }

    /// Get the heartbeat interval in seconds.
    pub fn heartbeat_interval_secs(&self) -> u32 {
        self.heartbeat_interval.as_secs() as u32
    }

    /// Register a new node in the cluster.
    ///
    /// Generates a node ID and persists the node info. Returns the
    /// assigned node ID.
    pub fn join(
        &self,
        address: &str,
        port: u16,
        labels: HashMap<String, String>,
        capacity_memory_bytes: u64,
        capacity_cpu_weight: u32,
    ) -> StateResult<String> {
        let node_id = generate_node_id(address, port);
        let now = epoch_secs();

        let node = NodeInfo {
            id: node_id.clone(),
            address: address.to_string(),
            port,
            capacity_memory_bytes,
            capacity_cpu_weight,
            used_memory_bytes: 0,
            used_cpu_weight: 0,
            labels,
            last_heartbeat: now,
        };

        self.state.put_node(&node)?;
        info!(%node_id, %address, port, "node joined cluster");
        Ok(node_id)
    }

    /// Process a heartbeat from a node.
    ///
    /// Updates resource usage and last-seen timestamp.
    pub fn heartbeat(
        &self,
        node_id: &str,
        used_memory_bytes: u64,
        used_cpu_weight: u32,
    ) -> StateResult<bool> {
        let node = self.state.get_node(node_id)?;
        match node {
            Some(mut n) => {
                n.used_memory_bytes = used_memory_bytes;
                n.used_cpu_weight = used_cpu_weight;
                n.last_heartbeat = epoch_secs();
                self.state.put_node(&n)?;
                debug!(%node_id, "heartbeat received");
                Ok(true)
            }
            None => {
                warn!(%node_id, "heartbeat from unknown node");
                Ok(false)
            }
        }
    }

    /// Remove a node from the cluster.
    pub fn leave(&self, node_id: &str) -> StateResult<bool> {
        let existed = self.state.delete_node(node_id)?;
        if existed {
            info!(%node_id, "node left cluster");
        }
        Ok(existed)
    }

    /// List all current members with their status.
    pub fn list_members(&self) -> StateResult<Vec<Member>> {
        let now = epoch_secs();
        let nodes = self.state.list_nodes()?;

        let members = nodes
            .into_iter()
            .map(|n| {
                let status = if now - n.last_heartbeat > self.dead_timeout.as_secs() {
                    MemberStatus::Dead
                } else {
                    MemberStatus::Ready
                };

                Member {
                    node_id: n.id,
                    address: n.address,
                    port: n.port,
                    status,
                    labels: n.labels,
                    capacity_memory_bytes: n.capacity_memory_bytes,
                    capacity_cpu_weight: n.capacity_cpu_weight,
                    used_memory_bytes: n.used_memory_bytes,
                    used_cpu_weight: n.used_cpu_weight,
                    last_heartbeat: n.last_heartbeat,
                }
            })
            .collect();

        Ok(members)
    }

    /// Get a single member by ID.
    pub fn get_member(&self, node_id: &str) -> StateResult<Option<Member>> {
        let now = epoch_secs();
        match self.state.get_node(node_id)? {
            Some(n) => {
                let status = if now - n.last_heartbeat > self.dead_timeout.as_secs() {
                    MemberStatus::Dead
                } else {
                    MemberStatus::Ready
                };

                Ok(Some(Member {
                    node_id: n.id,
                    address: n.address,
                    port: n.port,
                    status,
                    labels: n.labels,
                    capacity_memory_bytes: n.capacity_memory_bytes,
                    capacity_cpu_weight: n.capacity_cpu_weight,
                    used_memory_bytes: n.used_memory_bytes,
                    used_cpu_weight: n.used_cpu_weight,
                    last_heartbeat: n.last_heartbeat,
                }))
            }
            None => Ok(None),
        }
    }

    /// Detect and remove dead nodes.
    ///
    /// Returns the IDs of nodes that were removed.
    pub fn reap_dead_nodes(&self) -> StateResult<Vec<String>> {
        let members = self.list_members()?;
        let mut reaped = Vec::new();

        for member in members {
            if member.status == MemberStatus::Dead {
                self.state.delete_node(&member.node_id)?;
                warn!(node_id = %member.node_id, "reaped dead node");
                reaped.push(member.node_id);
            }
        }

        Ok(reaped)
    }

    /// Count of ready (alive) nodes.
    pub fn ready_count(&self) -> StateResult<usize> {
        let members = self.list_members()?;
        Ok(members.iter().filter(|m| m.status == MemberStatus::Ready).count())
    }
}

/// Generate a deterministic node ID from address and port.
fn generate_node_id(address: &str, port: u16) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    address.hash(&mut hasher);
    port.hash(&mut hasher);
    epoch_secs().hash(&mut hasher);
    format!("node-{:08x}", hasher.finish() as u32)
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

    fn test_state() -> StateStore {
        StateStore::open_in_memory().unwrap()
    }

    #[test]
    fn join_creates_node() {
        let mgr = MembershipManager::new(test_state());
        let node_id = mgr
            .join("10.0.0.1", 8443, HashMap::new(), 8_000_000_000, 1000)
            .unwrap();

        assert!(node_id.starts_with("node-"));
        let member = mgr.get_member(&node_id).unwrap().unwrap();
        assert_eq!(member.address, "10.0.0.1");
        assert_eq!(member.port, 8443);
        assert_eq!(member.status, MemberStatus::Ready);
    }

    #[test]
    fn heartbeat_updates_usage() {
        let mgr = MembershipManager::new(test_state());
        let node_id = mgr
            .join("10.0.0.1", 8443, HashMap::new(), 8_000_000_000, 1000)
            .unwrap();

        mgr.heartbeat(&node_id, 1_000_000_000, 200).unwrap();

        let member = mgr.get_member(&node_id).unwrap().unwrap();
        assert_eq!(member.used_memory_bytes, 1_000_000_000);
        assert_eq!(member.used_cpu_weight, 200);
    }

    #[test]
    fn heartbeat_unknown_node_returns_false() {
        let mgr = MembershipManager::new(test_state());
        let ack = mgr.heartbeat("unknown", 0, 0).unwrap();
        assert!(!ack);
    }

    #[test]
    fn leave_removes_node() {
        let mgr = MembershipManager::new(test_state());
        let node_id = mgr
            .join("10.0.0.1", 8443, HashMap::new(), 8_000_000_000, 1000)
            .unwrap();

        assert!(mgr.leave(&node_id).unwrap());
        assert!(mgr.get_member(&node_id).unwrap().is_none());
    }

    #[test]
    fn list_members_returns_all() {
        let mgr = MembershipManager::new(test_state());
        mgr.join("10.0.0.1", 8443, HashMap::new(), 8_000_000_000, 1000)
            .unwrap();
        mgr.join("10.0.0.2", 8443, HashMap::new(), 8_000_000_000, 1000)
            .unwrap();

        let members = mgr.list_members().unwrap();
        assert_eq!(members.len(), 2);
    }

    #[test]
    fn dead_node_detection() {
        let state = test_state();
        let mgr = MembershipManager::new(state.clone())
            .with_dead_timeout(Duration::from_secs(0));

        let node_id = mgr
            .join("10.0.0.1", 8443, HashMap::new(), 8_000_000_000, 1000)
            .unwrap();

        // With 0s timeout, node should be immediately "dead" on next check
        // (since last_heartbeat == now, we need it to be in the past).
        // Manually set heartbeat to old timestamp.
        let mut node = state.get_node(&node_id).unwrap().unwrap();
        node.last_heartbeat = 1000; // Very old.
        state.put_node(&node).unwrap();

        let member = mgr.get_member(&node_id).unwrap().unwrap();
        assert_eq!(member.status, MemberStatus::Dead);
    }

    #[test]
    fn reap_dead_nodes() {
        let state = test_state();
        let mgr = MembershipManager::new(state.clone())
            .with_dead_timeout(Duration::from_secs(0));

        let node_id = mgr
            .join("10.0.0.1", 8443, HashMap::new(), 8_000_000_000, 1000)
            .unwrap();

        // Make it dead.
        let mut node = state.get_node(&node_id).unwrap().unwrap();
        node.last_heartbeat = 1000;
        state.put_node(&node).unwrap();

        let reaped = mgr.reap_dead_nodes().unwrap();
        assert_eq!(reaped.len(), 1);
        assert!(mgr.list_members().unwrap().is_empty());
    }

    #[test]
    fn ready_count() {
        let mgr = MembershipManager::new(test_state());
        mgr.join("10.0.0.1", 8443, HashMap::new(), 8_000_000_000, 1000)
            .unwrap();
        mgr.join("10.0.0.2", 8443, HashMap::new(), 8_000_000_000, 1000)
            .unwrap();

        assert_eq!(mgr.ready_count().unwrap(), 2);
    }

    #[test]
    fn labels_preserved() {
        let mgr = MembershipManager::new(test_state());
        let mut labels = HashMap::new();
        labels.insert("region".to_string(), "us-east-1".to_string());
        labels.insert("zone".to_string(), "a".to_string());

        let node_id = mgr
            .join("10.0.0.1", 8443, labels.clone(), 8_000_000_000, 1000)
            .unwrap();

        let member = mgr.get_member(&node_id).unwrap().unwrap();
        assert_eq!(member.labels.get("region").unwrap(), "us-east-1");
        assert_eq!(member.labels.get("zone").unwrap(), "a");
    }
}
