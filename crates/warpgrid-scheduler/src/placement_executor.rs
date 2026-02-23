//! Placement executor — applies a PlacementPlan to the cluster.
//!
//! Takes a `PlacementPlan` from the placement engine and splits it into:
//! - Local assignments (handled by the scheduler on this node)
//! - Remote assignments (dispatched as `NodeCommand`s via heartbeat responses)

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use warpgrid_placement::placer::PlacementPlan;
use warpgrid_state::*;

use crate::error::{SchedulerError, SchedulerResult};

/// A command to be sent to a remote node via heartbeat response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeCommand {
    pub node_id: String,
    pub command_type: String,
    pub payload: String,
}

/// Payload for a "schedule" command sent to a remote node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulePayload {
    pub deployment_id: String,
    pub instance_count: u32,
}

/// Result of executing a placement plan.
#[derive(Debug)]
pub struct ExecutionResult {
    pub remote_commands: Vec<NodeCommand>,
    pub local_instances: u32,
}

/// Apply a [`PlacementPlan`] — write instance state and generate remote commands.
///
/// Assignments matching `local_node_id` are recorded but no command is generated.
/// Remote assignments produce `NodeCommand` structs for dispatch.
pub fn execute(
    plan: &PlacementPlan,
    local_node_id: &str,
    state: &StateStore,
) -> SchedulerResult<ExecutionResult> {
    let mut remote_commands = Vec::new();
    let mut local_instances: u32 = 0;
    let now = epoch_secs();
    let mut global_idx: u32 = 0;

    for (node_id, &count) in &plan.assignments {
        for _i in 0..count {
            let instance_state = InstanceState {
                id: format!("inst-{global_idx}"),
                deployment_id: plan.deployment_id.clone(),
                node_id: node_id.clone(),
                status: InstanceStatus::Starting,
                health: HealthStatus::Unknown,
                restart_count: 0,
                memory_bytes: 0,
                started_at: now,
                updated_at: now,
            };
            state
                .put_instance(&instance_state)
                .map_err(SchedulerError::State)?;
            global_idx += 1;
        }

        if node_id == local_node_id {
            local_instances += count;
            debug!(
                deployment = %plan.deployment_id,
                count,
                "local assignment — scheduler will handle"
            );
        } else {
            let payload = SchedulePayload {
                deployment_id: plan.deployment_id.clone(),
                instance_count: count,
            };
            let payload_json = serde_json::to_string(&payload)
                .map_err(|e| SchedulerError::Placement(format!("serialize payload: {e}")))?;

            remote_commands.push(NodeCommand {
                node_id: node_id.clone(),
                command_type: "schedule".to_string(),
                payload: payload_json,
            });

            info!(
                deployment = %plan.deployment_id,
                target_node = %node_id,
                count,
                "remote assignment — command queued"
            );
        }
    }

    Ok(ExecutionResult {
        remote_commands,
        local_instances,
    })
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

    fn make_plan(
        deployment_id: &str,
        assignments: Vec<(&str, u32)>,
    ) -> PlacementPlan {
        PlacementPlan {
            deployment_id: deployment_id.to_string(),
            assignments: assignments
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            preemptions: Vec::new(),
        }
    }

    fn test_state() -> StateStore {
        StateStore::open_in_memory().unwrap()
    }

    #[test]
    fn all_local_produces_no_commands() {
        let plan = make_plan("deploy/a", vec![("node-1", 3)]);
        let state = test_state();

        let result = execute(&plan, "node-1", &state).unwrap();

        assert!(result.remote_commands.is_empty());
        assert_eq!(result.local_instances, 3);
    }

    #[test]
    fn all_remote_produces_commands() {
        let plan = make_plan("deploy/a", vec![("node-2", 2), ("node-3", 1)]);
        let state = test_state();

        let result = execute(&plan, "node-1", &state).unwrap();

        assert_eq!(result.remote_commands.len(), 2);
        assert_eq!(result.local_instances, 0);

        for cmd in &result.remote_commands {
            assert_eq!(cmd.command_type, "schedule");
            let payload: SchedulePayload =
                serde_json::from_str(&cmd.payload).unwrap();
            assert_eq!(payload.deployment_id, "deploy/a");
        }
    }

    #[test]
    fn mixed_local_and_remote() {
        let plan = make_plan("deploy/a", vec![("node-1", 2), ("node-2", 3)]);
        let state = test_state();

        let result = execute(&plan, "node-1", &state).unwrap();

        assert_eq!(result.local_instances, 2);
        assert_eq!(result.remote_commands.len(), 1);
        assert_eq!(result.remote_commands[0].node_id, "node-2");

        let payload: SchedulePayload =
            serde_json::from_str(&result.remote_commands[0].payload).unwrap();
        assert_eq!(payload.instance_count, 3);
    }

    #[test]
    fn writes_instance_state_for_all_assignments() {
        let plan = make_plan("deploy/a", vec![("node-1", 2), ("node-2", 1)]);
        let state = test_state();

        execute(&plan, "node-1", &state).unwrap();

        let instances = state
            .list_instances_for_deployment("deploy/a")
            .unwrap();
        assert_eq!(instances.len(), 3);

        let local_count = instances.iter().filter(|i| i.node_id == "node-1").count();
        let remote_count = instances.iter().filter(|i| i.node_id == "node-2").count();
        assert_eq!(local_count, 2);
        assert_eq!(remote_count, 1);
    }

    #[test]
    fn instance_state_starts_as_starting() {
        let plan = make_plan("deploy/a", vec![("node-1", 1)]);
        let state = test_state();

        execute(&plan, "node-1", &state).unwrap();

        let instances = state
            .list_instances_for_deployment("deploy/a")
            .unwrap();
        assert_eq!(instances[0].status, InstanceStatus::Starting);
        assert_eq!(instances[0].health, HealthStatus::Unknown);
    }

    #[test]
    fn empty_plan_is_noop() {
        let plan = make_plan("deploy/a", vec![]);
        let state = test_state();

        let result = execute(&plan, "node-1", &state).unwrap();

        assert!(result.remote_commands.is_empty());
        assert_eq!(result.local_instances, 0);
    }

    #[test]
    fn command_payload_deserializes() {
        let plan = make_plan("deploy/svc", vec![("node-2", 5)]);
        let state = test_state();

        let result = execute(&plan, "node-1", &state).unwrap();
        let cmd = &result.remote_commands[0];

        let payload: SchedulePayload =
            serde_json::from_str(&cmd.payload).unwrap();
        assert_eq!(payload.deployment_id, "deploy/svc");
        assert_eq!(payload.instance_count, 5);
    }
}
