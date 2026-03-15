//! Stripe billing integration for the WarpGrid cloud platform.
//!
//! Provides plan management, usage reporting, and billing portal access
//! via the Stripe API. When no `STRIPE_SECRET_KEY` is configured, falls
//! back to a mock client that logs operations via `tracing::debug`.
//!
//! Supports two backends for plan storage:
//! - In-memory (for tests and development without persistence)
//! - libSQL (for production — persists across restarts, edge-replicable)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::debug;

// ── Plans ───────────────────────────────────────────────────────

/// Billing plan tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
    Free,
    Pro,
    Enterprise,
}

impl Plan {
    /// Monthly price in USD cents.
    pub fn price_cents(self) -> Option<u64> {
        match self {
            Self::Free => Some(0),
            Self::Pro => Some(2900),
            Self::Enterprise => None, // custom pricing
        }
    }

    /// Human-readable price label.
    pub fn price_label(self) -> &'static str {
        match self {
            Self::Free => "$0/mo",
            Self::Pro => "$29/mo",
            Self::Enterprise => "Custom",
        }
    }
}

// ── Plan limits ─────────────────────────────────────────────────

/// Resource limits associated with a billing plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanLimits {
    pub max_deployments: u32,
    pub max_instances_per_deployment: u32,
    pub max_wasm_size_bytes: u64,
    pub max_request_rate: u32,
}

impl PlanLimits {
    /// Return the limits for a given plan.
    pub fn for_plan(plan: Plan) -> Self {
        match plan {
            Plan::Free => Self {
                max_deployments: 3,
                max_instances_per_deployment: 5,
                max_wasm_size_bytes: 10 * 1024 * 1024, // 10 MB
                max_request_rate: 100,
            },
            Plan::Pro => Self {
                max_deployments: 10,
                max_instances_per_deployment: 20,
                max_wasm_size_bytes: 50 * 1024 * 1024, // 50 MB
                max_request_rate: 1000,
            },
            Plan::Enterprise => Self {
                max_deployments: u32::MAX,
                max_instances_per_deployment: u32::MAX,
                max_wasm_size_bytes: u64::MAX,
                max_request_rate: u32::MAX,
            },
        }
    }
}

// ── Usage record ────────────────────────────────────────────────

/// A metered usage record for a billing period.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    pub team_id: String,
    pub period_start: u64,
    pub period_end: u64,
    pub compute_seconds: f64,
    pub egress_bytes: u64,
    pub storage_bytes: u64,
    pub request_count: u64,
}

// ── Stripe billing client ───────────────────────────────────────

const STRIPE_API_BASE: &str = "https://api.stripe.com/v1";

/// Live Stripe API client.
#[derive(Clone)]
pub struct StripeBillingClient {
    http: reqwest::Client,
    secret_key: String,
}

impl StripeBillingClient {
    pub fn new(secret_key: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            secret_key: secret_key.to_string(),
        }
    }

    pub async fn create_customer(
        &self,
        email: &str,
        team_id: &str,
    ) -> Result<String, BillingError> {
        let resp = self
            .http
            .post(format!("{STRIPE_API_BASE}/customers"))
            .bearer_auth(&self.secret_key)
            .form(&[("email", email), ("metadata[team_id]", team_id)])
            .send()
            .await
            .map_err(|e| BillingError::Api(e.to_string()))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BillingError::Api(format!("Stripe error: {body}")));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| BillingError::Api(e.to_string()))?;

        json["id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| BillingError::Api("missing customer id in response".to_string()))
    }

    pub async fn create_billing_portal_session(
        &self,
        customer_id: &str,
    ) -> Result<String, BillingError> {
        let resp = self
            .http
            .post(format!("{STRIPE_API_BASE}/billing_portal/sessions"))
            .bearer_auth(&self.secret_key)
            .form(&[("customer", customer_id)])
            .send()
            .await
            .map_err(|e| BillingError::Api(e.to_string()))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BillingError::Api(format!("Stripe error: {body}")));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| BillingError::Api(e.to_string()))?;

        json["url"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| BillingError::Api("missing portal URL in response".to_string()))
    }

    pub async fn report_usage(
        &self,
        customer_id: &str,
        usage: &UsageRecord,
    ) -> Result<(), BillingError> {
        // Report compute seconds as a metered usage event.
        let _resp = self
            .http
            .post(format!("{STRIPE_API_BASE}/billing/meter_events"))
            .bearer_auth(&self.secret_key)
            .form(&[
                ("event_name", "compute_seconds"),
                ("payload[stripe_customer_id]", customer_id),
                (
                    "payload[value]",
                    &format!("{}", usage.compute_seconds as u64),
                ),
            ])
            .send()
            .await
            .map_err(|e| BillingError::Api(e.to_string()))?;

        Ok(())
    }
}

// ── Mock billing client ─────────────────────────────────────────

/// Mock billing client for development and beta.
///
/// Logs all operations via `tracing::debug` and stores plan data
/// either in memory or in libSQL. Used when no `STRIPE_SECRET_KEY` is set.
#[derive(Clone)]
pub struct MockBillingClient {
    backend: BillingBackend,
    counter: Arc<RwLock<u64>>,
}

#[derive(Clone)]
enum BillingBackend {
    Memory {
        plans: Arc<RwLock<HashMap<String, Plan>>>,
    },
    LibSql {
        conn: libsql::Connection,
    },
}

impl MockBillingClient {
    /// Create an in-memory mock billing client (for tests).
    pub fn new() -> Self {
        Self {
            backend: BillingBackend::Memory {
                plans: Arc::new(RwLock::new(HashMap::new())),
            },
            counter: Arc::new(RwLock::new(0)),
        }
    }

    /// Create a persistent mock billing client backed by libSQL.
    pub fn with_libsql(conn: libsql::Connection) -> Self {
        Self {
            backend: BillingBackend::LibSql { conn },
            counter: Arc::new(RwLock::new(0)),
        }
    }

    fn next_id(&self) -> String {
        let mut counter = self.counter.write().unwrap();
        *counter += 1;
        format!("mock_cus_{:08x}", *counter)
    }

    pub async fn create_customer(
        &self,
        email: &str,
        team_id: &str,
    ) -> Result<String, BillingError> {
        let customer_id = self.next_id();
        debug!(
            customer_id = %customer_id,
            email = email,
            team_id = team_id,
            "mock: created customer"
        );

        match &self.backend {
            BillingBackend::Memory { plans } => {
                let mut store = plans.write().unwrap();
                store.insert(customer_id.clone(), Plan::Free);
            }
            BillingBackend::LibSql { conn } => {
                let now = epoch_secs();
                let _ = conn
                    .execute(
                        "INSERT INTO cloud_billing (customer_id, team_id, plan, created_at) VALUES (?, ?, ?, ?)",
                        libsql::params![
                            customer_id.clone(),
                            team_id.to_string(),
                            "free".to_string(),
                            now as i64
                        ],
                    )
                    .await;
            }
        }

        Ok(customer_id)
    }

    pub async fn create_billing_portal_session(
        &self,
        customer_id: &str,
    ) -> Result<String, BillingError> {
        debug!(
            customer_id = %customer_id,
            "mock: created billing portal session"
        );
        Ok(format!(
            "https://billing.stripe.com/mock/session/{customer_id}"
        ))
    }

    pub async fn report_usage(
        &self,
        customer_id: &str,
        usage: &UsageRecord,
    ) -> Result<(), BillingError> {
        debug!(
            customer_id = %customer_id,
            compute_seconds = usage.compute_seconds,
            request_count = usage.request_count,
            egress_bytes = usage.egress_bytes,
            "mock: reported usage"
        );
        Ok(())
    }

    pub async fn get_plan(&self, customer_id: &str) -> Plan {
        match &self.backend {
            BillingBackend::Memory { plans } => {
                let store = plans.read().unwrap();
                store.get(customer_id).copied().unwrap_or(Plan::Free)
            }
            BillingBackend::LibSql { conn } => {
                let mut rows = match conn
                    .query(
                        "SELECT plan FROM cloud_billing WHERE customer_id = ?",
                        libsql::params![customer_id.to_string()],
                    )
                    .await
                {
                    Ok(r) => r,
                    Err(_) => return Plan::Free,
                };

                let row = match rows.next().await {
                    Ok(Some(r)) => r,
                    _ => return Plan::Free,
                };

                let plan_str: String = match row.get(0) {
                    Ok(s) => s,
                    Err(_) => return Plan::Free,
                };

                match plan_str.as_str() {
                    "pro" => Plan::Pro,
                    "enterprise" => Plan::Enterprise,
                    _ => Plan::Free,
                }
            }
        }
    }

    pub async fn set_plan(&self, customer_id: &str, plan: Plan) {
        let plan_str = match plan {
            Plan::Free => "free",
            Plan::Pro => "pro",
            Plan::Enterprise => "enterprise",
        };

        match &self.backend {
            BillingBackend::Memory { plans } => {
                let mut store = plans.write().unwrap();
                store.insert(customer_id.to_string(), plan);
            }
            BillingBackend::LibSql { conn } => {
                let _ = conn
                    .execute(
                        "UPDATE cloud_billing SET plan = ? WHERE customer_id = ?",
                        libsql::params![plan_str.to_string(), customer_id.to_string()],
                    )
                    .await;
            }
        }
    }
}

// ── Billing error ───────────────────────────────────────────────

/// Error type for billing operations.
#[derive(Debug, Clone)]
pub enum BillingError {
    Api(String),
}

impl std::fmt::Display for BillingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api(msg) => write!(f, "billing API error: {msg}"),
        }
    }
}

impl std::error::Error for BillingError {}

// ── Billing service facade ──────────────────────────────────────

/// Unified billing facade — either a live Stripe client or a mock.
#[derive(Clone)]
pub enum BillingService {
    /// Live Stripe integration.
    Active(StripeBillingClient),
    /// Mock — logs operations via `tracing::debug`.
    Mock(MockBillingClient),
}

impl BillingService {
    /// Build from an optional Stripe secret key. Returns `Mock` when
    /// the key is `None` or empty.
    pub fn from_env(stripe_key: Option<String>) -> Self {
        match stripe_key {
            Some(ref key) if !key.is_empty() => Self::Active(StripeBillingClient::new(key)),
            _ => Self::Mock(MockBillingClient::new()),
        }
    }

    /// Build from an optional Stripe secret key with a libSQL connection
    /// for plan persistence. Returns `Mock` with libSQL backend when
    /// the key is `None` or empty.
    pub fn from_env_with_libsql(stripe_key: Option<String>, conn: libsql::Connection) -> Self {
        match stripe_key {
            Some(ref key) if !key.is_empty() => Self::Active(StripeBillingClient::new(key)),
            _ => Self::Mock(MockBillingClient::with_libsql(conn)),
        }
    }

    /// Create a Stripe customer for a team.
    pub async fn create_customer(
        &self,
        email: &str,
        team_id: &str,
    ) -> Result<String, BillingError> {
        match self {
            Self::Active(client) => client.create_customer(email, team_id).await,
            Self::Mock(mock) => mock.create_customer(email, team_id).await,
        }
    }

    /// Create a Stripe billing portal session and return the URL.
    pub async fn create_billing_portal_session(
        &self,
        customer_id: &str,
    ) -> Result<String, BillingError> {
        match self {
            Self::Active(client) => client.create_billing_portal_session(customer_id).await,
            Self::Mock(mock) => mock.create_billing_portal_session(customer_id).await,
        }
    }

    /// Report metered usage to Stripe.
    pub async fn report_usage(
        &self,
        customer_id: &str,
        usage: &UsageRecord,
    ) -> Result<(), BillingError> {
        match self {
            Self::Active(client) => client.report_usage(customer_id, usage).await,
            Self::Mock(mock) => mock.report_usage(customer_id, usage).await,
        }
    }

    /// Get the current plan for a customer.
    pub async fn get_plan(&self, customer_id: &str) -> Plan {
        match self {
            Self::Active(_) => {
                // In production, this would query Stripe subscriptions.
                // For now, default to Free.
                debug!(
                    customer_id = %customer_id,
                    "stripe: get_plan defaulting to Free (subscription lookup not yet implemented)"
                );
                Plan::Free
            }
            Self::Mock(mock) => mock.get_plan(customer_id).await,
        }
    }

    /// Set the plan for a customer (used by webhook handlers).
    pub async fn set_plan(&self, customer_id: &str, plan: Plan) {
        match self {
            Self::Active(_) => {
                // In production with a live Stripe client, the plan is
                // managed by Stripe subscriptions — this is a no-op.
                debug!(
                    customer_id = %customer_id,
                    plan = ?plan,
                    "stripe: set_plan is a no-op for live Stripe (subscription is source of truth)"
                );
            }
            Self::Mock(mock) => mock.set_plan(customer_id, plan).await,
        }
    }
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_plan_limits_match_beta_constraints() {
        let limits = PlanLimits::for_plan(Plan::Free);
        assert_eq!(limits.max_deployments, 3);
        assert_eq!(limits.max_instances_per_deployment, 5);
        assert_eq!(limits.max_wasm_size_bytes, 10 * 1024 * 1024);
        assert_eq!(limits.max_request_rate, 100);
    }

    #[test]
    fn pro_plan_limits() {
        let limits = PlanLimits::for_plan(Plan::Pro);
        assert_eq!(limits.max_deployments, 10);
        assert_eq!(limits.max_instances_per_deployment, 20);
        assert_eq!(limits.max_wasm_size_bytes, 50 * 1024 * 1024);
        assert_eq!(limits.max_request_rate, 1000);
    }

    #[test]
    fn enterprise_plan_limits_are_unlimited() {
        let limits = PlanLimits::for_plan(Plan::Enterprise);
        assert_eq!(limits.max_deployments, u32::MAX);
        assert_eq!(limits.max_instances_per_deployment, u32::MAX);
        assert_eq!(limits.max_request_rate, u32::MAX);
    }

    #[test]
    fn plan_price_cents() {
        assert_eq!(Plan::Free.price_cents(), Some(0));
        assert_eq!(Plan::Pro.price_cents(), Some(2900));
        assert_eq!(Plan::Enterprise.price_cents(), None);
    }

    #[test]
    fn plan_price_labels() {
        assert_eq!(Plan::Free.price_label(), "$0/mo");
        assert_eq!(Plan::Pro.price_label(), "$29/mo");
        assert_eq!(Plan::Enterprise.price_label(), "Custom");
    }

    #[test]
    fn plan_serialization_roundtrip() {
        let plans = [Plan::Free, Plan::Pro, Plan::Enterprise];
        for plan in plans {
            let json = serde_json::to_string(&plan).unwrap();
            let deserialized: Plan = serde_json::from_str(&json).unwrap();
            assert_eq!(plan, deserialized);
        }
    }

    #[test]
    fn plan_serializes_as_lowercase() {
        assert_eq!(serde_json::to_string(&Plan::Free).unwrap(), r#""free""#);
        assert_eq!(serde_json::to_string(&Plan::Pro).unwrap(), r#""pro""#);
        assert_eq!(
            serde_json::to_string(&Plan::Enterprise).unwrap(),
            r#""enterprise""#
        );
    }

    #[tokio::test]
    async fn mock_billing_roundtrip() {
        let mock = MockBillingClient::new();

        // Create customer.
        let customer_id = mock
            .create_customer("alice@example.com", "team-1")
            .await
            .unwrap();
        assert!(customer_id.starts_with("mock_cus_"));

        // Default plan is Free.
        assert_eq!(mock.get_plan(&customer_id).await, Plan::Free);

        // Upgrade to Pro.
        mock.set_plan(&customer_id, Plan::Pro).await;
        assert_eq!(mock.get_plan(&customer_id).await, Plan::Pro);

        // Create portal session.
        let url = mock
            .create_billing_portal_session(&customer_id)
            .await
            .unwrap();
        assert!(url.starts_with("https://billing.stripe.com/mock/session/"));
        assert!(url.contains(&customer_id));

        // Report usage.
        let usage = UsageRecord {
            team_id: "team-1".to_string(),
            period_start: 1000,
            period_end: 2000,
            compute_seconds: 42.5,
            egress_bytes: 1024,
            storage_bytes: 2048,
            request_count: 100,
        };
        let result = mock.report_usage(&customer_id, &usage).await;
        assert!(result.is_ok());
    }

    #[test]
    fn usage_record_construction() {
        let record = UsageRecord {
            team_id: "team-abc".to_string(),
            period_start: 1700000000,
            period_end: 1700003600,
            compute_seconds: 123.456,
            egress_bytes: 1024 * 1024,
            storage_bytes: 512 * 1024,
            request_count: 5000,
        };
        assert_eq!(record.team_id, "team-abc");
        assert_eq!(record.period_end - record.period_start, 3600);
        assert_eq!(record.request_count, 5000);
    }

    #[test]
    fn usage_record_serialization() {
        let record = UsageRecord {
            team_id: "team-1".to_string(),
            period_start: 1000,
            period_end: 2000,
            compute_seconds: 10.0,
            egress_bytes: 100,
            storage_bytes: 200,
            request_count: 50,
        };
        let json = serde_json::to_string(&record).unwrap();
        let deserialized: UsageRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.team_id, "team-1");
        assert_eq!(deserialized.request_count, 50);
    }

    #[test]
    fn billing_service_from_env_none_yields_mock() {
        assert!(matches!(
            BillingService::from_env(None),
            BillingService::Mock(_)
        ));
    }

    #[test]
    fn billing_service_from_env_empty_yields_mock() {
        assert!(matches!(
            BillingService::from_env(Some(String::new())),
            BillingService::Mock(_)
        ));
    }

    #[test]
    fn billing_service_from_env_with_key_yields_active() {
        assert!(matches!(
            BillingService::from_env(Some("sk_test_abc123".to_string())),
            BillingService::Active(_)
        ));
    }

    #[tokio::test]
    async fn billing_service_mock_create_customer() {
        let svc = BillingService::from_env(None);
        let customer_id = svc
            .create_customer("test@example.com", "team-1")
            .await
            .unwrap();
        assert!(!customer_id.is_empty());
        assert_eq!(svc.get_plan(&customer_id).await, Plan::Free);
    }

    #[test]
    fn mock_customer_ids_are_unique() {
        let mock = MockBillingClient::new();
        let id1 = mock.next_id();
        let id2 = mock.next_id();
        assert_ne!(id1, id2);
    }

    #[tokio::test]
    async fn unknown_customer_defaults_to_free() {
        let mock = MockBillingClient::new();
        assert_eq!(mock.get_plan("nonexistent").await, Plan::Free);
    }

    // ── libSQL backend tests ────────────────────────────────────

    #[tokio::test]
    async fn libsql_create_customer_and_get_plan() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let mock = MockBillingClient::with_libsql(conn);

        let customer_id = mock
            .create_customer("alice@example.com", "team-1")
            .await
            .unwrap();
        assert!(customer_id.starts_with("mock_cus_"));

        // Default plan is Free.
        assert_eq!(mock.get_plan(&customer_id).await, Plan::Free);

        // Upgrade to Pro.
        mock.set_plan(&customer_id, Plan::Pro).await;
        assert_eq!(mock.get_plan(&customer_id).await, Plan::Pro);
    }

    #[tokio::test]
    async fn libsql_billing_service_with_libsql() {
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();

        let svc = BillingService::from_env_with_libsql(None, conn);
        let customer_id = svc
            .create_customer("test@example.com", "team-1")
            .await
            .unwrap();
        assert!(!customer_id.is_empty());
        assert_eq!(svc.get_plan(&customer_id).await, Plan::Free);
    }
}
