//! Integration tests for the warpgrid-health crate.
//!
//! Tests cover HTTP probes against real servers, health tracker state
//! transitions, monitor lifecycle, exponential backoff, and probe timeouts.

use std::time::Duration;

use warpgrid_health::{HealthMonitor, HealthTracker, ProbeResult};
use warpgrid_state::{HealthConfig, HealthStatus, StateStore};

// ── Helpers ──────────────────────────────────────────────────────────

fn in_memory_state() -> StateStore {
    StateStore::open_in_memory().unwrap()
}

fn test_health_config() -> HealthConfig {
    HealthConfig {
        endpoint: "/healthz".to_string(),
        interval: "1s".to_string(),
        timeout: "1s".to_string(),
        unhealthy_threshold: 3,
    }
}

// ── 1. Health probe against real HTTP server ─────────────────────────

#[tokio::test]
async fn probe_healthy_server_returns_healthy() {
    use warpgrid_health::checker::http_probe;

    // Start a minimal HTTP server on an ephemeral port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Spawn a tiny handler that always returns 200.
    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let io = hyper_util::rt::TokioIo::new(stream);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(|_req| async {
                    Ok::<_, hyper::Error>(hyper::Response::new(http_body_util::Full::new(
                        bytes::Bytes::from("ok"),
                    )))
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await;
            });
        }
    });

    let result = http_probe(&addr.to_string(), "/healthz", Duration::from_secs(2)).await;
    assert_eq!(result, ProbeResult::Healthy);
}

#[tokio::test]
async fn probe_non_2xx_returns_unhealthy() {
    use warpgrid_health::checker::http_probe;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let io = hyper_util::rt::TokioIo::new(stream);
            tokio::spawn(async move {
                let service = hyper::service::service_fn(|_req| async {
                    Ok::<_, hyper::Error>(
                        hyper::Response::builder()
                            .status(503)
                            .body(http_body_util::Full::new(bytes::Bytes::from("unavailable")))
                            .unwrap(),
                    )
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await;
            });
        }
    });

    let result = http_probe(&addr.to_string(), "/healthz", Duration::from_secs(2)).await;
    assert_eq!(result, ProbeResult::Unhealthy);
}

#[tokio::test]
async fn probe_closed_port_returns_failed() {
    use warpgrid_health::checker::http_probe;

    // Port 1 is almost certainly not listening.
    let result = http_probe("127.0.0.1:1", "/healthz", Duration::from_millis(100)).await;
    assert_eq!(result, ProbeResult::Failed);
}

// ── 2. Health tracker state transitions ──────────────────────────────

#[test]
fn tracker_transitions_unknown_to_healthy_to_unhealthy_to_recovery() {
    let mut tracker = HealthTracker::with_thresholds(3, 1, Duration::from_secs(1));

    // Starts Unknown.
    assert_eq!(tracker.status(), HealthStatus::Unknown);

    // First success: Unknown -> Healthy.
    let status = tracker.record(ProbeResult::Healthy);
    assert_eq!(status, HealthStatus::Healthy);

    // Two failures: still Healthy (threshold is 3).
    tracker.record(ProbeResult::Unhealthy);
    tracker.record(ProbeResult::Unhealthy);
    assert_eq!(tracker.status(), HealthStatus::Healthy);
    assert_eq!(tracker.consecutive_failures(), 2);

    // Third failure: Healthy -> Unhealthy.
    let status = tracker.record(ProbeResult::Unhealthy);
    assert_eq!(status, HealthStatus::Unhealthy);
    assert!(tracker.needs_replacement());

    // Single success: Unhealthy -> Healthy (recovery).
    let status = tracker.record(ProbeResult::Healthy);
    assert_eq!(status, HealthStatus::Healthy);
    assert!(!tracker.needs_replacement());
    assert_eq!(tracker.consecutive_failures(), 0);
}

#[test]
fn tracker_with_higher_recovery_threshold() {
    let mut tracker = HealthTracker::with_thresholds(2, 3, Duration::from_secs(1));

    // Drive to unhealthy.
    tracker.record(ProbeResult::Unhealthy);
    tracker.record(ProbeResult::Unhealthy);
    assert_eq!(tracker.status(), HealthStatus::Unhealthy);

    // Need 3 successes to recover.
    tracker.record(ProbeResult::Healthy);
    assert_eq!(tracker.status(), HealthStatus::Unhealthy);
    tracker.record(ProbeResult::Healthy);
    assert_eq!(tracker.status(), HealthStatus::Unhealthy);
    tracker.record(ProbeResult::Healthy);
    assert_eq!(tracker.status(), HealthStatus::Healthy);
}

// ── 3. Monitor lifecycle ─────────────────────────────────────────────

#[tokio::test]
async fn monitor_start_stop_lifecycle() {
    let state = in_memory_state();
    let monitor = HealthMonitor::new(state);

    assert!(monitor.active_monitors().await.is_empty());
    assert!(!monitor.is_monitoring("deploy-1").await);

    // Start monitor (will fail to connect, but lifecycle still works).
    monitor
        .start_monitor("deploy-1", &test_health_config(), "127.0.0.1:0")
        .await;
    assert!(monitor.is_monitoring("deploy-1").await);
    assert_eq!(monitor.active_monitors().await.len(), 1);

    // Stop monitor.
    monitor.stop_monitor("deploy-1").await;
    assert!(!monitor.is_monitoring("deploy-1").await);
    assert!(monitor.active_monitors().await.is_empty());
}

#[tokio::test]
async fn monitor_stop_all_clears_all_monitors() {
    let state = in_memory_state();
    let monitor = HealthMonitor::new(state);

    monitor
        .start_monitor("deploy-1", &test_health_config(), "127.0.0.1:0")
        .await;
    monitor
        .start_monitor("deploy-2", &test_health_config(), "127.0.0.1:0")
        .await;
    monitor
        .start_monitor("deploy-3", &test_health_config(), "127.0.0.1:0")
        .await;
    assert_eq!(monitor.active_monitors().await.len(), 3);

    monitor.stop_all().await;
    assert!(monitor.active_monitors().await.is_empty());
}

#[tokio::test]
async fn monitor_replace_existing_monitor() {
    let state = in_memory_state();
    let monitor = HealthMonitor::new(state);

    // Start a monitor.
    monitor
        .start_monitor("deploy-1", &test_health_config(), "127.0.0.1:0")
        .await;

    // Starting again for the same deployment replaces the old one.
    let new_config = HealthConfig {
        endpoint: "/ready".to_string(),
        interval: "2s".to_string(),
        timeout: "1s".to_string(),
        unhealthy_threshold: 5,
    };
    monitor
        .start_monitor("deploy-1", &new_config, "127.0.0.1:1")
        .await;

    // Should still be just one monitor.
    assert_eq!(monitor.active_monitors().await.len(), 1);
    assert!(monitor.is_monitoring("deploy-1").await);

    monitor.stop_all().await;
}

// ── 4. Exponential backoff caps at 60s ───────────────────────────────

#[test]
fn backoff_doubles_on_each_failure_and_caps_at_60s() {
    let mut tracker = HealthTracker::with_thresholds(100, 1, Duration::from_secs(1));

    assert_eq!(tracker.next_interval(), Duration::from_secs(1));

    // Expected backoff sequence: 1 -> 2 -> 4 -> 8 -> 16 -> 32 -> 60 (capped)
    let expected_backoffs = [2, 4, 8, 16, 32, 60, 60, 60];
    for &expected in &expected_backoffs {
        tracker.record(ProbeResult::Unhealthy);
        assert_eq!(
            tracker.next_interval(),
            Duration::from_secs(expected),
            "expected backoff of {expected}s"
        );
    }
}

#[test]
fn backoff_resets_on_successful_probe() {
    let mut tracker = HealthTracker::with_thresholds(10, 1, Duration::from_secs(1));

    // Accumulate some backoff.
    tracker.record(ProbeResult::Unhealthy);
    tracker.record(ProbeResult::Unhealthy);
    tracker.record(ProbeResult::Unhealthy);
    assert_eq!(tracker.next_interval(), Duration::from_secs(8));

    // Single success resets backoff.
    tracker.record(ProbeResult::Healthy);
    assert_eq!(tracker.next_interval(), Duration::from_secs(1));
}

// ── 5. Probe timeout counts as failure ───────────────────────────────

#[tokio::test]
async fn probe_timeout_counts_as_failure() {
    use warpgrid_health::checker::http_probe;

    // Start a server that accepts but never responds.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            // Accept the connection but never respond — just hold it open.
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(30)).await;
                drop(stream);
            });
        }
    });

    // Use a very short timeout so the test completes quickly.
    let result = http_probe(&addr.to_string(), "/healthz", Duration::from_millis(50)).await;
    assert_eq!(
        result,
        ProbeResult::Failed,
        "timeout should count as failure"
    );
}

#[test]
fn failed_probe_increments_failure_count_like_unhealthy() {
    let mut tracker = HealthTracker::with_thresholds(3, 1, Duration::from_secs(1));
    tracker.record(ProbeResult::Healthy);

    // Three Failed probes should trigger Unhealthy, same as Unhealthy probes.
    tracker.record(ProbeResult::Failed);
    tracker.record(ProbeResult::Failed);
    tracker.record(ProbeResult::Failed);

    assert_eq!(tracker.status(), HealthStatus::Unhealthy);
    assert!(tracker.needs_replacement());
}
