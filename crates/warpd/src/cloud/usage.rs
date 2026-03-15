//! Usage metering for the WarpGrid cloud platform.
//!
//! Tracks per-team resource consumption (requests, compute, egress)
//! using in-memory counters. Periodically snapshotted and reported
//! to the billing system.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use super::billing::UsageRecord;

// ── Per-team counters ───────────────────────────────────────────

#[derive(Debug, Clone)]
struct TeamCounters {
    period_start: u64,
    request_count: u64,
    compute_seconds: f64,
    egress_bytes: u64,
}

impl TeamCounters {
    fn new() -> Self {
        Self {
            period_start: epoch_secs(),
            request_count: 0,
            compute_seconds: 0.0,
            egress_bytes: 0,
        }
    }
}

// ── Usage tracker ───────────────────────────────────────────────

/// Tracks per-team resource consumption using in-memory counters.
///
/// Thread-safe via `Arc<RwLock<HashMap>>`. Counters are reset on
/// each snapshot to start a new billing period.
#[derive(Clone)]
pub struct UsageTracker {
    counters: Arc<RwLock<HashMap<String, TeamCounters>>>,
}

impl UsageTracker {
    pub fn new() -> Self {
        Self {
            counters: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record an HTTP request for a team's deployment.
    pub fn record_request(&self, team_id: &str, _deployment_id: &str, _latency_ms: u64) {
        let mut counters = self.counters.write().unwrap();
        let entry = counters
            .entry(team_id.to_string())
            .or_insert_with(TeamCounters::new);
        entry.request_count += 1;
    }

    /// Record compute time for a team.
    pub fn record_compute(&self, team_id: &str, duration_secs: f64) {
        let mut counters = self.counters.write().unwrap();
        let entry = counters
            .entry(team_id.to_string())
            .or_insert_with(TeamCounters::new);
        entry.compute_seconds += duration_secs;
    }

    /// Record egress bytes for a team.
    pub fn record_egress(&self, team_id: &str, bytes: u64) {
        let mut counters = self.counters.write().unwrap();
        let entry = counters
            .entry(team_id.to_string())
            .or_insert_with(TeamCounters::new);
        entry.egress_bytes += bytes;
    }

    /// Take a snapshot of the current period usage for a team and
    /// reset the counters for a new period.
    pub fn snapshot(&self, team_id: &str) -> UsageRecord {
        let mut counters = self.counters.write().unwrap();
        let now = epoch_secs();

        let entry = counters.remove(team_id);

        match entry {
            Some(c) => UsageRecord {
                team_id: team_id.to_string(),
                period_start: c.period_start,
                period_end: now,
                compute_seconds: c.compute_seconds,
                egress_bytes: c.egress_bytes,
                storage_bytes: 0,
                request_count: c.request_count,
            },
            None => UsageRecord {
                team_id: team_id.to_string(),
                period_start: now,
                period_end: now,
                compute_seconds: 0.0,
                egress_bytes: 0,
                storage_bytes: 0,
                request_count: 0,
            },
        }
    }

    /// Peek at current usage without resetting counters.
    pub fn peek(&self, team_id: &str) -> UsageRecord {
        let counters = self.counters.read().unwrap();
        let now = epoch_secs();

        match counters.get(team_id) {
            Some(c) => UsageRecord {
                team_id: team_id.to_string(),
                period_start: c.period_start,
                period_end: now,
                compute_seconds: c.compute_seconds,
                egress_bytes: c.egress_bytes,
                storage_bytes: 0,
                request_count: c.request_count,
            },
            None => UsageRecord {
                team_id: team_id.to_string(),
                period_start: now,
                period_end: now,
                compute_seconds: 0.0,
                egress_bytes: 0,
                storage_bytes: 0,
                request_count: 0,
            },
        }
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_snapshot_roundtrip() {
        let tracker = UsageTracker::new();

        tracker.record_request("team-1", "deploy-a", 15);
        tracker.record_request("team-1", "deploy-a", 22);
        tracker.record_request("team-1", "deploy-b", 8);
        tracker.record_compute("team-1", 1.5);
        tracker.record_compute("team-1", 2.0);
        tracker.record_egress("team-1", 1024);
        tracker.record_egress("team-1", 2048);

        let snapshot = tracker.snapshot("team-1");
        assert_eq!(snapshot.team_id, "team-1");
        assert_eq!(snapshot.request_count, 3);
        assert!((snapshot.compute_seconds - 3.5).abs() < f64::EPSILON);
        assert_eq!(snapshot.egress_bytes, 3072);
        assert!(snapshot.period_start <= snapshot.period_end);
    }

    #[test]
    fn multiple_teams_isolated() {
        let tracker = UsageTracker::new();

        tracker.record_request("team-a", "deploy-1", 10);
        tracker.record_request("team-a", "deploy-1", 20);
        tracker.record_compute("team-a", 5.0);

        tracker.record_request("team-b", "deploy-2", 30);
        tracker.record_egress("team-b", 4096);

        let snap_a = tracker.peek("team-a");
        let snap_b = tracker.peek("team-b");

        assert_eq!(snap_a.request_count, 2);
        assert!((snap_a.compute_seconds - 5.0).abs() < f64::EPSILON);
        assert_eq!(snap_a.egress_bytes, 0);

        assert_eq!(snap_b.request_count, 1);
        assert!((snap_b.compute_seconds - 0.0).abs() < f64::EPSILON);
        assert_eq!(snap_b.egress_bytes, 4096);
    }

    #[test]
    fn snapshot_resets_counters() {
        let tracker = UsageTracker::new();

        tracker.record_request("team-1", "deploy-1", 10);
        tracker.record_request("team-1", "deploy-1", 20);
        tracker.record_compute("team-1", 3.0);

        let first = tracker.snapshot("team-1");
        assert_eq!(first.request_count, 2);
        assert!((first.compute_seconds - 3.0).abs() < f64::EPSILON);

        // After snapshot, counters should be reset.
        let second = tracker.snapshot("team-1");
        assert_eq!(second.request_count, 0);
        assert!((second.compute_seconds - 0.0).abs() < f64::EPSILON);
        assert_eq!(second.egress_bytes, 0);
    }

    #[test]
    fn snapshot_unknown_team_returns_empty() {
        let tracker = UsageTracker::new();
        let snap = tracker.snapshot("nonexistent");
        assert_eq!(snap.team_id, "nonexistent");
        assert_eq!(snap.request_count, 0);
        assert!((snap.compute_seconds - 0.0).abs() < f64::EPSILON);
        assert_eq!(snap.egress_bytes, 0);
    }

    #[test]
    fn peek_does_not_reset() {
        let tracker = UsageTracker::new();

        tracker.record_request("team-1", "deploy-1", 5);
        tracker.record_request("team-1", "deploy-1", 10);

        let first_peek = tracker.peek("team-1");
        assert_eq!(first_peek.request_count, 2);

        let second_peek = tracker.peek("team-1");
        assert_eq!(second_peek.request_count, 2);
    }

    #[test]
    fn tracker_is_clone() {
        let tracker = UsageTracker::new();
        tracker.record_request("team-1", "deploy-1", 5);

        let cloned = tracker.clone();
        cloned.record_request("team-1", "deploy-1", 10);

        // Both references share the same underlying data.
        let snap = tracker.peek("team-1");
        assert_eq!(snap.request_count, 2);
    }
}
