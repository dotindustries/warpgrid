//! Type conversions between state store types and placement types.
//!
//! Bridges `warpgrid_state::{NodeInfo, DeploymentSpec}` to the placement
//! engine's `NodeResources` and `PlacementRequirements`.

use std::collections::HashMap;

use warpgrid_state::{DeploymentSpec, NodeInfo};

use crate::scorer::{NodeResources, PlacementRequirements};

/// Default priority for deployments that don't specify one.
const DEFAULT_PRIORITY: u32 = 10;

/// Convert a [`NodeInfo`] to [`NodeResources`] for placement.
///
/// `is_draining` is passed externally because drain state is managed
/// by the cluster layer, not the state store.
pub fn node_info_to_resources(node: &NodeInfo, is_draining: bool) -> NodeResources {
    NodeResources {
        node_id: node.id.clone(),
        labels: node.labels.clone(),
        capacity_memory_bytes: node.capacity_memory_bytes,
        capacity_cpu_weight: node.capacity_cpu_weight,
        used_memory_bytes: node.used_memory_bytes,
        used_cpu_weight: node.used_cpu_weight,
        active_instances: 0,
        is_draining,
    }
}

/// Convert a [`NodeInfo`] to [`NodeResources`] with an explicit instance count.
pub fn node_info_to_resources_with_instances(
    node: &NodeInfo,
    active_instances: u32,
    is_draining: bool,
) -> NodeResources {
    NodeResources {
        node_id: node.id.clone(),
        labels: node.labels.clone(),
        capacity_memory_bytes: node.capacity_memory_bytes,
        capacity_cpu_weight: node.capacity_cpu_weight,
        used_memory_bytes: node.used_memory_bytes,
        used_cpu_weight: node.used_cpu_weight,
        active_instances,
        is_draining,
    }
}

/// Convert a [`DeploymentSpec`] to [`PlacementRequirements`].
pub fn deployment_to_requirements(
    spec: &DeploymentSpec,
    instance_count: u32,
) -> PlacementRequirements {
    PlacementRequirements {
        memory_bytes: spec.resources.memory_bytes,
        cpu_weight: spec.resources.cpu_weight,
        instance_count,
        required_labels: HashMap::new(),
        preferred_labels: HashMap::new(),
        priority: DEFAULT_PRIORITY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use warpgrid_state::*;

    fn sample_node() -> NodeInfo {
        NodeInfo {
            id: "node-1".to_string(),
            address: "10.0.0.1".to_string(),
            port: 8443,
            capacity_memory_bytes: 8 * 1024 * 1024 * 1024,
            capacity_cpu_weight: 1000,
            used_memory_bytes: 2 * 1024 * 1024 * 1024,
            used_cpu_weight: 250,
            labels: {
                let mut m = HashMap::new();
                m.insert("region".to_string(), "us-east".to_string());
                m.insert("gpu".to_string(), "true".to_string());
                m
            },
            last_heartbeat: 1700000000,
        }
    }

    fn sample_deployment() -> DeploymentSpec {
        DeploymentSpec {
            id: "prod/api".to_string(),
            namespace: "prod".to_string(),
            name: "api".to_string(),
            source: "oci://registry/api:v1".to_string(),
            trigger: TriggerConfig::Http { port: Some(8080) },
            instances: InstanceConstraints { min: 3, max: 10 },
            resources: ResourceLimits {
                memory_bytes: 128 * 1024 * 1024,
                cpu_weight: 100,
            },
            scaling: None,
            health: None,
            shims: ShimsEnabled::default(),
            env: HashMap::new(),
            created_at: 1000,
            updated_at: 1000,
        }
    }

    #[test]
    fn converts_node_info_fields() {
        let node = sample_node();
        let res = node_info_to_resources(&node, false);

        assert_eq!(res.node_id, "node-1");
        assert_eq!(res.capacity_memory_bytes, 8 * 1024 * 1024 * 1024);
        assert_eq!(res.capacity_cpu_weight, 1000);
        assert_eq!(res.used_memory_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(res.used_cpu_weight, 250);
        assert_eq!(res.active_instances, 0);
        assert!(!res.is_draining);
    }

    #[test]
    fn preserves_labels() {
        let node = sample_node();
        let res = node_info_to_resources(&node, false);

        assert_eq!(res.labels.get("region"), Some(&"us-east".to_string()));
        assert_eq!(res.labels.get("gpu"), Some(&"true".to_string()));
        assert_eq!(res.labels.len(), 2);
    }

    #[test]
    fn draining_flag_propagates() {
        let node = sample_node();

        let not_draining = node_info_to_resources(&node, false);
        assert!(!not_draining.is_draining);

        let draining = node_info_to_resources(&node, true);
        assert!(draining.is_draining);
    }

    #[test]
    fn with_instances_sets_count() {
        let node = sample_node();
        let res = node_info_to_resources_with_instances(&node, 42, false);

        assert_eq!(res.active_instances, 42);
        assert_eq!(res.node_id, "node-1");
    }

    #[test]
    fn converts_deployment_resources() {
        let spec = sample_deployment();
        let req = deployment_to_requirements(&spec, 5);

        assert_eq!(req.memory_bytes, 128 * 1024 * 1024);
        assert_eq!(req.cpu_weight, 100);
        assert_eq!(req.instance_count, 5);
    }

    #[test]
    fn default_priority_is_medium() {
        let spec = sample_deployment();
        let req = deployment_to_requirements(&spec, 1);
        assert_eq!(req.priority, 10);
    }

    #[test]
    fn labels_default_to_empty() {
        let spec = sample_deployment();
        let req = deployment_to_requirements(&spec, 1);
        assert!(req.required_labels.is_empty());
        assert!(req.preferred_labels.is_empty());
    }

    #[test]
    fn instance_count_is_caller_controlled() {
        let spec = sample_deployment();

        let req_min = deployment_to_requirements(&spec, spec.instances.min);
        assert_eq!(req_min.instance_count, 3);

        let req_max = deployment_to_requirements(&spec, spec.instances.max);
        assert_eq!(req_max.instance_count, 10);
    }

    #[test]
    fn converted_types_work_with_scorer() {
        use crate::scorer::{ScoringWeights, score_node};

        let node = sample_node();
        let spec = sample_deployment();

        let resources = node_info_to_resources(&node, false);
        let requirements = deployment_to_requirements(&spec, 3);
        let weights = ScoringWeights::default();

        let score = score_node(&resources, &requirements, &weights, 0.3);
        assert!(score.is_some());
        assert!(score.unwrap().score > 0.0);
    }
}
