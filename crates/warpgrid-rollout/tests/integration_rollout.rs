//! Integration tests for warpgrid-rollout.
//!
//! These tests exercise the rollout state machine end-to-end across
//! all three strategies (Rolling, Canary, BlueGreen). Unlike the unit
//! tests in `controller.rs`, these drive multi-step lifecycles that
//! combine start, advance, pause, resume, and rollback transitions.

use warpgrid_rollout::{
    BatchAction, HealthMetrics, Rollout, RolloutPhase,
    CanaryConfig, RollingConfig, RolloutStrategy,
};

// ── Helpers ──────────────────────────────────────────────────────

fn healthy() -> HealthMetrics {
    HealthMetrics {
        healthy_count: 10,
        total_count: 10,
        error_rate: 0.5,
        p99_latency_ms: 50,
    }
}

fn unhealthy() -> HealthMetrics {
    HealthMetrics {
        healthy_count: 2,
        total_count: 10,
        error_rate: 25.0,
        p99_latency_ms: 5000,
    }
}

// ── 1. Rolling update full lifecycle (multiple batches) ──────────

#[test]
fn rolling_update_full_lifecycle_multiple_batches() {
    // 10 instances, batch size 3 => 4 batches (3+3+3+1).
    let mut rollout = Rollout::new(
        "deploy/web",
        RolloutStrategy::Rolling(RollingConfig {
            batch_size: 3,
            ..Default::default()
        }),
        10,
        "v1.0",
        "v2.0",
    );

    assert_eq!(rollout.phase, RolloutPhase::Pending);
    rollout.start();

    assert_eq!(
        rollout.phase,
        RolloutPhase::RollingBatch {
            current: 1,
            total: 4,
        }
    );

    // Batch 1: instances 0..3
    let action = rollout.advance(&healthy()).unwrap();
    assert_eq!(
        action,
        BatchAction::UpdateBatch {
            start_index: 0,
            count: 3,
        }
    );
    assert_eq!(
        rollout.phase,
        RolloutPhase::RollingBatch {
            current: 2,
            total: 4,
        }
    );

    // Batch 2: instances 3..6
    let action = rollout.advance(&healthy()).unwrap();
    assert_eq!(
        action,
        BatchAction::UpdateBatch {
            start_index: 3,
            count: 3,
        }
    );
    assert_eq!(
        rollout.phase,
        RolloutPhase::RollingBatch {
            current: 3,
            total: 4,
        }
    );

    // Batch 3: instances 6..9
    let action = rollout.advance(&healthy()).unwrap();
    assert_eq!(
        action,
        BatchAction::UpdateBatch {
            start_index: 6,
            count: 3,
        }
    );
    assert_eq!(
        rollout.phase,
        RolloutPhase::RollingBatch {
            current: 4,
            total: 4,
        }
    );

    // Batch 4 (final): instances 9..10
    let action = rollout.advance(&healthy()).unwrap();
    assert_eq!(
        action,
        BatchAction::UpdateBatch {
            start_index: 9,
            count: 1,
        }
    );
    assert_eq!(rollout.phase, RolloutPhase::Completed);

    // After completion, advance returns None.
    assert!(rollout.advance(&healthy()).is_none());
}

// ── 2. Rolling rollback mid-flight ──────────────────────────────

#[test]
fn rolling_rollback_mid_flight() {
    // 6 instances, batch size 2 => 3 batches.
    let mut rollout = Rollout::new(
        "deploy/api",
        RolloutStrategy::Rolling(RollingConfig {
            batch_size: 2,
            ..Default::default()
        }),
        6,
        "v3.0",
        "v4.0",
    );

    rollout.start();

    // Batch 1 succeeds.
    let action = rollout.advance(&healthy()).unwrap();
    assert_eq!(
        action,
        BatchAction::UpdateBatch {
            start_index: 0,
            count: 2,
        }
    );

    // Batch 2 hits unhealthy metrics => rollback.
    let action = rollout.advance(&unhealthy()).unwrap();
    assert_eq!(action, BatchAction::Rollback);

    match &rollout.phase {
        RolloutPhase::RolledBack { reason } => {
            assert!(
                reason.contains("health gate failed"),
                "reason should mention health gate failure, got: {reason}"
            );
            assert!(
                reason.contains("batch 2/3"),
                "reason should identify batch 2/3, got: {reason}"
            );
        }
        other => panic!("expected RolledBack, got: {other:?}"),
    }

    // After rollback, advance returns None.
    assert!(rollout.advance(&healthy()).is_none());
}

// ── 3. Canary promote lifecycle ─────────────────────────────────

#[test]
fn canary_promote_lifecycle() {
    let mut rollout = Rollout::new(
        "deploy/canary-svc",
        RolloutStrategy::Canary(CanaryConfig {
            traffic_percent: 10,
            canary_instances: 1,
            observation_secs: 300,
            error_rate_threshold: 5.0,
            latency_threshold_ms: 500,
        }),
        8,
        "v1.0",
        "v1.1",
    );

    assert_eq!(rollout.phase, RolloutPhase::Pending);
    rollout.start();
    assert_eq!(rollout.phase, RolloutPhase::CanaryObserving);

    // Canary observes healthy metrics => promote.
    let action = rollout.advance(&healthy()).unwrap();
    assert_eq!(action, BatchAction::PromoteCanary);
    assert_eq!(rollout.phase, RolloutPhase::CanaryPromoting);

    // Promotion step: update all instances.
    let action = rollout.advance(&healthy()).unwrap();
    assert_eq!(
        action,
        BatchAction::UpdateBatch {
            start_index: 0,
            count: 8,
        }
    );
    assert_eq!(rollout.phase, RolloutPhase::Completed);

    // Post-completion returns None.
    assert!(rollout.advance(&healthy()).is_none());
}

// ── 4. Canary rollback at boundary error rate ───────────────────

#[test]
fn canary_rollback_at_boundary_error_rate() {
    // Threshold is exactly 2.0%. An error_rate of 2.1% should trigger rollback.
    let mut rollout = Rollout::new(
        "deploy/canary-edge",
        RolloutStrategy::Canary(CanaryConfig {
            error_rate_threshold: 2.0,
            latency_threshold_ms: 1000,
            ..Default::default()
        }),
        5,
        "v1.0",
        "v1.1",
    );

    rollout.start();
    assert_eq!(rollout.phase, RolloutPhase::CanaryObserving);

    // Exactly at threshold: 2.0% should pass (not strictly greater).
    let at_threshold = HealthMetrics {
        error_rate: 2.0,
        ..healthy()
    };
    let action = rollout.advance(&at_threshold).unwrap();
    assert_eq!(
        action,
        BatchAction::PromoteCanary,
        "error_rate equal to threshold should pass"
    );

    // Start a fresh rollout to test just above threshold.
    let mut rollout2 = Rollout::new(
        "deploy/canary-edge-2",
        RolloutStrategy::Canary(CanaryConfig {
            error_rate_threshold: 2.0,
            latency_threshold_ms: 1000,
            ..Default::default()
        }),
        5,
        "v1.0",
        "v1.1",
    );

    rollout2.start();

    let above_threshold = HealthMetrics {
        error_rate: 2.1,
        ..healthy()
    };
    let action = rollout2.advance(&above_threshold).unwrap();
    assert_eq!(
        action,
        BatchAction::Rollback,
        "error_rate above threshold should trigger rollback"
    );

    match &rollout2.phase {
        RolloutPhase::RolledBack { reason } => {
            assert!(
                reason.contains("canary failed"),
                "reason should mention canary failure, got: {reason}"
            );
        }
        other => panic!("expected RolledBack, got: {other:?}"),
    }
}

// ── 5. Blue-green switch and rollback ───────────────────────────

#[test]
fn blue_green_switch_on_healthy() {
    let mut rollout = Rollout::new(
        "deploy/bg-prod",
        RolloutStrategy::BlueGreen,
        10,
        "blue-v1",
        "green-v2",
    );

    rollout.start();
    assert_eq!(rollout.phase, RolloutPhase::HealthGate);

    let action = rollout.advance(&healthy()).unwrap();
    assert_eq!(action, BatchAction::SwitchTraffic);
    assert_eq!(rollout.phase, RolloutPhase::Completed);
}

#[test]
fn blue_green_rollback_on_unhealthy() {
    let mut rollout = Rollout::new(
        "deploy/bg-prod",
        RolloutStrategy::BlueGreen,
        10,
        "blue-v1",
        "green-v2",
    );

    rollout.start();
    assert_eq!(rollout.phase, RolloutPhase::HealthGate);

    let action = rollout.advance(&unhealthy()).unwrap();
    assert_eq!(action, BatchAction::Rollback);

    match &rollout.phase {
        RolloutPhase::RolledBack { reason } => {
            assert!(
                reason.contains("blue-green"),
                "reason should mention blue-green, got: {reason}"
            );
        }
        other => panic!("expected RolledBack, got: {other:?}"),
    }
}

// ── 6. Pause preserves batch position ───────────────────────────

#[test]
fn pause_preserves_batch_position_and_resumes_via_health_gate() {
    // 6 instances, batch size 2 => 3 batches.
    let mut rollout = Rollout::new(
        "deploy/pausable",
        RolloutStrategy::Rolling(RollingConfig {
            batch_size: 2,
            ..Default::default()
        }),
        6,
        "v1",
        "v2",
    );

    rollout.start();

    // Advance batch 1.
    let action = rollout.advance(&healthy()).unwrap();
    assert_eq!(
        action,
        BatchAction::UpdateBatch {
            start_index: 0,
            count: 2,
        }
    );

    // Now at batch 2/3. Pause.
    assert_eq!(
        rollout.phase,
        RolloutPhase::RollingBatch {
            current: 2,
            total: 3,
        }
    );
    rollout.pause();
    assert_eq!(rollout.phase, RolloutPhase::Paused);

    // Advance while paused returns None.
    assert!(rollout.advance(&healthy()).is_none());

    // Resume goes to HealthGate.
    rollout.resume();
    assert_eq!(rollout.phase, RolloutPhase::HealthGate);

    // HealthGate with healthy metrics completes.
    let action = rollout.advance(&healthy()).unwrap();
    assert_eq!(action, BatchAction::SwitchTraffic);
    assert_eq!(rollout.phase, RolloutPhase::Completed);
}

#[test]
fn pause_is_noop_when_completed() {
    let mut rollout = Rollout::new(
        "deploy/done",
        RolloutStrategy::BlueGreen,
        5,
        "v1",
        "v2",
    );

    rollout.start();
    rollout.advance(&healthy());
    assert_eq!(rollout.phase, RolloutPhase::Completed);

    // Pausing a completed rollout is a no-op.
    rollout.pause();
    assert_eq!(rollout.phase, RolloutPhase::Completed);
}

#[test]
fn pause_is_noop_when_rolled_back() {
    let mut rollout = Rollout::new(
        "deploy/failed",
        RolloutStrategy::BlueGreen,
        5,
        "v1",
        "v2",
    );

    rollout.start();
    rollout.advance(&unhealthy());
    assert!(matches!(rollout.phase, RolloutPhase::RolledBack { .. }));

    // Pausing a rolled-back rollout is a no-op.
    rollout.pause();
    assert!(matches!(rollout.phase, RolloutPhase::RolledBack { .. }));
}

// ── 7. Strategy serialization roundtrip ─────────────────────────

#[test]
fn rolling_strategy_serialization_roundtrip() {
    let strategy = RolloutStrategy::Rolling(RollingConfig {
        batch_size: 5,
        batch_interval_secs: 30,
        health_timeout_secs: 60,
        max_unavailable: 2,
    });

    let json = serde_json::to_string(&strategy).unwrap();
    let deserialized: RolloutStrategy = serde_json::from_str(&json).unwrap();

    match deserialized {
        RolloutStrategy::Rolling(cfg) => {
            assert_eq!(cfg.batch_size, 5);
            assert_eq!(cfg.batch_interval_secs, 30);
            assert_eq!(cfg.health_timeout_secs, 60);
            assert_eq!(cfg.max_unavailable, 2);
        }
        other => panic!("expected Rolling, got: {other:?}"),
    }
}

#[test]
fn canary_strategy_serialization_roundtrip() {
    let strategy = RolloutStrategy::Canary(CanaryConfig {
        traffic_percent: 20,
        canary_instances: 3,
        observation_secs: 600,
        error_rate_threshold: 1.5,
        latency_threshold_ms: 200,
    });

    let json = serde_json::to_string(&strategy).unwrap();
    let deserialized: RolloutStrategy = serde_json::from_str(&json).unwrap();

    match deserialized {
        RolloutStrategy::Canary(cfg) => {
            assert_eq!(cfg.traffic_percent, 20);
            assert_eq!(cfg.canary_instances, 3);
            assert_eq!(cfg.observation_secs, 600);
            assert!((cfg.error_rate_threshold - 1.5).abs() < f64::EPSILON);
            assert_eq!(cfg.latency_threshold_ms, 200);
        }
        other => panic!("expected Canary, got: {other:?}"),
    }
}

#[test]
fn blue_green_strategy_serialization_roundtrip() {
    let strategy = RolloutStrategy::BlueGreen;

    let json = serde_json::to_string(&strategy).unwrap();
    let deserialized: RolloutStrategy = serde_json::from_str(&json).unwrap();

    assert!(
        matches!(deserialized, RolloutStrategy::BlueGreen),
        "expected BlueGreen, got: {deserialized:?}"
    );
}

#[test]
fn rollout_phase_serialization_roundtrip() {
    let phases = vec![
        RolloutPhase::Pending,
        RolloutPhase::RollingBatch {
            current: 3,
            total: 7,
        },
        RolloutPhase::CanaryObserving,
        RolloutPhase::CanaryPromoting,
        RolloutPhase::HealthGate,
        RolloutPhase::Paused,
        RolloutPhase::Completed,
        RolloutPhase::RolledBack {
            reason: "test failure".to_string(),
        },
    ];

    for phase in &phases {
        let json = serde_json::to_string(phase).unwrap();
        let deserialized: RolloutPhase = serde_json::from_str(&json).unwrap();
        assert_eq!(
            &deserialized, phase,
            "phase roundtrip failed for {phase:?}"
        );
    }
}
