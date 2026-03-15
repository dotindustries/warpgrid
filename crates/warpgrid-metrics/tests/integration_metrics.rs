//! Integration tests for the warpgrid-metrics crate.
//!
//! Tests cover collector lifecycle, snapshot resets, Prometheus exposition
//! format, percentile accuracy, and resource usage refresh.

use std::collections::HashMap;
use std::time::Duration;

use warpgrid_metrics::{MetricsCollector, render_prometheus};
use warpgrid_state::*;

// ── Helpers ──────────────────────────────────────────────────────────

fn in_memory_state() -> StateStore {
    StateStore::open_in_memory().unwrap()
}

fn make_deployment(id: &str) -> DeploymentSpec {
    DeploymentSpec {
        id: id.to_string(),
        namespace: "default".to_string(),
        name: id.to_string(),
        source: "file://test.wasm".to_string(),
        trigger: TriggerConfig::Http { port: None },
        instances: InstanceConstraints { min: 1, max: 3 },
        resources: ResourceLimits {
            memory_bytes: 64 * 1024 * 1024,
            cpu_weight: 100,
        },
        scaling: None,
        health: None,
        shims: ShimsEnabled::default(),
        env: HashMap::new(),
        created_at: 0,
        updated_at: 0,
    }
}

fn make_instance(
    id: &str,
    deployment_id: &str,
    status: InstanceStatus,
    memory_bytes: u64,
) -> InstanceState {
    InstanceState {
        id: id.to_string(),
        deployment_id: deployment_id.to_string(),
        node_id: "standalone".to_string(),
        status,
        health: HealthStatus::Unknown,
        restart_count: 0,
        memory_bytes,
        started_at: 0,
        updated_at: 0,
    }
}

// ── 1. Collector register/record/snapshot lifecycle ──────────────────

#[tokio::test]
async fn collector_full_lifecycle() {
    let state = in_memory_state();
    let collector = MetricsCollector::new(state.clone(), Duration::from_secs(60));

    // Register a deployment.
    collector.register("deploy-1").await;
    assert_eq!(collector.registered_deployments().await.len(), 1);

    // Record several requests with varying latencies.
    collector.record_request("deploy-1", 5_000, false).await;
    collector.record_request("deploy-1", 10_000, false).await;
    collector.record_request("deploy-1", 50_000, true).await;

    assert_eq!(collector.current_request_count("deploy-1").await, 3);

    // Update resource usage.
    collector
        .update_resource_usage("deploy-1", 128_000_000, 2)
        .await;

    // Take snapshot — persists to state store and returns snapshots.
    let snapshots = collector.snapshot().await.unwrap();
    assert_eq!(snapshots.len(), 1);

    let snap = &snapshots[0];
    assert_eq!(snap.deployment_id, "deploy-1");
    assert!(snap.rps > 0.0, "rps should be positive");
    assert!(
        snap.error_rate > 0.0 && snap.error_rate < 1.0,
        "error rate should be between 0 and 1"
    );
    assert_eq!(snap.total_memory_bytes, 128_000_000);
    assert_eq!(snap.active_instances, 2);

    // Verify data persisted in the state store.
    let stored = state.list_metrics_for_deployment("deploy-1", 10).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].deployment_id, "deploy-1");

    // Unregister.
    collector.unregister("deploy-1").await;
    assert!(collector.registered_deployments().await.is_empty());
}

#[tokio::test]
async fn recording_to_unregistered_deployment_is_noop() {
    let collector = MetricsCollector::new(in_memory_state(), Duration::from_secs(60));

    // Should not panic or error.
    collector.record_request("unknown", 5_000, false).await;
    assert_eq!(collector.current_request_count("unknown").await, 0);
}

#[tokio::test]
async fn multiple_deployments_tracked_independently() {
    let collector = MetricsCollector::new(in_memory_state(), Duration::from_secs(60));

    collector.register("svc-a").await;
    collector.register("svc-b").await;

    collector.record_request("svc-a", 1_000, false).await;
    collector.record_request("svc-a", 2_000, false).await;
    collector.record_request("svc-b", 5_000, true).await;

    assert_eq!(collector.current_request_count("svc-a").await, 2);
    assert_eq!(collector.current_request_count("svc-b").await, 1);

    let snapshots = collector.snapshot().await.unwrap();
    assert_eq!(snapshots.len(), 2);
}

// ── 2. Snapshot resets counters between windows ──────────────────────

#[tokio::test]
async fn snapshot_resets_counters() {
    let collector = MetricsCollector::new(in_memory_state(), Duration::from_secs(60));
    collector.register("deploy-1").await;

    // Record some requests.
    collector.record_request("deploy-1", 5_000, false).await;
    collector.record_request("deploy-1", 10_000, false).await;
    assert_eq!(collector.current_request_count("deploy-1").await, 2);

    // First snapshot.
    let snap1 = collector.snapshot().await.unwrap();
    assert_eq!(snap1.len(), 1);

    // After snapshot, counters should be reset.
    assert_eq!(collector.current_request_count("deploy-1").await, 0);

    // Second snapshot with no new requests should show zero RPS.
    let snap2 = collector.snapshot().await.unwrap();
    assert_eq!(snap2[0].rps, 0.0);
}

#[tokio::test]
async fn snapshot_resets_latency_samples() {
    let collector = MetricsCollector::new(in_memory_state(), Duration::from_secs(60));
    collector.register("deploy-1").await;

    // Record high-latency requests.
    collector.record_request("deploy-1", 100_000, false).await;
    let snap1 = collector.snapshot().await.unwrap();
    assert!(snap1[0].latency_p50_ms > 0.0);

    // After reset, no latency data.
    let snap2 = collector.snapshot().await.unwrap();
    assert_eq!(snap2[0].latency_p50_ms, 0.0);
    assert_eq!(snap2[0].latency_p99_ms, 0.0);
}

// ── 3. Prometheus exposition multi-deployment format ─────────────────

#[test]
fn prometheus_renders_multiple_deployments() {
    let snapshots = vec![
        MetricsSnapshot {
            deployment_id: "prod/api".to_string(),
            epoch: 1000,
            rps: 150.5,
            latency_p50_ms: 5.2,
            latency_p99_ms: 45.8,
            error_rate: 0.012,
            total_memory_bytes: 256_000_000,
            active_instances: 4,
        },
        MetricsSnapshot {
            deployment_id: "prod/worker".to_string(),
            epoch: 1000,
            rps: 42.0,
            latency_p50_ms: 1.5,
            latency_p99_ms: 10.0,
            error_rate: 0.0,
            total_memory_bytes: 128_000_000,
            active_instances: 2,
        },
    ];

    let output = render_prometheus(&snapshots);

    // Verify HELP and TYPE headers for all metrics.
    assert!(output.contains("# HELP warpgrid_requests_per_second"));
    assert!(output.contains("# TYPE warpgrid_requests_per_second gauge"));
    assert!(output.contains("# HELP warpgrid_latency_p50_ms"));
    assert!(output.contains("# HELP warpgrid_latency_p99_ms"));
    assert!(output.contains("# HELP warpgrid_error_rate"));
    assert!(output.contains("# HELP warpgrid_memory_bytes"));
    assert!(output.contains("# HELP warpgrid_active_instances"));

    // Verify both deployments appear in each metric.
    assert!(output.contains("warpgrid_requests_per_second{deployment=\"prod/api\"} 150.50"));
    assert!(output.contains("warpgrid_requests_per_second{deployment=\"prod/worker\"} 42.00"));
    assert!(output.contains("warpgrid_latency_p50_ms{deployment=\"prod/api\"} 5.20"));
    assert!(output.contains("warpgrid_latency_p99_ms{deployment=\"prod/worker\"} 10.00"));
    assert!(output.contains("warpgrid_error_rate{deployment=\"prod/api\"} 0.0120"));
    assert!(output.contains("warpgrid_error_rate{deployment=\"prod/worker\"} 0.0000"));
    assert!(output.contains("warpgrid_memory_bytes{deployment=\"prod/api\"} 256000000"));
    assert!(output.contains("warpgrid_active_instances{deployment=\"prod/worker\"} 2"));
}

#[test]
fn prometheus_empty_snapshots_still_has_headers() {
    let output = render_prometheus(&[]);

    assert!(output.contains("# HELP warpgrid_requests_per_second"));
    assert!(output.contains("# TYPE warpgrid_requests_per_second gauge"));
    // No data lines with deployment labels.
    for line in output.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        panic!("unexpected data line in empty snapshot: {line}");
    }
}

#[test]
fn prometheus_format_has_valid_metric_lines() {
    let snapshots = vec![MetricsSnapshot {
        deployment_id: "test/svc".to_string(),
        epoch: 1000,
        rps: 10.0,
        latency_p50_ms: 1.0,
        latency_p99_ms: 5.0,
        error_rate: 0.05,
        total_memory_bytes: 1024,
        active_instances: 1,
    }];

    let output = render_prometheus(&snapshots);

    for line in output.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Every data line must have labels in curly braces.
        assert!(
            line.contains('{') && line.contains('}'),
            "data line missing labels: {line}"
        );
    }
}

// ── 4. Percentile computation accuracy ───────────────────────────────

#[tokio::test]
async fn percentile_computation_across_distribution() {
    let collector = MetricsCollector::new(in_memory_state(), Duration::from_secs(60));
    collector.register("deploy-1").await;

    // Record 100 requests with latencies from 1ms to 100ms (in microseconds).
    for i in 1..=100 {
        collector
            .record_request("deploy-1", i * 1000, false)
            .await;
    }

    let snapshots = collector.snapshot().await.unwrap();
    let snap = &snapshots[0];

    // P50 should be around 50ms.
    assert!(
        snap.latency_p50_ms >= 49.0 && snap.latency_p50_ms <= 51.0,
        "p50 was {}",
        snap.latency_p50_ms
    );
    // P99 should be around 99ms.
    assert!(
        snap.latency_p99_ms >= 98.0 && snap.latency_p99_ms <= 100.0,
        "p99 was {}",
        snap.latency_p99_ms
    );
}

#[tokio::test]
async fn percentile_single_sample() {
    let collector = MetricsCollector::new(in_memory_state(), Duration::from_secs(60));
    collector.register("deploy-1").await;

    collector.record_request("deploy-1", 5_000, false).await;

    let snapshots = collector.snapshot().await.unwrap();
    let snap = &snapshots[0];

    // Single sample: both percentiles should equal the same value.
    assert_eq!(snap.latency_p50_ms, 5.0);
    assert_eq!(snap.latency_p99_ms, 5.0);
}

#[tokio::test]
async fn percentile_no_samples_returns_zero() {
    let collector = MetricsCollector::new(in_memory_state(), Duration::from_secs(60));
    collector.register("deploy-1").await;

    // No requests recorded.
    let snapshots = collector.snapshot().await.unwrap();
    let snap = &snapshots[0];

    assert_eq!(snap.latency_p50_ms, 0.0);
    assert_eq!(snap.latency_p99_ms, 0.0);
}

// ── 5. Resource usage refresh ────────────────────────────────────────

#[tokio::test]
async fn refresh_resource_usage_from_instance_state() {
    let state = in_memory_state();
    let collector = MetricsCollector::new(state.clone(), Duration::from_secs(60));
    collector.register("deploy-1").await;

    // Add instances with varying statuses.
    state
        .put_instance(&make_instance(
            "i-1",
            "deploy-1",
            InstanceStatus::Running,
            32_000_000,
        ))
        .unwrap();
    state
        .put_instance(&make_instance(
            "i-2",
            "deploy-1",
            InstanceStatus::Running,
            48_000_000,
        ))
        .unwrap();
    state
        .put_instance(&make_instance(
            "i-3",
            "deploy-1",
            InstanceStatus::Stopped,
            16_000_000,
        ))
        .unwrap();

    // Refresh should pick up instance data.
    collector.refresh_resource_usage().await.unwrap();

    // Take snapshot to read the computed values.
    let snapshots = collector.snapshot().await.unwrap();
    let snap = &snapshots[0];

    // Only Running instances count as active.
    assert_eq!(snap.active_instances, 2);
    // Total memory sums all instances (running + stopped).
    assert_eq!(
        snap.total_memory_bytes,
        32_000_000 + 48_000_000 + 16_000_000
    );
}

#[tokio::test]
async fn auto_discover_finds_new_deployments_from_state() {
    let state = in_memory_state();
    let collector = MetricsCollector::new(state.clone(), Duration::from_secs(60));

    // No deployments initially.
    collector.auto_discover().await.unwrap();
    assert!(collector.registered_deployments().await.is_empty());

    // Add a deployment to the state store.
    state.put_deployment(&make_deployment("svc-a")).unwrap();

    // Auto-discover should pick it up.
    collector.auto_discover().await.unwrap();
    let registered = collector.registered_deployments().await;
    assert_eq!(registered.len(), 1);
    assert!(registered.contains(&"svc-a".to_string()));

    // Calling again should not duplicate.
    collector.auto_discover().await.unwrap();
    assert_eq!(collector.registered_deployments().await.len(), 1);
}

#[tokio::test]
async fn auto_discover_preserves_existing_request_counts() {
    let state = in_memory_state();
    let collector = MetricsCollector::new(state.clone(), Duration::from_secs(60));

    // Manually register and record a request.
    collector.register("svc-a").await;
    collector.record_request("svc-a", 5_000, false).await;

    // Add the same deployment to the state store.
    state.put_deployment(&make_deployment("svc-a")).unwrap();

    // Auto-discover should not reset the existing metrics.
    collector.auto_discover().await.unwrap();
    assert_eq!(collector.current_request_count("svc-a").await, 1);
}
