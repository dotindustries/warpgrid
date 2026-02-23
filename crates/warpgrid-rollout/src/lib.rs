//! WarpGrid rolling updates — batch rollout, canary, health gates.
//!
//! This crate provides the rollout state machine for deploying new
//! versions of workloads. It supports rolling updates (batch-by-batch
//! with health checks), canary deployments (traffic-split observation),
//! and blue-green switches.
//!
//! # Components
//!
//! - **`strategy`** — Rollout strategy configuration (Rolling, Canary, BlueGreen)
//! - **`controller`** — Rollout state machine (advance, pause, rollback)

pub mod controller;
pub mod strategy;

pub use controller::{BatchAction, HealthMetrics, Rollout, RolloutPhase};
pub use strategy::{CanaryConfig, RollingConfig, RolloutStrategy};
