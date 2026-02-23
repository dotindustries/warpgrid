//! REST API handlers for rollout management.
//!
//! Provides endpoints to start, list, get, pause, and resume rollouts.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use tokio::sync::RwLock;

use warpgrid_rollout::{Rollout, RolloutPhase, RolloutStrategy};

/// Shared rollout state across handlers.
pub type RolloutStore = Arc<RwLock<HashMap<String, Rollout>>>;

/// Rollout-aware API state.
#[derive(Clone)]
pub struct RolloutApiState {
    pub store: warpgrid_state::StateStore,
    pub rollouts: RolloutStore,
}

/// Response wrapper for rollout endpoints.
#[derive(serde::Serialize)]
struct RolloutResponse<T: serde::Serialize> {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl<T: serde::Serialize> RolloutResponse<T> {
    fn ok(data: T) -> Json<Self> {
        Json(Self {
            success: true,
            data: Some(data),
            error: None,
        })
    }
}

fn rollout_error(msg: &str, status: StatusCode) -> impl IntoResponse {
    (
        status,
        Json(RolloutResponse::<()> {
            success: false,
            data: None,
            error: Some(msg.to_string()),
        }),
    )
}

/// Serializable rollout status for API responses.
#[derive(serde::Serialize)]
pub struct RolloutStatus {
    pub deployment_id: String,
    pub phase: RolloutPhase,
    pub old_version: String,
    pub new_version: String,
    pub target_instances: u32,
}

impl From<&Rollout> for RolloutStatus {
    fn from(r: &Rollout) -> Self {
        Self {
            deployment_id: r.deployment_id.clone(),
            phase: r.phase.clone(),
            old_version: r.old_version.clone(),
            new_version: r.new_version.clone(),
            target_instances: r.target_instances,
        }
    }
}

/// Request body to start a rollout.
#[derive(serde::Deserialize)]
pub struct StartRolloutRequest {
    pub strategy: RolloutStrategy,
    pub new_version: String,
}

/// POST /api/v1/deployments/:id/rollout
pub async fn start_rollout(
    State(state): State<RolloutApiState>,
    Path(id): Path<String>,
    Json(req): Json<StartRolloutRequest>,
) -> impl IntoResponse {
    // Verify deployment exists.
    let spec = match state.store.get_deployment(&id) {
        Ok(Some(spec)) => spec,
        Ok(None) => {
            return rollout_error("deployment not found", StatusCode::NOT_FOUND).into_response()
        }
        Err(e) => {
            return rollout_error(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response()
        }
    };

    // Check for existing active rollout.
    {
        let rollouts = state.rollouts.read().await;
        if let Some(existing) = rollouts.get(&id) {
            if existing.phase != RolloutPhase::Completed
                && !matches!(existing.phase, RolloutPhase::RolledBack { .. })
            {
                return rollout_error(
                    "rollout already in progress",
                    StatusCode::CONFLICT,
                )
                .into_response();
            }
        }
    }

    // Create and start the rollout.
    let old_version = spec.source.clone();
    let mut rollout = Rollout::new(
        &id,
        req.strategy,
        spec.instances.min,
        &old_version,
        &req.new_version,
    );
    rollout.start();

    let status = RolloutStatus::from(&rollout);

    {
        let mut rollouts = state.rollouts.write().await;
        rollouts.insert(id, rollout);
    }

    (StatusCode::CREATED, RolloutResponse::ok(status)).into_response()
}

/// GET /api/v1/rollouts
pub async fn list_rollouts(State(state): State<RolloutApiState>) -> impl IntoResponse {
    let rollouts = state.rollouts.read().await;
    let statuses: Vec<RolloutStatus> = rollouts.values().map(RolloutStatus::from).collect();
    RolloutResponse::ok(statuses).into_response()
}

/// GET /api/v1/rollouts/:id
pub async fn get_rollout(
    State(state): State<RolloutApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let rollouts = state.rollouts.read().await;
    match rollouts.get(&id) {
        Some(rollout) => RolloutResponse::ok(RolloutStatus::from(rollout)).into_response(),
        None => rollout_error("rollout not found", StatusCode::NOT_FOUND).into_response(),
    }
}

/// POST /api/v1/rollouts/:id/pause
pub async fn pause_rollout(
    State(state): State<RolloutApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut rollouts = state.rollouts.write().await;
    match rollouts.get_mut(&id) {
        Some(rollout) => {
            rollout.pause();
            RolloutResponse::ok(RolloutStatus::from(&*rollout)).into_response()
        }
        None => rollout_error("rollout not found", StatusCode::NOT_FOUND).into_response(),
    }
}

/// POST /api/v1/rollouts/:id/resume
pub async fn resume_rollout(
    State(state): State<RolloutApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut rollouts = state.rollouts.write().await;
    match rollouts.get_mut(&id) {
        Some(rollout) => {
            rollout.resume();
            RolloutResponse::ok(RolloutStatus::from(&*rollout)).into_response()
        }
        None => rollout_error("rollout not found", StatusCode::NOT_FOUND).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use warpgrid_rollout::strategy::{CanaryConfig, RollingConfig};
    use warpgrid_state::*;

    fn test_state() -> RolloutApiState {
        let store = StateStore::open_in_memory().unwrap();
        RolloutApiState {
            store,
            rollouts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn test_deployment(ns: &str, name: &str) -> DeploymentSpec {
        DeploymentSpec {
            id: format!("{ns}/{name}"),
            namespace: ns.to_string(),
            name: name.to_string(),
            source: "oci://registry/app:v1".to_string(),
            trigger: TriggerConfig::Http { port: Some(8080) },
            instances: InstanceConstraints { min: 3, max: 10 },
            resources: ResourceLimits {
                memory_bytes: 64 * 1024 * 1024,
                cpu_weight: 100,
            },
            scaling: None,
            health: None,
            shims: ShimsEnabled::default(),
            env: HashMap::new(),
            created_at: 1000,
            updated_at: 1000,
        }
    }

    #[tokio::test]
    async fn start_rollout_for_existing_deployment() {
        let state = test_state();
        let spec = test_deployment("prod", "api");
        state.store.put_deployment(&spec).unwrap();

        let req = StartRolloutRequest {
            strategy: RolloutStrategy::Rolling(RollingConfig {
                batch_size: 1,
                ..Default::default()
            }),
            new_version: "v2".to_string(),
        };

        let resp = start_rollout(
            State(state.clone()),
            Path("prod/api".to_string()),
            Json(req),
        )
        .await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Verify rollout was stored.
        let rollouts = state.rollouts.read().await;
        assert!(rollouts.contains_key("prod/api"));
    }

    #[tokio::test]
    async fn start_rollout_missing_deployment() {
        let state = test_state();
        let req = StartRolloutRequest {
            strategy: RolloutStrategy::default(),
            new_version: "v2".to_string(),
        };

        let resp = start_rollout(
            State(state),
            Path("nope/missing".to_string()),
            Json(req),
        )
        .await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn duplicate_active_rollout_rejected() {
        let state = test_state();
        let spec = test_deployment("prod", "api");
        state.store.put_deployment(&spec).unwrap();

        let req = StartRolloutRequest {
            strategy: RolloutStrategy::default(),
            new_version: "v2".to_string(),
        };

        // First rollout succeeds.
        let resp = start_rollout(
            State(state.clone()),
            Path("prod/api".to_string()),
            Json(req),
        )
        .await;
        assert_eq!(resp.into_response().status(), StatusCode::CREATED);

        // Second rollout should conflict.
        let req2 = StartRolloutRequest {
            strategy: RolloutStrategy::default(),
            new_version: "v3".to_string(),
        };
        let resp = start_rollout(
            State(state),
            Path("prod/api".to_string()),
            Json(req2),
        )
        .await;
        assert_eq!(resp.into_response().status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn list_rollouts_empty() {
        let state = test_state();
        let resp = list_rollouts(State(state)).await;
        assert_eq!(resp.into_response().status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn get_nonexistent_rollout() {
        let state = test_state();
        let resp = get_rollout(State(state), Path("nope".to_string())).await;
        assert_eq!(resp.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn pause_and_resume_rollout() {
        let state = test_state();
        let spec = test_deployment("prod", "api");
        state.store.put_deployment(&spec).unwrap();

        // Start a rolling update.
        let req = StartRolloutRequest {
            strategy: RolloutStrategy::Rolling(RollingConfig::default()),
            new_version: "v2".to_string(),
        };
        start_rollout(
            State(state.clone()),
            Path("prod/api".to_string()),
            Json(req),
        )
        .await;

        // Pause.
        let resp = pause_rollout(
            State(state.clone()),
            Path("prod/api".to_string()),
        )
        .await;
        assert_eq!(resp.into_response().status(), StatusCode::OK);

        {
            let rollouts = state.rollouts.read().await;
            assert_eq!(rollouts["prod/api"].phase, RolloutPhase::Paused);
        }

        // Resume.
        let resp = resume_rollout(
            State(state.clone()),
            Path("prod/api".to_string()),
        )
        .await;
        assert_eq!(resp.into_response().status(), StatusCode::OK);

        {
            let rollouts = state.rollouts.read().await;
            assert_eq!(rollouts["prod/api"].phase, RolloutPhase::HealthGate);
        }
    }

    #[tokio::test]
    async fn canary_rollout_starts_observing() {
        let state = test_state();
        let spec = test_deployment("prod", "svc");
        state.store.put_deployment(&spec).unwrap();

        let req = StartRolloutRequest {
            strategy: RolloutStrategy::Canary(CanaryConfig::default()),
            new_version: "v2".to_string(),
        };

        start_rollout(
            State(state.clone()),
            Path("prod/svc".to_string()),
            Json(req),
        )
        .await;

        let rollouts = state.rollouts.read().await;
        assert_eq!(rollouts["prod/svc"].phase, RolloutPhase::CanaryObserving);
    }

    #[tokio::test]
    async fn blue_green_rollout_starts_health_gate() {
        let state = test_state();
        let spec = test_deployment("prod", "web");
        state.store.put_deployment(&spec).unwrap();

        let req = StartRolloutRequest {
            strategy: RolloutStrategy::BlueGreen,
            new_version: "v2".to_string(),
        };

        start_rollout(
            State(state.clone()),
            Path("prod/web".to_string()),
            Json(req),
        )
        .await;

        let rollouts = state.rollouts.read().await;
        assert_eq!(rollouts["prod/web"].phase, RolloutPhase::HealthGate);
    }
}
