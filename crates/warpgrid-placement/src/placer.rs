//! Placement engine — coordinates scheduling across nodes.
//!
//! Given a set of nodes and deployment specs, the placer decides:
//! 1. Which nodes receive instances (using scorer)
//! 2. Preemption if no node has capacity (evict lower-priority workloads)
//! 3. Placement spread across nodes (anti-affinity for HA)

use std::collections::HashMap;

use tracing::{debug, info, warn};

use crate::scorer::{
    NodeResources, PlacementRequirements, ScoringWeights, rank_nodes,
};

/// A placement decision for a single deployment.
#[derive(Debug, Clone)]
pub struct PlacementPlan {
    pub deployment_id: String,
    /// Node-id → number of instances to place on that node.
    pub assignments: HashMap<String, u32>,
    /// Preemption decisions: (deployment_id, node_id, count_to_evict).
    pub preemptions: Vec<Preemption>,
}

/// A preemption decision — evict instances to make room.
#[derive(Debug, Clone)]
pub struct Preemption {
    pub victim_deployment_id: String,
    pub node_id: String,
    pub count: u32,
}

/// Current state of instances per deployment per node.
#[derive(Debug, Clone)]
pub struct RunningState {
    pub deployment_id: String,
    pub node_id: String,
    pub instance_count: u32,
    pub priority: u32,
    pub memory_per_instance: u64,
    pub cpu_per_instance: u32,
}

/// Compute a placement plan for a deployment across available nodes.
pub fn compute_placement(
    req: &PlacementRequirements,
    deployment_id: &str,
    nodes: &[NodeResources],
    weights: &ScoringWeights,
) -> PlacementPlan {
    let ranked = rank_nodes(nodes, req, weights);

    let mut remaining = req.instance_count;
    let mut assignments: HashMap<String, u32> = HashMap::new();

    for node in &ranked {
        if remaining == 0 {
            break;
        }
        let to_place = remaining.min(node.capacity);
        assignments.insert(node.node_id.clone(), to_place);
        remaining -= to_place;
        debug!(
            node = %node.node_id,
            instances = to_place,
            score = node.score,
            "placed instances"
        );
    }

    if remaining > 0 {
        warn!(
            deployment = deployment_id,
            remaining,
            "could not place all instances — insufficient cluster capacity"
        );
    }

    PlacementPlan {
        deployment_id: deployment_id.to_string(),
        assignments,
        preemptions: Vec::new(),
    }
}

/// Compute a placement plan with preemption support.
///
/// If normal placement can't fit all instances, attempt to preempt
/// lower-priority workloads on the best-scoring nodes.
pub fn compute_placement_with_preemption(
    req: &PlacementRequirements,
    deployment_id: &str,
    nodes: &[NodeResources],
    running: &[RunningState],
    weights: &ScoringWeights,
) -> PlacementPlan {
    // First try normal placement.
    let mut plan = compute_placement(req, deployment_id, nodes, weights);

    let placed: u32 = plan.assignments.values().sum();
    let mut remaining = req.instance_count.saturating_sub(placed);

    if remaining == 0 {
        return plan;
    }

    // Preemption: find lower-priority victims on feasible nodes.
    // Only preempt workloads with strictly higher priority number (lower importance).
    let mut victims: Vec<&RunningState> = running
        .iter()
        .filter(|r| r.priority > req.priority && r.deployment_id != deployment_id)
        .collect();

    // Sort by priority descending (lowest importance first).
    victims.sort_by(|a, b| b.priority.cmp(&a.priority));

    for victim in victims {
        if remaining == 0 {
            break;
        }

        // Find the node for this victim.
        let node = nodes.iter().find(|n| n.node_id == victim.node_id);
        let Some(node) = node else { continue };

        // Check required labels still match.
        let labels_ok = req
            .required_labels
            .iter()
            .all(|(k, v)| node.labels.get(k).is_some_and(|nv| nv == v));
        if !labels_ok {
            continue;
        }

        // How many instances we can reclaim by evicting this victim.
        let mem_freed = victim.memory_per_instance * u64::from(victim.instance_count);
        let cpu_freed = victim.cpu_per_instance * victim.instance_count;

        let mem_gain = if req.memory_bytes > 0 {
            mem_freed / req.memory_bytes
        } else {
            u64::MAX
        };
        let cpu_gain = if req.cpu_weight > 0 {
            u64::from(cpu_freed / req.cpu_weight)
        } else {
            u64::MAX
        };
        let instances_gained = mem_gain.min(cpu_gain).min(u64::from(u32::MAX)) as u32;

        if instances_gained == 0 {
            continue;
        }

        let to_evict = victim.instance_count.min(
            ((remaining as f64 / instances_gained.max(1) as f64).ceil() as u32)
                .min(victim.instance_count),
        );
        let to_place = instances_gained.min(remaining);

        plan.preemptions.push(Preemption {
            victim_deployment_id: victim.deployment_id.clone(),
            node_id: victim.node_id.clone(),
            count: to_evict,
        });

        *plan.assignments.entry(victim.node_id.clone()).or_insert(0) += to_place;
        remaining = remaining.saturating_sub(to_place);

        info!(
            victim_deployment = %victim.deployment_id,
            node = %victim.node_id,
            evicted = to_evict,
            gained = to_place,
            "preempted lower-priority workload"
        );
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, cap_mem: u64, used_mem: u64) -> NodeResources {
        NodeResources {
            node_id: id.to_string(),
            labels: HashMap::new(),
            capacity_memory_bytes: cap_mem,
            capacity_cpu_weight: 1000,
            used_memory_bytes: used_mem,
            used_cpu_weight: 0,
            active_instances: 0,
            is_draining: false,
        }
    }

    fn default_req(mem: u64, count: u32) -> PlacementRequirements {
        PlacementRequirements {
            memory_bytes: mem,
            cpu_weight: 0,
            instance_count: count,
            required_labels: HashMap::new(),
            preferred_labels: HashMap::new(),
            priority: 5,
        }
    }

    #[test]
    fn simple_placement_single_node() {
        let nodes = vec![make_node("n1", 1024, 0)];
        let req = default_req(128, 3);
        let weights = ScoringWeights::default();

        let plan = compute_placement(&req, "deploy/a", &nodes, &weights);

        assert_eq!(plan.assignments.get("n1"), Some(&3));
        assert!(plan.preemptions.is_empty());
    }

    #[test]
    fn placement_spreads_across_nodes() {
        // Two nodes, each can fit 2 instances. Need 3 total.
        let nodes = vec![
            make_node("n1", 256, 0),
            make_node("n2", 256, 0),
        ];
        let req = default_req(128, 3);
        let weights = ScoringWeights::default();

        let plan = compute_placement(&req, "deploy/a", &nodes, &weights);

        let total: u32 = plan.assignments.values().sum();
        assert_eq!(total, 3);
        assert_eq!(plan.assignments.len(), 2); // Spread across 2 nodes.
    }

    #[test]
    fn placement_partial_when_insufficient() {
        let nodes = vec![make_node("n1", 256, 0)]; // Only fits 2.
        let req = default_req(128, 5);
        let weights = ScoringWeights::default();

        let plan = compute_placement(&req, "deploy/a", &nodes, &weights);

        let total: u32 = plan.assignments.values().sum();
        assert_eq!(total, 2); // Only 2 fit.
    }

    #[test]
    fn preemption_evicts_lower_priority() {
        // Node is full, but has a low-priority workload.
        let nodes = vec![make_node("n1", 1024, 1024)]; // Full!
        let req = PlacementRequirements {
            memory_bytes: 256,
            cpu_weight: 0,
            instance_count: 2,
            required_labels: HashMap::new(),
            preferred_labels: HashMap::new(),
            priority: 5, // Higher importance (lower number).
        };

        let running = vec![RunningState {
            deployment_id: "deploy/low".to_string(),
            node_id: "n1".to_string(),
            instance_count: 4,
            priority: 10, // Lower importance.
            memory_per_instance: 256,
            cpu_per_instance: 0,
        }];

        let weights = ScoringWeights::default();
        let plan = compute_placement_with_preemption(
            &req, "deploy/high", &nodes, &running, &weights,
        );

        assert!(!plan.preemptions.is_empty());
        assert_eq!(plan.preemptions[0].victim_deployment_id, "deploy/low");
    }

    #[test]
    fn no_preemption_for_same_or_higher_priority() {
        let nodes = vec![make_node("n1", 1024, 1024)];
        let req = PlacementRequirements {
            memory_bytes: 256,
            cpu_weight: 0,
            instance_count: 2,
            required_labels: HashMap::new(),
            preferred_labels: HashMap::new(),
            priority: 10,
        };

        let running = vec![RunningState {
            deployment_id: "deploy/other".to_string(),
            node_id: "n1".to_string(),
            instance_count: 4,
            priority: 5, // Higher importance — can't preempt.
            memory_per_instance: 256,
            cpu_per_instance: 0,
        }];

        let weights = ScoringWeights::default();
        let plan = compute_placement_with_preemption(
            &req, "deploy/low", &nodes, &running, &weights,
        );

        assert!(plan.preemptions.is_empty());
    }
}
