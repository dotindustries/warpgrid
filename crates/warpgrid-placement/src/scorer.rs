//! Node scoring for placement decisions.
//!
//! Evaluates candidate nodes using a weighted combination of:
//! - **Bin-packing** (best-fit): prefer nodes that will be most full after placement
//! - **Affinity**: prefer nodes whose labels match deployment requirements
//! - **Resource availability**: reject nodes that can't fit the workload

use std::collections::HashMap;

/// Resource capacity and usage for a single node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeResources {
    pub node_id: String,
    pub labels: HashMap<String, String>,
    pub capacity_memory_bytes: u64,
    pub capacity_cpu_weight: u32,
    pub used_memory_bytes: u64,
    pub used_cpu_weight: u32,
    pub active_instances: u32,
    pub is_draining: bool,
}

impl NodeResources {
    pub fn free_memory(&self) -> u64 {
        self.capacity_memory_bytes.saturating_sub(self.used_memory_bytes)
    }

    pub fn free_cpu(&self) -> u32 {
        self.capacity_cpu_weight.saturating_sub(self.used_cpu_weight)
    }
}

/// Requirements for a placement.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlacementRequirements {
    /// Memory needed per instance in bytes.
    pub memory_bytes: u64,
    /// CPU weight needed per instance.
    pub cpu_weight: u32,
    /// Number of instances to place.
    pub instance_count: u32,
    /// Required label matches (all must match).
    pub required_labels: HashMap<String, String>,
    /// Preferred label matches (soft affinity, adds score).
    pub preferred_labels: HashMap<String, String>,
    /// Priority (0 = highest, used for preemption ordering).
    pub priority: u32,
}

/// Scored placement result for a single node.
#[derive(Debug, Clone)]
pub struct NodeScore {
    pub node_id: String,
    /// Total composite score (higher = better). Range: 0.0..=100.0.
    pub score: f64,
    /// How many instances this node can accept.
    pub capacity: u32,
    /// Breakdown of score components.
    pub breakdown: ScoreBreakdown,
}

/// Individual score components for debugging.
#[derive(Debug, Clone)]
pub struct ScoreBreakdown {
    /// Bin-packing score: how full the node will be (higher = more packed).
    pub bin_packing: f64,
    /// Affinity score: how well labels match.
    pub affinity: f64,
    /// Balance score: spread instances across nodes.
    pub balance: f64,
}

/// Weights for the scoring components.
#[derive(Debug, Clone)]
pub struct ScoringWeights {
    pub bin_packing: f64,
    pub affinity: f64,
    pub balance: f64,
}

impl Default for ScoringWeights {
    fn default() -> Self {
        Self {
            bin_packing: 0.5,
            affinity: 0.3,
            balance: 0.2,
        }
    }
}

/// Score a single node for the given placement requirements.
pub fn score_node(
    node: &NodeResources,
    req: &PlacementRequirements,
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

    // Check resource capacity.
    let mem_capacity = if req.memory_bytes > 0 {
        node.free_memory() / req.memory_bytes
    } else {
        u64::MAX
    };
    let cpu_capacity = if req.cpu_weight > 0 {
        u64::from(node.free_cpu()) / u64::from(req.cpu_weight)
    } else {
        u64::MAX
    };
    let capacity = mem_capacity.min(cpu_capacity).min(u64::from(u32::MAX)) as u32;

    if capacity == 0 {
        return None;
    }

    let instances_to_place = req.instance_count.min(capacity);

    // Bin-packing score: how full will the node be after placement?
    // Higher = more packed = better for bin-packing strategy.
    let projected_memory = node.used_memory_bytes + req.memory_bytes * u64::from(instances_to_place);
    let bin_packing = if node.capacity_memory_bytes > 0 {
        (projected_memory as f64 / node.capacity_memory_bytes as f64).min(1.0) * 100.0
    } else {
        50.0
    };

    // Affinity score: soft label matching.
    let total_preferred = req.preferred_labels.len();
    let matched = req
        .preferred_labels
        .iter()
        .filter(|(k, v)| node.labels.get(*k).is_some_and(|nv| nv == *v))
        .count();
    let affinity = if total_preferred > 0 {
        (matched as f64 / total_preferred as f64) * 100.0
    } else {
        50.0 // Neutral when no preferences.
    };

    // Balance score: penalize nodes far above average utilization.
    let node_util = if node.capacity_memory_bytes > 0 {
        node.used_memory_bytes as f64 / node.capacity_memory_bytes as f64
    } else {
        0.5
    };
    let balance = (1.0 - (node_util - cluster_avg_utilization).abs()).max(0.0) * 100.0;

    let score = weights.bin_packing * bin_packing
        + weights.affinity * affinity
        + weights.balance * balance;

    Some(NodeScore {
        node_id: node.node_id.clone(),
        score,
        capacity,
        breakdown: ScoreBreakdown {
            bin_packing,
            affinity,
            balance,
        },
    })
}

/// Score all nodes and return a sorted list (best first).
pub fn rank_nodes(
    nodes: &[NodeResources],
    req: &PlacementRequirements,
    weights: &ScoringWeights,
) -> Vec<NodeScore> {
    let cluster_avg = if nodes.is_empty() {
        0.5
    } else {
        let total_util: f64 = nodes
            .iter()
            .map(|n| {
                if n.capacity_memory_bytes > 0 {
                    n.used_memory_bytes as f64 / n.capacity_memory_bytes as f64
                } else {
                    0.5
                }
            })
            .sum();
        total_util / nodes.len() as f64
    };

    let mut scores: Vec<NodeScore> = nodes
        .iter()
        .filter_map(|n| score_node(n, req, weights, cluster_avg))
        .collect();

    // Sort descending by score.
    scores.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    scores
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, cap_mem: u64, used_mem: u64, cap_cpu: u32, used_cpu: u32) -> NodeResources {
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

    fn default_req(mem: u64, cpu: u32) -> PlacementRequirements {
        PlacementRequirements {
            memory_bytes: mem,
            cpu_weight: cpu,
            instance_count: 1,
            required_labels: HashMap::new(),
            preferred_labels: HashMap::new(),
            priority: 10,
        }
    }

    #[test]
    fn rejects_draining_node() {
        let mut node = make_node("n1", 1024, 0, 100, 0);
        node.is_draining = true;
        let req = default_req(128, 10);
        let weights = ScoringWeights::default();

        assert!(score_node(&node, &req, &weights, 0.5).is_none());
    }

    #[test]
    fn rejects_insufficient_memory() {
        let node = make_node("n1", 1024, 1000, 100, 0);
        let req = default_req(128, 10); // Needs 128 but only 24 free.
        let weights = ScoringWeights::default();

        assert!(score_node(&node, &req, &weights, 0.5).is_none());
    }

    #[test]
    fn rejects_missing_required_label() {
        let node = make_node("n1", 1024, 0, 100, 0);
        let mut req = default_req(128, 10);
        req.required_labels.insert("region".to_string(), "us-east".to_string());
        let weights = ScoringWeights::default();

        assert!(score_node(&node, &req, &weights, 0.5).is_none());
    }

    #[test]
    fn accepts_node_with_matching_label() {
        let mut node = make_node("n1", 1024, 0, 100, 0);
        node.labels.insert("region".to_string(), "us-east".to_string());
        let mut req = default_req(128, 10);
        req.required_labels.insert("region".to_string(), "us-east".to_string());
        let weights = ScoringWeights::default();

        let result = score_node(&node, &req, &weights, 0.5);
        assert!(result.is_some());
    }

    #[test]
    fn bin_packing_prefers_fuller_node() {
        let nearly_full = make_node("n1", 1024, 800, 100, 0);
        let mostly_empty = make_node("n2", 1024, 100, 100, 0);
        let req = default_req(128, 10);
        let weights = ScoringWeights {
            bin_packing: 1.0,
            affinity: 0.0,
            balance: 0.0,
        };

        let s1 = score_node(&nearly_full, &req, &weights, 0.5).unwrap();
        let s2 = score_node(&mostly_empty, &req, &weights, 0.5).unwrap();

        assert!(
            s1.score > s2.score,
            "nearly full ({}) should score higher than mostly empty ({}) for bin-packing",
            s1.score, s2.score
        );
    }

    #[test]
    fn preferred_labels_boost_score() {
        let mut labeled = make_node("n1", 1024, 0, 100, 0);
        labeled.labels.insert("gpu".to_string(), "true".to_string());

        let unlabeled = make_node("n2", 1024, 0, 100, 0);

        let mut req = default_req(128, 10);
        req.preferred_labels.insert("gpu".to_string(), "true".to_string());

        let weights = ScoringWeights {
            bin_packing: 0.0,
            affinity: 1.0,
            balance: 0.0,
        };

        let s1 = score_node(&labeled, &req, &weights, 0.5).unwrap();
        let s2 = score_node(&unlabeled, &req, &weights, 0.5).unwrap();

        assert!(s1.score > s2.score);
    }

    #[test]
    fn rank_nodes_returns_sorted() {
        let nodes = vec![
            make_node("n1", 1024, 100, 100, 0),  // Less full.
            make_node("n2", 1024, 800, 100, 0),  // More full — scores higher for bin-packing.
            make_node("n3", 1024, 500, 100, 0),  // Middle.
        ];
        let req = default_req(128, 10);
        let weights = ScoringWeights {
            bin_packing: 1.0,
            affinity: 0.0,
            balance: 0.0,
        };

        let ranked = rank_nodes(&nodes, &req, &weights);

        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0].node_id, "n2"); // Fullest node first.
        assert!(ranked[0].score >= ranked[1].score);
        assert!(ranked[1].score >= ranked[2].score);
    }

    #[test]
    fn capacity_reflects_resources() {
        let node = make_node("n1", 1024, 0, 100, 0);
        let req = default_req(256, 10); // 1024/256 = 4 instances max.
        let weights = ScoringWeights::default();

        let result = score_node(&node, &req, &weights, 0.5).unwrap();
        assert_eq!(result.capacity, 4);
    }
}
