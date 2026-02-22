//! Health check probe logic.
//!
//! Performs HTTP health checks against instance endpoints with
//! configurable thresholds and exponential backoff.

use std::time::Duration;

use tracing::{debug, warn};

use warpgrid_state::{HealthConfig, HealthStatus};

/// Result of a single health probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeResult {
    /// The health endpoint returned 2xx.
    Healthy,
    /// The health endpoint returned non-2xx or timed out.
    Unhealthy,
    /// The probe could not be executed (connection error).
    Failed,
}

/// Tracks consecutive probe results for a single instance.
#[derive(Debug)]
pub struct HealthTracker {
    /// Current health status.
    status: HealthStatus,
    /// Consecutive failure count.
    consecutive_failures: u32,
    /// Consecutive success count (for recovery).
    consecutive_successes: u32,
    /// Threshold before marking unhealthy.
    unhealthy_threshold: u32,
    /// Successes needed to recover from unhealthy.
    healthy_threshold: u32,
    /// Current backoff interval.
    current_backoff: Duration,
    /// Base check interval.
    base_interval: Duration,
    /// Maximum backoff.
    max_backoff: Duration,
}

impl HealthTracker {
    /// Create a new tracker from a health config.
    pub fn new(config: &HealthConfig) -> Self {
        let base_interval = parse_duration(&config.interval).unwrap_or(Duration::from_secs(5));
        Self {
            status: HealthStatus::Unknown,
            consecutive_failures: 0,
            consecutive_successes: 0,
            unhealthy_threshold: config.unhealthy_threshold,
            healthy_threshold: 1, // Single success to recover.
            current_backoff: base_interval,
            base_interval,
            max_backoff: Duration::from_secs(60),
        }
    }

    /// Create a tracker with custom thresholds (for testing).
    pub fn with_thresholds(
        unhealthy_threshold: u32,
        healthy_threshold: u32,
        interval: Duration,
    ) -> Self {
        Self {
            status: HealthStatus::Unknown,
            consecutive_failures: 0,
            consecutive_successes: 0,
            unhealthy_threshold,
            healthy_threshold,
            current_backoff: interval,
            base_interval: interval,
            max_backoff: Duration::from_secs(60),
        }
    }

    /// Record a probe result and return the new health status.
    pub fn record(&mut self, result: ProbeResult) -> HealthStatus {
        match result {
            ProbeResult::Healthy => {
                self.consecutive_failures = 0;
                self.consecutive_successes += 1;
                self.current_backoff = self.base_interval;

                if self.consecutive_successes >= self.healthy_threshold {
                    if self.status != HealthStatus::Healthy {
                        debug!(
                            successes = self.consecutive_successes,
                            "instance recovered to healthy"
                        );
                    }
                    self.status = HealthStatus::Healthy;
                }
            }
            ProbeResult::Unhealthy | ProbeResult::Failed => {
                self.consecutive_successes = 0;
                self.consecutive_failures += 1;

                // Exponential backoff: double the interval up to max.
                self.current_backoff = (self.current_backoff * 2).min(self.max_backoff);

                if self.consecutive_failures >= self.unhealthy_threshold {
                    if self.status != HealthStatus::Unhealthy {
                        warn!(
                            failures = self.consecutive_failures,
                            threshold = self.unhealthy_threshold,
                            "instance marked unhealthy"
                        );
                    }
                    self.status = HealthStatus::Unhealthy;
                }
            }
        }

        self.status
    }

    /// Current health status.
    pub fn status(&self) -> HealthStatus {
        self.status
    }

    /// Current number of consecutive failures.
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Current backoff interval before next check.
    pub fn next_interval(&self) -> Duration {
        self.current_backoff
    }

    /// Whether this instance needs replacement (unhealthy).
    pub fn needs_replacement(&self) -> bool {
        self.status == HealthStatus::Unhealthy
    }
}

/// Perform an HTTP health probe against an endpoint.
///
/// Returns `Healthy` if the response is 2xx, `Unhealthy` for non-2xx,
/// or `Failed` if the connection fails or times out.
pub async fn http_probe(address: &str, path: &str, timeout: Duration) -> ProbeResult {
    let uri = format!("http://{address}{path}");

    let result = tokio::time::timeout(timeout, async {
        // Use a simple TCP connection + hyper for the probe.
        let stream = match tokio::net::TcpStream::connect(address).await {
            Ok(s) => s,
            Err(e) => {
                debug!(error = %e, %uri, "health probe connection failed");
                return ProbeResult::Failed;
            }
        };

        let io = hyper_util::rt::TokioIo::new(stream);
        let (mut sender, conn) = match hyper::client::conn::http1::handshake(io).await {
            Ok(pair) => pair,
            Err(e) => {
                debug!(error = %e, %uri, "health probe handshake failed");
                return ProbeResult::Failed;
            }
        };

        // Drive the connection in the background.
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let req = http::Request::builder()
            .method("GET")
            .uri(&uri)
            .header("host", address)
            .header("user-agent", "warpgrid-health/0.1")
            .body(http_body_util::Empty::<bytes::Bytes>::new())
            .unwrap();

        match sender.send_request(req).await {
            Ok(resp) => {
                if resp.status().is_success() {
                    ProbeResult::Healthy
                } else {
                    debug!(status = %resp.status(), %uri, "health probe non-2xx");
                    ProbeResult::Unhealthy
                }
            }
            Err(e) => {
                debug!(error = %e, %uri, "health probe request failed");
                ProbeResult::Failed
            }
        }
    })
    .await;

    match result {
        Ok(probe) => probe,
        Err(_) => {
            debug!(%uri, "health probe timed out");
            ProbeResult::Failed
        }
    }
}

/// Parse a duration string like "5s", "500ms", "1m".
fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if let Some(secs) = s.strip_suffix('s') {
        if let Some(ms) = secs.strip_suffix('m') {
            ms.parse::<u64>().ok().map(Duration::from_millis)
        } else {
            secs.parse::<u64>().ok().map(Duration::from_secs)
        }
    } else if let Some(mins) = s.strip_suffix('m') {
        mins.parse::<u64>().ok().map(|m| Duration::from_secs(m * 60))
    } else {
        s.parse::<u64>().ok().map(Duration::from_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> HealthConfig {
        HealthConfig {
            endpoint: "/healthz".to_string(),
            interval: "5s".to_string(),
            timeout: "2s".to_string(),
            unhealthy_threshold: 3,
        }
    }

    #[test]
    fn tracker_starts_unknown() {
        let tracker = HealthTracker::new(&test_config());
        assert_eq!(tracker.status(), HealthStatus::Unknown);
        assert_eq!(tracker.consecutive_failures(), 0);
    }

    #[test]
    fn tracker_becomes_healthy_on_first_success() {
        let mut tracker = HealthTracker::new(&test_config());
        let status = tracker.record(ProbeResult::Healthy);
        assert_eq!(status, HealthStatus::Healthy);
    }

    #[test]
    fn tracker_stays_healthy_under_threshold() {
        let mut tracker = HealthTracker::new(&test_config());
        tracker.record(ProbeResult::Healthy);

        // Two failures — under threshold of 3.
        tracker.record(ProbeResult::Unhealthy);
        tracker.record(ProbeResult::Unhealthy);
        assert_eq!(tracker.status(), HealthStatus::Healthy);
        assert_eq!(tracker.consecutive_failures(), 2);
    }

    #[test]
    fn tracker_becomes_unhealthy_at_threshold() {
        let mut tracker = HealthTracker::new(&test_config());
        tracker.record(ProbeResult::Healthy);

        tracker.record(ProbeResult::Unhealthy);
        tracker.record(ProbeResult::Unhealthy);
        let status = tracker.record(ProbeResult::Unhealthy);
        assert_eq!(status, HealthStatus::Unhealthy);
        assert!(tracker.needs_replacement());
    }

    #[test]
    fn tracker_recovers_on_success() {
        let mut tracker = HealthTracker::new(&test_config());

        // Drive to unhealthy.
        for _ in 0..3 {
            tracker.record(ProbeResult::Unhealthy);
        }
        assert_eq!(tracker.status(), HealthStatus::Unhealthy);

        // Single success recovers.
        let status = tracker.record(ProbeResult::Healthy);
        assert_eq!(status, HealthStatus::Healthy);
        assert!(!tracker.needs_replacement());
    }

    #[test]
    fn tracker_exponential_backoff() {
        let mut tracker =
            HealthTracker::with_thresholds(3, 1, Duration::from_secs(1));

        // Base interval.
        assert_eq!(tracker.next_interval(), Duration::from_secs(1));

        // Each failure doubles the backoff.
        tracker.record(ProbeResult::Unhealthy);
        assert_eq!(tracker.next_interval(), Duration::from_secs(2));

        tracker.record(ProbeResult::Unhealthy);
        assert_eq!(tracker.next_interval(), Duration::from_secs(4));

        tracker.record(ProbeResult::Unhealthy);
        assert_eq!(tracker.next_interval(), Duration::from_secs(8));
    }

    #[test]
    fn tracker_backoff_caps_at_max() {
        let mut tracker =
            HealthTracker::with_thresholds(100, 1, Duration::from_secs(1));

        // Drive failures until backoff exceeds 60s.
        for _ in 0..10 {
            tracker.record(ProbeResult::Failed);
        }
        // 1 → 2 → 4 → 8 → 16 → 32 → 60 → 60 → 60 → 60
        assert_eq!(tracker.next_interval(), Duration::from_secs(60));
    }

    #[test]
    fn tracker_backoff_resets_on_success() {
        let mut tracker =
            HealthTracker::with_thresholds(3, 1, Duration::from_secs(1));

        tracker.record(ProbeResult::Unhealthy);
        tracker.record(ProbeResult::Unhealthy);
        assert_eq!(tracker.next_interval(), Duration::from_secs(4));

        tracker.record(ProbeResult::Healthy);
        assert_eq!(tracker.next_interval(), Duration::from_secs(1));
    }

    #[test]
    fn tracker_failed_counts_as_failure() {
        let mut tracker = HealthTracker::new(&test_config());
        tracker.record(ProbeResult::Healthy);

        tracker.record(ProbeResult::Failed);
        tracker.record(ProbeResult::Failed);
        tracker.record(ProbeResult::Failed);
        assert_eq!(tracker.status(), HealthStatus::Unhealthy);
    }

    #[test]
    fn parse_duration_seconds() {
        assert_eq!(parse_duration("5s"), Some(Duration::from_secs(5)));
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
    }

    #[test]
    fn parse_duration_milliseconds() {
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
    }

    #[test]
    fn parse_duration_minutes() {
        assert_eq!(parse_duration("2m"), Some(Duration::from_secs(120)));
    }

    #[test]
    fn parse_duration_plain_number_as_seconds() {
        assert_eq!(parse_duration("10"), Some(Duration::from_secs(10)));
    }

    #[test]
    fn custom_thresholds() {
        let mut tracker =
            HealthTracker::with_thresholds(5, 3, Duration::from_secs(1));

        // Need 5 failures for unhealthy.
        for _ in 0..4 {
            tracker.record(ProbeResult::Unhealthy);
        }
        assert_ne!(tracker.status(), HealthStatus::Unhealthy);

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
}
