//! Sprite-specific placement logic.
//!
//! Sprites require more resources than Wasm instances (GBs vs MBs) and
//! benefit from NVMe cache locality. This module scores nodes for sprite
//! placement with additional considerations:
//!
//! - Cache affinity: prefer nodes that previously ran this sprite (warm cache)
//! - Resource fit: sprites need 2+ vCPUs and 4+ GB memory
//! - Fungible resources: sprites and Wasm instances share the same node capacity

use std::collections::HashMap;

use crate::scorer::{NodeResources, NodeScore, ScoreBreakdown, ScoringWeights};

/// Placement requirements for a sprite VM.
#[derive(Debug, Clone)]
pub struct SpriteRequirements {
    /// Memory needed in bytes (e.g., 4 GB = 4 * 1024^3).
    pub memory_bytes: u64,
    /// CPU weight needed (e.g., 200 for 2 vCPUs at weight 100 each).
    pub cpu_weight: u32,
    /// Required labels (all must match).
    pub required_labels: HashMap<String, String>,
    /// Preferred labels (soft affinity).
    pub preferred_labels: HashMap<String, String>,
    /// Node that last ran this sprite (for cache affinity). None for new sprites.
    pub preferred_node_id: Option<String>,
    /// Priority (lower number = higher importance).
    pub priority: u32,
}

/// Convert a `SpriteSpec` into placement requirements.
pub fn sprite_to_requirements(
    spec: &warpgrid_state::SpriteSpec,
    last_node_id: Option<String>,
) -> SpriteRequirements {
    SpriteRequirements {
        memory_bytes: u64::from(spec.resources.memory_mb) * 1024 * 1024,
        cpu_weight: spec.resources.vcpus * 100,
        required_labels: HashMap::new(),
        preferred_labels: HashMap::new(),
        preferred_node_id: last_node_id.or(spec.node_id.clone()),
        priority: 5, // Sprites are higher priority than default Wasm workloads.
    }
}

/// Score and rank nodes for sprite placement.
///
/// Similar to `rank_nodes` but with an additional cache affinity bonus
/// for the node that last ran this sprite.
pub fn rank_nodes_for_sprite(
    nodes: &[NodeResources],
    req: &SpriteRequirements,
    weights: &ScoringWeights,
) -> Vec<NodeScore> {
    let cluster_avg = if nodes.is_empty() {
        0.5
    } else {
        let total: f64 = nodes
            .iter()
            .map(|n| {
                if n.capacity_memory_bytes > 0 {
                    n.used_memory_bytes as f64 / n.capacity_memory_bytes as f64
                } else {
                    0.5
                }
            })
            .sum();
        total / nodes.len() as f64
    };

    let mut scores: Vec<NodeScore> = nodes
        .iter()
        .filter_map(|node| score_node_for_sprite(node, req, weights, cluster_avg))
        .collect();

    scores.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    scores
}

/// Score a single node for sprite placement.
fn score_node_for_sprite(
    node: &NodeResources,
    req: &SpriteRequirements,
    weights: &ScoringWeights,
    cluster_avg_utilization: f64,
) -> Option<NodeScore> {
    // Reject draining nodes.
    if node.is_draining {
        return None;
    }

    // Check hard label constraints.
    for (key, value) in &req.required_labels {
        match node.labels.get(key) {
            Some(v) if v == value => {}
            _ => return None,
        }
    }

    // Check resource capacity (sprites need more resources).
    if node.free_memory() < req.memory_bytes {
        return None;
    }
    if node.free_cpu() < req.cpu_weight {
        return None;
    }

    // Bin-packing score.
    let projected_memory = node.used_memory_bytes + req.memory_bytes;
    let bin_packing = if node.capacity_memory_bytes > 0 {
        (projected_memory as f64 / node.capacity_memory_bytes as f64).min(1.0) * 100.0
    } else {
        50.0
    };

    // Affinity score (labels + cache locality).
    let total_preferred = req.preferred_labels.len();
    let matched = req
        .preferred_labels
        .iter()
        .filter(|(k, v)| node.labels.get(*k).is_some_and(|nv| nv == *v))
        .count();

    let label_affinity = if total_preferred > 0 {
        (matched as f64 / total_preferred as f64) * 100.0
    } else {
        50.0
    };

    // Cache affinity bonus: +30 points if this was the last node.
    let cache_bonus = match &req.preferred_node_id {
        Some(preferred) if preferred == &node.node_id => 30.0,
        _ => 0.0,
    };

    let affinity = (label_affinity + cache_bonus).min(100.0);

    // Balance score.
    let node_util = if node.capacity_memory_bytes > 0 {
        node.used_memory_bytes as f64 / node.capacity_memory_bytes as f64
    } else {
        0.5
    };
    let balance = (1.0 - (node_util - cluster_avg_utilization).abs()).max(0.0) * 100.0;

    let score =
        weights.bin_packing * bin_packing + weights.affinity * affinity + weights.balance * balance;

    Some(NodeScore {
        node_id: node.node_id.clone(),
        score,
        capacity: 1, // Sprites are placed one at a time.
        breakdown: ScoreBreakdown {
            bin_packing,
            affinity,
            balance,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(
        id: &str,
        cap_mem: u64,
        used_mem: u64,
        cap_cpu: u32,
        used_cpu: u32,
    ) -> NodeResources {
        NodeResources {
            node_id: id.to_string(),
            labels: HashMap::new(),
            capacity_memory_bytes: cap_mem,
            capacity_cpu_weight: cap_cpu,
            used_memory_bytes: used_mem,
            used_cpu_weight: used_cpu,
            active_instances: 0,
            is_draining: false,
        }
    }

    fn sprite_req(mem: u64, cpu: u32) -> SpriteRequirements {
        SpriteRequirements {
            memory_bytes: mem,
            cpu_weight: cpu,
            required_labels: HashMap::new(),
            preferred_labels: HashMap::new(),
            preferred_node_id: None,
            priority: 5,
        }
    }

    #[test]
    fn rejects_node_with_insufficient_memory() {
        // Node has 2GB free, sprite needs 4GB.
        let node = make_node("n1", 8 * GB, 6 * GB, 1000, 0);
        let req = sprite_req(4 * GB, 200);
        let weights = ScoringWeights::default();

        assert!(score_node_for_sprite(&node, &req, &weights, 0.5).is_none());
    }

    #[test]
    fn accepts_node_with_sufficient_resources() {
        let node = make_node("n1", 64 * GB, 10 * GB, 6400, 0);
        let req = sprite_req(4 * GB, 200);
        let weights = ScoringWeights::default();

        let result = score_node_for_sprite(&node, &req, &weights, 0.5);
        assert!(result.is_some());
    }

    #[test]
    fn cache_affinity_prefers_last_node() {
        let n1 = make_node("n1", 64 * GB, 10 * GB, 6400, 0);
        let n2 = make_node("n2", 64 * GB, 10 * GB, 6400, 0);

        let mut req = sprite_req(4 * GB, 200);
        req.preferred_node_id = Some("n1".to_string());

        let weights = ScoringWeights {
            bin_packing: 0.0,
            affinity: 1.0,
            balance: 0.0,
        };

        let s1 = score_node_for_sprite(&n1, &req, &weights, 0.5).unwrap();
        let s2 = score_node_for_sprite(&n2, &req, &weights, 0.5).unwrap();

        assert!(
            s1.score > s2.score,
            "cached node ({}) should score higher than non-cached ({})",
            s1.score,
            s2.score
        );
    }

    #[test]
    fn rank_returns_best_first() {
        let nodes = vec![
            make_node("n1", 64 * GB, 50 * GB, 6400, 0), // More full.
            make_node("n2", 64 * GB, 10 * GB, 6400, 0), // Less full.
        ];
        let req = sprite_req(4 * GB, 200);
        let weights = ScoringWeights {
            bin_packing: 1.0,
            affinity: 0.0,
            balance: 0.0,
        };

        let ranked = rank_nodes_for_sprite(&nodes, &req, &weights);
        assert_eq!(ranked.len(), 2);
        assert!(ranked[0].score >= ranked[1].score);
    }

    #[test]
    fn sprite_to_requirements_converts_spec() {
        let spec = warpgrid_state::SpriteSpec {
            id: "sprite-1".to_string(),
            owner: "alice".to_string(),
            name: "workspace".to_string(),
            image_version: "v1".to_string(),
            resources: warpgrid_state::SpriteResources {
                vcpus: 4,
                memory_mb: 8192,
                disk_gb: 100,
            },
            storage_url: String::new(),
            checkpoint_id: None,
            status: warpgrid_state::SpriteStatus::Running,
            node_id: Some("node-2".to_string()),
            created_at: 1000,
            last_active_at: 1000,
        };

        let req = super::sprite_to_requirements(&spec, None);
        assert_eq!(req.memory_bytes, 8192 * 1024 * 1024);
        assert_eq!(req.cpu_weight, 400);
        assert_eq!(req.preferred_node_id, Some("node-2".to_string()));
    }

    const GB: u64 = 1024 * 1024 * 1024;
}
