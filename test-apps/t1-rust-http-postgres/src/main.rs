//! T1 reference application: Rust axum HTTP handler with Postgres.
//!
//! This is the reference Rust application for the T1 integration test.
//! When compiled to `wasm32-wasip2` with the patched wasi-libc sysroot,
//! DNS resolution routes through the WarpGrid DNS shim and database
//! connections flow through the WarpGrid database proxy shim.
//!
//! Build (native): cargo run
//! Build (Wasm):   cargo build --target wasm32-wasip2  (requires patched wasi-libc)
//!
//! The HTTP handler implements:
//!   GET  /users     — returns all users as JSON
//!   POST /users     — creates a new user, returns 201
//!
//! Database connectivity flows through the WarpGrid shim chain:
//!   sqlx::PgPool::connect("postgres://testuser@db.test.warp.local:5432/testdb")
//!     → DNS shim resolves "db.test.warp.local" to service registry IP
//!     → connect() routed through database proxy shim
//!     → send/recv pass raw Postgres wire protocol bytes through proxy

use std::sync::Arc;

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// A row in the test_users table.
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct User {
    id: i32,
    name: String,
}

/// POST request body for creating a user.
#[derive(Debug, Deserialize)]
struct CreateUser {
    name: String,
}

/// Application state shared across handlers.
struct AppState {
    pool: PgPool,
}

async fn get_users(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match sqlx::query_as::<_, User>("SELECT id, name FROM test_users ORDER BY id")
        .fetch_all(&state.pool)
        .await
    {
        Ok(users) => (StatusCode::OK, Json(users)).into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("db error: {e}"),
        )
            .into_response(),
    }
}

async fn post_user(
    State(state): State<Arc<AppState>>,
    Json(input): Json<CreateUser>,
) -> impl IntoResponse {
    match sqlx::query_as::<_, User>(
        "INSERT INTO test_users (name) VALUES ($1) RETURNING id, name",
    )
    .bind(&input.name)
    .fetch_one(&state.pool)
    .await
    {
        Ok(user) => (StatusCode::CREATED, Json(user)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("insert error: {e}"),
        )
            .into_response(),
    }
}

#[tokio::main]
async fn main() {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://testuser@db.test.warp.local:5432/testdb".to_string());

    let pool = PgPool::connect(&database_url)
        .await
        .expect("failed to connect to database");

    let state = Arc::new(AppState { pool });

    let app = Router::new()
        .route("/users", get(get_users).post(post_user))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
        .await
        .expect("failed to bind");

    tracing::info!("listening on :8080");
    axum::serve(listener, app).await.expect("server error");
}
