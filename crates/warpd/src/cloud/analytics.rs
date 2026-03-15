//! PostHog analytics for the WarpGrid cloud platform.
//!
//! Provides server-side event tracking via PostHog's capture API.
//! When no `POSTHOG_API_KEY` is configured, falls back to a no-op
//! client that logs events via `tracing::debug`.

use serde_json::{Value, json};
use tracing::debug;

// ── Event constants ────────────────────────────────────────────

pub const EVENT_USER_REGISTERED: &str = "user_registered";
pub const EVENT_USER_LOGIN: &str = "user_login";
pub const EVENT_TEAM_CREATED: &str = "team_created";
pub const EVENT_DEPLOYMENT_CREATED: &str = "deployment_created";
pub const EVENT_DEPLOY_SUCCEEDED: &str = "deploy_succeeded";
pub const EVENT_DEPLOY_FAILED: &str = "deploy_failed";
pub const EVENT_DEPLOYMENT_SCALED: &str = "deployment_scaled";
pub const EVENT_DEPLOYMENT_DELETED: &str = "deployment_deleted";
pub const EVENT_DOMAIN_ADDED: &str = "domain_added";
pub const EVENT_ROLLOUT_STARTED: &str = "rollout_started";

const DEFAULT_POSTHOG_HOST: &str = "https://us.i.posthog.com";

// ── AnalyticsClient ────────────────────────────────────────────

/// HTTP client wrapper for PostHog's capture API.
#[derive(Clone)]
pub struct AnalyticsClient {
    http: reqwest::Client,
    api_key: String,
    host: String,
}

impl AnalyticsClient {
    /// Create a new PostHog analytics client.
    ///
    /// `host` defaults to `https://us.i.posthog.com` when `None`.
    pub fn new(api_key: &str, host: Option<&str>) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key: api_key.to_string(),
            host: host.unwrap_or(DEFAULT_POSTHOG_HOST).to_string(),
        }
    }

    /// Fire-and-forget event capture.
    ///
    /// Spawns a background task to POST to PostHog — never blocks
    /// the request path.
    pub fn capture(&self, distinct_id: &str, event: &str, properties: Value) {
        let url = format!("{}/capture/", self.host);
        let body = json!({
            "api_key": self.api_key,
            "event": event,
            "distinct_id": distinct_id,
            "properties": properties,
        });
        let client = self.http.clone();

        tokio::spawn(async move {
            if let Err(e) = client.post(&url).json(&body).send().await {
                tracing::warn!(error = %e, "posthog capture failed");
            }
        });
    }

    /// Identify a user with PostHog `$identify` event.
    pub fn identify(&self, distinct_id: &str, properties: Value) {
        self.capture(
            distinct_id,
            "$identify",
            json!({
                "$set": properties,
            }),
        );
    }
}

// ── AnalyticsService ───────────────────────────────────────────

/// Unified analytics facade — either an active PostHog client or a
/// no-op logger.
#[derive(Clone)]
pub enum AnalyticsService {
    /// Live PostHog integration.
    Active(AnalyticsClient),
    /// No-op — logs events via `tracing::debug` instead.
    Noop,
}

impl AnalyticsService {
    /// Build from an optional API key. Returns `Noop` when the key
    /// is `None` or empty.
    pub fn from_env(api_key: Option<&str>) -> Self {
        match api_key {
            Some(key) if !key.is_empty() => Self::Active(AnalyticsClient::new(key, None)),
            _ => Self::Noop,
        }
    }

    /// Track an analytics event.
    ///
    /// - `Active` — sends to PostHog in the background.
    /// - `Noop`   — logs via `tracing::debug`.
    pub fn track(&self, distinct_id: &str, event: &str, properties: Value) {
        match self {
            Self::Active(client) => client.capture(distinct_id, event, properties),
            Self::Noop => {
                debug!(
                    distinct_id = distinct_id,
                    event = event,
                    "analytics event (noop)"
                );
            }
        }
    }

    /// Identify a user.
    pub fn identify(&self, distinct_id: &str, properties: Value) {
        match self {
            Self::Active(client) => client.identify(distinct_id, properties),
            Self::Noop => {
                debug!(distinct_id = distinct_id, "analytics identify (noop)");
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_constants_use_snake_case() {
        let events = [
            EVENT_USER_REGISTERED,
            EVENT_USER_LOGIN,
            EVENT_TEAM_CREATED,
            EVENT_DEPLOYMENT_CREATED,
            EVENT_DEPLOY_SUCCEEDED,
            EVENT_DEPLOY_FAILED,
            EVENT_DEPLOYMENT_SCALED,
            EVENT_DEPLOYMENT_DELETED,
            EVENT_DOMAIN_ADDED,
            EVENT_ROLLOUT_STARTED,
        ];
        for event in events {
            assert!(!event.is_empty(), "event constant must not be empty");
            assert!(
                event.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "event '{event}' must be snake_case"
            );
        }
    }

    #[test]
    fn noop_track_does_not_panic() {
        let svc = AnalyticsService::Noop;
        svc.track("user-1", EVENT_USER_REGISTERED, json!({"email": "a@b.com"}));
        svc.identify("user-1", json!({"plan": "free"}));
    }

    #[test]
    fn from_env_none_yields_noop() {
        assert!(matches!(
            AnalyticsService::from_env(None),
            AnalyticsService::Noop
        ));
    }

    #[test]
    fn from_env_empty_yields_noop() {
        assert!(matches!(
            AnalyticsService::from_env(Some("")),
            AnalyticsService::Noop
        ));
    }

    #[test]
    fn from_env_with_key_yields_active() {
        assert!(matches!(
            AnalyticsService::from_env(Some("phc_test123")),
            AnalyticsService::Active(_)
        ));
    }

    #[test]
    fn property_building() {
        let props = json!({
            "email": "user@example.com",
            "namespace": "ns-abc",
            "plan": "pro",
        });
        assert_eq!(props["email"], "user@example.com");
        assert_eq!(props["namespace"], "ns-abc");
        assert_eq!(props["plan"], "pro");
    }

    #[test]
    fn analytics_client_default_host() {
        let client = AnalyticsClient::new("phc_key", None);
        assert_eq!(client.host, DEFAULT_POSTHOG_HOST);
    }

    #[test]
    fn analytics_client_custom_host() {
        let client = AnalyticsClient::new("phc_key", Some("https://eu.posthog.com"));
        assert_eq!(client.host, "https://eu.posthog.com");
    }
}
