//! REST API handlers for sprite (lightweight Linux VM) management.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use warpgrid_state::*;

use crate::ApiState;

/// Response wrapper for consistent API format.
#[derive(serde::Serialize)]
struct ApiResponse<T: serde::Serialize> {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl<T: serde::Serialize> ApiResponse<T> {
    fn ok(data: T) -> Json<Self> {
        Json(Self {
            success: true,
            data: Some(data),
            error: None,
        })
    }
}

fn error_response(msg: &str, status: StatusCode) -> impl IntoResponse {
    (
        status,
        Json(ApiResponse::<()> {
            success: false,
            data: None,
            error: Some(msg.to_string()),
        }),
    )
}

// ── Sprites ─────────────────────────────────────────────────────

/// POST /api/v1/sprites — create request body.
#[derive(serde::Deserialize)]
pub struct CreateSpriteRequest {
    pub owner: String,
    pub name: String,
    #[serde(default)]
    pub resources: SpriteResources,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

/// GET /api/v1/sprites
pub async fn list_sprites(State(state): State<ApiState>) -> impl IntoResponse {
    match state.store.list_sprites() {
        Ok(sprites) => ApiResponse::ok(sprites).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

/// POST /api/v1/sprites
pub async fn create_sprite(
    State(state): State<ApiState>,
    Json(req): Json<CreateSpriteRequest>,
) -> impl IntoResponse {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let sprite_id = format!("sprite-{now}");
    let spec = SpriteSpec {
        id: sprite_id,
        owner: req.owner,
        name: req.name,
        image_version: "latest".to_string(),
        resources: req.resources,
        storage_url: String::new(),
        checkpoint_id: None,
        status: SpriteStatus::Creating,
        node_id: None,
        created_at: now,
        last_active_at: now,
    };

    match state.store.put_sprite(&spec) {
        Ok(()) => (StatusCode::CREATED, ApiResponse::ok(spec)).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

/// GET /api/v1/sprites/:id
pub async fn get_sprite(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.get_sprite(&id) {
        Ok(Some(spec)) => ApiResponse::ok(spec).into_response(),
        Ok(None) => error_response("sprite not found", StatusCode::NOT_FOUND).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

/// DELETE /api/v1/sprites/:id
pub async fn delete_sprite(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.delete_sprite(&id) {
        Ok(true) => ApiResponse::ok("deleted").into_response(),
        Ok(false) => error_response("sprite not found", StatusCode::NOT_FOUND).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

/// POST /api/v1/sprites/:id/wake
pub async fn wake_sprite(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.get_sprite(&id) {
        Ok(Some(mut spec)) => {
            if spec.status != SpriteStatus::Paused && spec.status != SpriteStatus::Sleeping {
                return error_response(
                    &format!("cannot wake sprite in {:?} state", spec.status),
                    StatusCode::CONFLICT,
                )
                .into_response();
            }
            spec.status = SpriteStatus::Running;
            spec.last_active_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let _ = state.store.put_sprite(&spec);
            ApiResponse::ok(spec).into_response()
        }
        Ok(None) => error_response("sprite not found", StatusCode::NOT_FOUND).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

/// POST /api/v1/sprites/:id/sleep
pub async fn sleep_sprite(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.get_sprite(&id) {
        Ok(Some(mut spec)) => {
            if spec.status != SpriteStatus::Running && spec.status != SpriteStatus::Paused {
                return error_response(
                    &format!("cannot sleep sprite in {:?} state", spec.status),
                    StatusCode::CONFLICT,
                )
                .into_response();
            }
            spec.status = SpriteStatus::Sleeping;
            spec.node_id = None;
            let _ = state.store.put_sprite(&spec);
            ApiResponse::ok(spec).into_response()
        }
        Ok(None) => error_response("sprite not found", StatusCode::NOT_FOUND).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

/// POST /api/v1/sprites/:id/checkpoint
pub async fn checkpoint_sprite(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.get_sprite(&id) {
        Ok(Some(spec)) => {
            if spec.status != SpriteStatus::Running && spec.status != SpriteStatus::Paused {
                return error_response(
                    &format!("cannot checkpoint sprite in {:?} state", spec.status),
                    StatusCode::CONFLICT,
                )
                .into_response();
            }
            // In a full implementation, this would trigger the checkpoint manager.
            // For now, record intent in the API response.
            ApiResponse::ok(serde_json::json!({
                "sprite_id": id,
                "status": "checkpoint_initiated",
            }))
            .into_response()
        }
        Ok(None) => error_response("sprite not found", StatusCode::NOT_FOUND).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

/// Exec request body.
#[derive(serde::Deserialize)]
pub struct ExecRequest {
    pub command: String,
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

/// POST /api/v1/sprites/:id/exec
pub async fn exec_in_sprite(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(req): Json<ExecRequest>,
) -> impl IntoResponse {
    match state.store.get_sprite(&id) {
        Ok(Some(spec)) => {
            if spec.status != SpriteStatus::Running {
                return error_response(
                    &format!("cannot exec in sprite with {:?} status", spec.status),
                    StatusCode::CONFLICT,
                )
                .into_response();
            }
            // In a full implementation, this would send the command via vsock.
            ApiResponse::ok(serde_json::json!({
                "sprite_id": id,
                "command": req.command,
                "status": "exec_initiated",
            }))
            .into_response()
        }
        Ok(None) => error_response("sprite not found", StatusCode::NOT_FOUND).into_response(),
        Err(e) => error_response(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;

    fn test_state() -> ApiState {
        let store = StateStore::open_in_memory().unwrap();
        ApiState { store }
    }

    fn test_sprite_spec(id: &str, owner: &str) -> SpriteSpec {
        SpriteSpec {
            id: id.to_string(),
            owner: owner.to_string(),
            name: format!("{id}-workspace"),
            image_version: "latest".to_string(),
            resources: SpriteResources::default(),
            storage_url: String::new(),
            checkpoint_id: None,
            status: SpriteStatus::Running,
            node_id: Some("node-1".to_string()),
            created_at: 1000,
            last_active_at: 1000,
        }
    }

    #[tokio::test]
    async fn list_sprites_empty() {
        let state = test_state();
        let resp = list_sprites(State(state)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_and_get_sprite() {
        let state = test_state();
        let req = CreateSpriteRequest {
            owner: "alice".to_string(),
            name: "dev-workspace".to_string(),
            resources: SpriteResources::default(),
            env: std::collections::HashMap::new(),
        };

        let resp = create_sprite(State(state.clone()), Json(req)).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // List should show one sprite.
        let sprites = state.store.list_sprites().unwrap();
        assert_eq!(sprites.len(), 1);
        assert_eq!(sprites[0].owner, "alice");
    }

    #[tokio::test]
    async fn get_nonexistent_sprite() {
        let state = test_state();
        let resp = get_sprite(State(state), Path("nope".to_string())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_sprite_exists() {
        let state = test_state();
        state
            .store
            .put_sprite(&test_sprite_spec("s1", "alice"))
            .unwrap();

        let resp = delete_sprite(State(state), Path("s1".to_string())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn wake_paused_sprite() {
        let state = test_state();
        let mut spec = test_sprite_spec("s1", "alice");
        spec.status = SpriteStatus::Paused;
        state.store.put_sprite(&spec).unwrap();

        let resp = wake_sprite(State(state.clone()), Path("s1".to_string())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::OK);

        let updated = state.store.get_sprite("s1").unwrap().unwrap();
        assert_eq!(updated.status, SpriteStatus::Running);
    }

    #[tokio::test]
    async fn sleep_running_sprite() {
        let state = test_state();
        let spec = test_sprite_spec("s1", "alice");
        state.store.put_sprite(&spec).unwrap();

        let resp = sleep_sprite(State(state.clone()), Path("s1".to_string())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::OK);

        let updated = state.store.get_sprite("s1").unwrap().unwrap();
        assert_eq!(updated.status, SpriteStatus::Sleeping);
        assert!(updated.node_id.is_none());
    }

    #[tokio::test]
    async fn wake_running_sprite_fails() {
        let state = test_state();
        let spec = test_sprite_spec("s1", "alice");
        state.store.put_sprite(&spec).unwrap();

        let resp = wake_sprite(State(state), Path("s1".to_string())).await;
        let resp = resp.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }
}
