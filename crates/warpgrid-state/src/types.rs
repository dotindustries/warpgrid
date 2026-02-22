//! Domain types for the WarpGrid state store.
//!
//! These types represent the persisted state of deployments, instances,
//! nodes, services, and metrics snapshots. All types are serializable
//! to/from JSON for storage in redb tables.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Unique identifier for a deployment (namespace-scoped).
pub type DeploymentId = String;

/// Unique identifier for an instance within a deployment.
pub type InstanceId = String;

/// Unique identifier for a node in the cluster.
pub type NodeId = String;

// ── Deployment ─────────────────────────────────────────────────────

/// Specification for a deployed Wasm workload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeploymentSpec {
    pub id: DeploymentId,
    pub namespace: String,
    pub name: String,
    /// Source URI (oci://, file://, https://, etc.)
    pub source: String,
    /// Trigger type: "http", "cron", "queue", etc.
    pub trigger: TriggerConfig,
    /// Instance count constraints.
    pub instances: InstanceConstraints,
    /// Resource limits per instance.
    pub resources: ResourceLimits,
    /// Autoscaling configuration.
    pub scaling: Option<ScalingConfig>,
    /// Health check configuration.
    pub health: Option<HealthConfig>,
    /// Which shims to enable for this deployment.
    pub shims: ShimsEnabled,
    /// Environment variables injected into the Wasm module.
    pub env: HashMap<String, String>,
    /// Unix timestamp (seconds) when this spec was created.
    pub created_at: u64,
    /// Unix timestamp (seconds) when this spec was last updated.
    pub updated_at: u64,
}

/// Trigger configuration for a deployment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerConfig {
    Http { port: Option<u16> },
    Cron { schedule: String },
    Queue { topic: String },
}

/// Min/max instance count for a deployment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstanceConstraints {
    pub min: u32,
    pub max: u32,
}

/// Resource limits per Wasm instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceLimits {
    /// Memory limit in bytes.
    pub memory_bytes: u64,
    /// CPU weight (relative, higher = more CPU time).
    pub cpu_weight: u32,
}

/// Autoscaling parameters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScalingConfig {
    /// Metric to scale on: "rps", "latency_p99", "cpu", "memory".
    pub metric: String,
    /// Target value for the metric.
    pub target_value: f64,
    /// Cooldown before scaling up (e.g., "30s").
    pub scale_up_window: String,
    /// Cooldown before scaling down (e.g., "5m").
    pub scale_down_window: String,
}

/// Health check parameters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthConfig {
    /// HTTP path to probe (e.g., "/healthz").
    pub endpoint: String,
    /// Check interval (e.g., "5s").
    pub interval: String,
    /// Timeout per check (e.g., "2s").
    pub timeout: String,
    /// Consecutive failures before marking unhealthy.
    pub unhealthy_threshold: u32,
}

/// Which host shims are enabled for a deployment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ShimsEnabled {
    pub timezone: bool,
    pub dev_urandom: bool,
    pub dns: bool,
    pub signals: bool,
    pub database_proxy: bool,
}

// ── Instance ──────────────────────────────────────────────────────

/// Runtime state of a single Wasm instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstanceState {
    pub id: InstanceId,
    pub deployment_id: DeploymentId,
    pub node_id: NodeId,
    pub status: InstanceStatus,
    pub health: HealthStatus,
    pub restart_count: u32,
    /// Current memory usage in bytes.
    pub memory_bytes: u64,
    /// Unix timestamp when this instance started.
    pub started_at: u64,
    /// Unix timestamp of last status change.
    pub updated_at: u64,
}

/// Lifecycle status of a Wasm instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceStatus {
    Starting,
    Running,
    Unhealthy,
    Stopping,
    Stopped,
}

/// Health status as determined by health probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Healthy,
    Unhealthy,
    Unknown,
}

// ── Node ──────────────────────────────────────────────────────────

/// Information about a node in the cluster.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeInfo {
    pub id: NodeId,
    pub address: String,
    pub port: u16,
    /// Total memory available on this node (bytes).
    pub capacity_memory_bytes: u64,
    /// Total CPU weight capacity on this node.
    pub capacity_cpu_weight: u32,
    /// Current memory usage across all instances (bytes).
    pub used_memory_bytes: u64,
    /// Current CPU weight usage across all instances.
    pub used_cpu_weight: u32,
    /// Arbitrary labels for scheduling affinity.
    pub labels: HashMap<String, String>,
    /// Unix timestamp of last heartbeat.
    pub last_heartbeat: u64,
}

// ── Service ───────────────────────────────────────────────────────

/// Service endpoint entry for internal routing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceEndpoints {
    pub namespace: String,
    pub service: String,
    /// List of backend addresses (ip:port).
    pub endpoints: Vec<String>,
    /// Unix timestamp of last update.
    pub updated_at: u64,
}

// ── Metrics ───────────────────────────────────────────────────────

/// Point-in-time metrics snapshot for a deployment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricsSnapshot {
    pub deployment_id: DeploymentId,
    /// Epoch (unix timestamp, bucketed to interval).
    pub epoch: u64,
    /// Requests per second.
    pub rps: f64,
    /// Latency P50 in milliseconds.
    pub latency_p50_ms: f64,
    /// Latency P99 in milliseconds.
    pub latency_p99_ms: f64,
    /// Error rate (0.0–1.0).
    pub error_rate: f64,
    /// Total memory across all instances (bytes).
    pub total_memory_bytes: u64,
    /// Number of active instances.
    pub active_instances: u32,
}

impl DeploymentSpec {
    /// Build the composite key for the deployments table.
    pub fn table_key(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

impl InstanceState {
    /// Build the composite key for the instances table.
    pub fn table_key(&self) -> String {
        format!("{}:{}", self.deployment_id, self.id)
    }
}

impl ServiceEndpoints {
    /// Build the composite key for the services table.
    pub fn table_key(&self) -> String {
        format!("{}/{}", self.namespace, self.service)
    }
}

impl MetricsSnapshot {
    /// Build the composite key for the metrics table.
    pub fn table_key(&self) -> String {
        format!("{}:{}", self.deployment_id, self.epoch)
    }
}
