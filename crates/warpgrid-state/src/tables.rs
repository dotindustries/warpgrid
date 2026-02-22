//! redb table definitions for the WarpGrid state store.
//!
//! Each table uses `&str` keys and `&[u8]` values (JSON-serialized domain types).
//! Composite keys follow the pattern `{namespace}/{name}` or `{parent_id}:{child_id}`.

use redb::TableDefinition;

/// Deployment specs keyed by `{namespace}/{name}`.
pub const DEPLOYMENTS: TableDefinition<&str, &[u8]> = TableDefinition::new("deployments");

/// Instance state keyed by `{deployment_id}:{instance_index}`.
pub const INSTANCES: TableDefinition<&str, &[u8]> = TableDefinition::new("instances");

/// Node info keyed by `{node_id}`.
pub const NODES: TableDefinition<&str, &[u8]> = TableDefinition::new("nodes");

/// Service endpoints keyed by `{namespace}/{service}`.
pub const SERVICES: TableDefinition<&str, &[u8]> = TableDefinition::new("services");

/// Metrics snapshots keyed by `{deployment_id}:{epoch}`.
pub const METRICS: TableDefinition<&str, &[u8]> = TableDefinition::new("metrics");
