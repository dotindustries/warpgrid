//! Integration tests for warpgrid-autoscale.
//!
//! These tests exercise the autoscaler's decision logic end-to-end using
//! an in-memory state store. They verify scaling across multiple deployments,
//! scale-to-zero semantics, cooldown prevention, and metric-specific scaling.

use std::collections::HashMap;

use warpgrid_autoscale::{Autoscaler, ScaleDecision};
use warpgrid_state::{
    DeploymentSpec, InstanceConstraints, MetricsSnapshot, ResourceLimits, ScalingConfig,
    ShimsEnabled, StateStore, TriggerConfig,
};

// ── Helpers ──────────────────────────────────────────────────────

fn make_spec(id: &str, metric: &str, target: f64, min: u32, max: u32) -> DeploymentSpec {
    DeploymentSpec {
        id: id.to_string(),
        namespace: "default".to_string(),
        name: id.split('/').last().unwrap_or(id).to_string(),
        source: "file://test.wasm".to_string(),
        trigger: TriggerConfig::Http { port: Some(8080) },
        instances: InstanceConstraints { min, max },
        resources: ResourceLimits {
            memory_bytes: 64 * 1024 * 1024,
            cpu_weight: 100,
        },
        scaling: Some(ScalingConfig {
            metric: metric.to_string(),
            target_value: target,
            scale_up_window: "0s".to_string(),
            scale_down_window: "0s".to_string(),
        }),
        health: None,
        shims: ShimsEnabled::default(),
        env: HashMap::new(),
        created_at: 1000,
        updated_at: 1000,
    }
}

fn make_spec_with_cooldown(
    id: &str,
    metric: &str,
    target: f64,
    min: u32,
    max: u32,
    scale_up_window: &str,
    scale_down_window: &str,
) -> DeploymentSpec {
    let mut spec = make_spec(id, metric, target, min, max);
    if let Some(ref mut scaling) = spec.scaling {
        scaling.scale_up_window = scale_up_window.to_string();
        scaling.scale_down_window = scale_down_window.to_string();
    }
    spec
}

fn make_snapshot(deployment_id: &str, rps: f64, active_instances: u32) -> MetricsSnapshot {
    MetricsSnapshot {
        deployment_id: deployment_id.to_string(),
        epoch: 1000,
        rps,
        latency_p50_ms: 5.0,
        latency_p99_ms: 50.0,
        error_rate: 0.01,
        total_memory_bytes: 64 * 1024 * 1024,
        active_instances,
    }
}

// ── 1. Mixed deployment scale decisions ─────────────────────────

#[test]
fn mixed_deployment_scale_decisions() {
    let state = StateStore::open_in_memory().unwrap();
    let mut scaler = Autoscaler::new(state);

    // Deployment A: RPS way above target => scale up.
    let spec_a = make_spec("default/api", "rps", 100.0, 1, 20);
    let snap_a = make_snapshot("default/api", 500.0, 3);
    let decision_a = scaler.evaluate(&spec_a, &snap_a);
    assert!(
        matches!(decision_a, ScaleDecision::ScaleTo(n) if n > 3),
        "deployment A should scale up, got: {decision_a:?}"
    );

    // Deployment B: RPS well below target => scale down.
    let spec_b = make_spec("default/worker", "rps", 100.0, 1, 20);
    let snap_b = make_snapshot("default/worker", 10.0, 6);
    let decision_b = scaler.evaluate(&spec_b, &snap_b);
    assert!(
        matches!(decision_b, ScaleDecision::ScaleTo(n) if n < 6),
        "deployment B should scale down, got: {decision_b:?}"
    );

    // Deployment C: RPS near target => no change.
    let spec_c = make_spec("default/cache", "rps", 100.0, 1, 20);
    let snap_c = make_snapshot("default/cache", 95.0, 4);
    let decision_c = scaler.evaluate(&spec_c, &snap_c);
    assert_eq!(
        decision_c,
        ScaleDecision::NoChange,
        "deployment C should remain unchanged"
    );
}

#[tokio::test]
async fn mixed_deployment_evaluate_all_via_state_store() {
    let state = StateStore::open_in_memory().unwrap();

    // Register two deployments with different scaling needs.
    let spec_up = make_spec("default/api", "rps", 100.0, 1, 20);
    state.put_deployment(&spec_up).unwrap();
    state
        .put_metrics(&make_snapshot("default/api", 300.0, 2))
        .unwrap();

    let spec_down = make_spec("default/worker", "rps", 100.0, 1, 20);
    state.put_deployment(&spec_down).unwrap();
    state
        .put_metrics(&make_snapshot("default/worker", 10.0, 8))
        .unwrap();

    let mut scaler = Autoscaler::new(state);
    let decisions = scaler.evaluate_all().await.unwrap();

    assert_eq!(decisions.len(), 2);

    let api_decision = decisions
        .iter()
        .find(|(id, _)| id == "default/api")
        .map(|(_, d)| d);
    let worker_decision = decisions
        .iter()
        .find(|(id, _)| id == "default/worker")
        .map(|(_, d)| d);

    assert!(
        matches!(api_decision, Some(ScaleDecision::ScaleTo(n)) if *n > 2),
        "api should scale up, got: {api_decision:?}"
    );
    assert!(
        matches!(worker_decision, Some(ScaleDecision::ScaleTo(n)) if *n < 8),
        "worker should scale down, got: {worker_decision:?}"
    );
}

// ── 2. Scale-to-zero requires min=0 + zero RPS ─────────────────

#[test]
fn scale_to_zero_when_min_is_zero_and_rps_is_zero() {
    let state = StateStore::open_in_memory().unwrap();
    let mut scaler = Autoscaler::new(state);

    let spec = make_spec("default/idle-svc", "rps", 100.0, 0, 10);
    let snap = make_snapshot("default/idle-svc", 0.0, 3);

    let decision = scaler.evaluate(&spec, &snap);
    assert_eq!(
        decision,
        ScaleDecision::ScaleTo(0),
        "should scale to zero when min=0 and RPS=0"
    );
}

#[test]
fn no_scale_to_zero_when_min_is_positive() {
    let state = StateStore::open_in_memory().unwrap();
    let mut scaler = Autoscaler::new(state);

    let spec = make_spec("default/always-on", "rps", 100.0, 1, 10);
    let snap = make_snapshot("default/always-on", 0.0, 3);

    let decision = scaler.evaluate(&spec, &snap);
    // RPS=0 with min=1 should scale down but not to zero.
    assert!(
        matches!(decision, ScaleDecision::ScaleTo(n) if n >= 1),
        "should not scale to zero when min >= 1, got: {decision:?}"
    );
}

#[test]
fn no_scale_to_zero_when_rps_nonzero() {
    let state = StateStore::open_in_memory().unwrap();
    let mut scaler = Autoscaler::new(state);

    let spec = make_spec("default/low-traffic", "rps", 100.0, 0, 10);
    let snap = make_snapshot("default/low-traffic", 5.0, 3);

    let decision = scaler.evaluate(&spec, &snap);
    // RPS=5 is non-zero so the scale-to-zero path should not activate,
    // but since 5.0 < 100.0 * 0.5, it should scale down (not to zero).
    assert!(
        !matches!(decision, ScaleDecision::ScaleTo(0)),
        "should not scale to zero when RPS > 0, got: {decision:?}"
    );
}

#[test]
fn scale_to_zero_not_triggered_for_non_rps_metric() {
    let state = StateStore::open_in_memory().unwrap();
    let mut scaler = Autoscaler::new(state);

    // Latency-based scaling with min=0. Even though current value is low,
    // the scale-to-zero code path only checks RPS == 0.
    let spec = make_spec("default/latency-svc", "latency_p99", 50.0, 0, 10);
    let mut snap = make_snapshot("default/latency-svc", 0.0, 3);
    snap.latency_p99_ms = 10.0; // Well below target.

    let decision = scaler.evaluate(&spec, &snap);
    // The latency metric (10.0) is < 50.0 * 0.5, so it will scale down,
    // but not necessarily to zero. The scale-to-zero check only applies to "rps" metric.
    assert!(
        matches!(decision, ScaleDecision::ScaleTo(n) if n < 3),
        "latency metric should scale down but not via scale-to-zero path, got: {decision:?}"
    );
}

// ── 3. Cooldown prevents oscillation ────────────────────────────

#[test]
fn cooldown_prevents_immediate_re_scale_up() {
    let state = StateStore::open_in_memory().unwrap();
    let mut scaler = Autoscaler::new(state);

    // Long cooldown window: 1 hour.
    let spec = make_spec_with_cooldown(
        "default/cooldown-up",
        "rps",
        100.0,
        1,
        20,
        "3600s", // 1 hour scale-up cooldown
        "0s",
    );

    let snap = make_snapshot("default/cooldown-up", 500.0, 2);

    // First evaluation: should scale up.
    let decision1 = scaler.evaluate(&spec, &snap);
    assert!(
        matches!(decision1, ScaleDecision::ScaleTo(n) if n > 2),
        "first evaluation should scale up, got: {decision1:?}"
    );

    // Second evaluation immediately after: cooldown should prevent another scale-up.
    let decision2 = scaler.evaluate(&spec, &snap);
    assert_eq!(
        decision2,
        ScaleDecision::NoChange,
        "cooldown should prevent immediate re-scale-up"
    );
}

#[test]
fn cooldown_prevents_immediate_re_scale_down() {
    let state = StateStore::open_in_memory().unwrap();
    let mut scaler = Autoscaler::new(state);

    let spec = make_spec_with_cooldown(
        "default/cooldown-down",
        "rps",
        100.0,
        1,
        20,
        "0s",
        "3600s", // 1 hour scale-down cooldown
    );

    let snap = make_snapshot("default/cooldown-down", 10.0, 6);

    // First evaluation: should scale down.
    let decision1 = scaler.evaluate(&spec, &snap);
    assert!(
        matches!(decision1, ScaleDecision::ScaleTo(n) if n < 6),
        "first evaluation should scale down, got: {decision1:?}"
    );

    // Second evaluation immediately after: cooldown should prevent another scale-down.
    let decision2 = scaler.evaluate(&spec, &snap);
    assert_eq!(
        decision2,
        ScaleDecision::NoChange,
        "cooldown should prevent immediate re-scale-down"
    );
}

// ── 4. Latency-based scaling ────────────────────────────────────

#[test]
fn latency_based_scale_up_on_high_p99() {
    let state = StateStore::open_in_memory().unwrap();
    let mut scaler = Autoscaler::new(state);

    let spec = make_spec("default/latency-api", "latency_p99", 50.0, 1, 20);
    let mut snap = make_snapshot("default/latency-api", 100.0, 3);
    snap.latency_p99_ms = 120.0; // Well above 50.0 * 1.1 = 55.0 threshold.

    let decision = scaler.evaluate(&spec, &snap);
    assert!(
        matches!(decision, ScaleDecision::ScaleTo(n) if n > 3),
        "high p99 latency should trigger scale-up, got: {decision:?}"
    );
}

#[test]
fn latency_based_scale_down_on_low_p99() {
    let state = StateStore::open_in_memory().unwrap();
    let mut scaler = Autoscaler::new(state);

    let spec = make_spec("default/latency-api", "latency_p99", 100.0, 1, 20);
    let mut snap = make_snapshot("default/latency-api", 50.0, 6);
    snap.latency_p99_ms = 20.0; // Below 100.0 * 0.5 = 50.0 threshold.

    let decision = scaler.evaluate(&spec, &snap);
    assert!(
        matches!(decision, ScaleDecision::ScaleTo(n) if n < 6),
        "low p99 latency should trigger scale-down, got: {decision:?}"
    );
}

#[test]
fn latency_based_no_change_when_near_target() {
    let state = StateStore::open_in_memory().unwrap();
    let mut scaler = Autoscaler::new(state);

    let spec = make_spec("default/latency-api", "latency_p99", 50.0, 1, 20);
    let mut snap = make_snapshot("default/latency-api", 100.0, 3);
    snap.latency_p99_ms = 48.0; // Within the 50% < x < 110% range.

    let decision = scaler.evaluate(&spec, &snap);
    assert_eq!(
        decision,
        ScaleDecision::NoChange,
        "latency near target should not trigger scaling"
    );
}

// ── 5. Memory-based scaling ─────────────────────────────────────

#[test]
fn memory_based_scale_up_on_high_usage() {
    let state = StateStore::open_in_memory().unwrap();
    let mut scaler = Autoscaler::new(state);

    // Target: 512 MB. Current: 1200 MB (above 512 * 1.1 = 563.2 MB).
    let target_bytes = 512.0 * 1024.0 * 1024.0;
    let spec = make_spec("default/mem-app", "memory", target_bytes, 1, 20);
    let mut snap = make_snapshot("default/mem-app", 50.0, 2);
    snap.total_memory_bytes = (1200.0 * 1024.0 * 1024.0) as u64;

    let decision = scaler.evaluate(&spec, &snap);
    assert!(
        matches!(decision, ScaleDecision::ScaleTo(n) if n > 2),
        "high memory usage should trigger scale-up, got: {decision:?}"
    );
}

#[test]
fn memory_based_scale_down_on_low_usage() {
    let state = StateStore::open_in_memory().unwrap();
    let mut scaler = Autoscaler::new(state);

    // Target: 512 MB. Current: 100 MB (below 512 * 0.5 = 256 MB).
    let target_bytes = 512.0 * 1024.0 * 1024.0;
    let spec = make_spec("default/mem-app", "memory", target_bytes, 1, 20);
    let mut snap = make_snapshot("default/mem-app", 50.0, 5);
    snap.total_memory_bytes = (100.0 * 1024.0 * 1024.0) as u64;

    let decision = scaler.evaluate(&spec, &snap);
    assert!(
        matches!(decision, ScaleDecision::ScaleTo(n) if n < 5),
        "low memory usage should trigger scale-down, got: {decision:?}"
    );
}

#[test]
fn memory_based_no_change_when_within_range() {
    let state = StateStore::open_in_memory().unwrap();
    let mut scaler = Autoscaler::new(state);

    // Target: 512 MB. Current: 400 MB (between 256 MB and 563.2 MB).
    let target_bytes = 512.0 * 1024.0 * 1024.0;
    let spec = make_spec("default/mem-app", "memory", target_bytes, 1, 20);
    let mut snap = make_snapshot("default/mem-app", 50.0, 3);
    snap.total_memory_bytes = (400.0 * 1024.0 * 1024.0) as u64;

    let decision = scaler.evaluate(&spec, &snap);
    assert_eq!(
        decision,
        ScaleDecision::NoChange,
        "memory within range should not trigger scaling"
    );
}

#[test]
fn memory_based_scale_up_clamped_to_max() {
    let state = StateStore::open_in_memory().unwrap();
    let mut scaler = Autoscaler::new(state);

    // Extreme memory usage: wants way more instances than max allows.
    let target_bytes = 100.0 * 1024.0 * 1024.0;
    let spec = make_spec("default/mem-burst", "memory", target_bytes, 1, 5);
    let mut snap = make_snapshot("default/mem-burst", 50.0, 1);
    snap.total_memory_bytes = (5000.0 * 1024.0 * 1024.0) as u64; // 50x target.

    let decision = scaler.evaluate(&spec, &snap);
    assert_eq!(
        decision,
        ScaleDecision::ScaleTo(5),
        "memory scale-up should be clamped to max=5"
    );
}
