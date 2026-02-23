//! WarpGrid distributed scheduler — bin-packing, affinity, preemption.
//!
//! This crate provides placement decisions for scheduling deployments
//! across a multi-node cluster. It does NOT manage local instance pools
//! (that's `warpgrid-scheduler`). Instead, it scores nodes and produces
//! placement plans that the orchestrator executes.
//!
//! # Components
//!
//! - **`scorer`** — Node scoring (bin-packing, affinity, balance)
//! - **`placer`** — Placement engine (assignments, preemption)
//! - **`convert`** — Type conversions from state store types

pub mod convert;
pub mod placer;
pub mod scorer;

pub use convert::{deployment_to_requirements, node_info_to_resources, node_info_to_resources_with_instances};
pub use placer::{PlacementPlan, Preemption, RunningState, compute_placement, compute_placement_with_preemption};
pub use scorer::{NodeResources, NodeScore, PlacementRequirements, ScoringWeights, rank_nodes, score_node};
