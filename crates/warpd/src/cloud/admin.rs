//! Admin dashboard — server-rendered web UI for platform operators.
//!
//! Serves HTML pages at `/admin/*` that provide a global view of the
//! platform: all users, nodes, deployments, and metrics across every
//! region. Uses raw HTML strings with inline CSS (same pattern as console.rs).
//!
//! Authentication is cookie-based via `wg_admin_session`. The admin key
//! is either set explicitly via `WARPGRID_ADMIN_KEY` or, as a bootstrap
//! fallback, any valid API key belonging to the first registered user is
//! accepted.

use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect};
use axum::routing::{get, post};
use serde::Deserialize;

use super::routes::CloudState;

// ── Types ──────────────────────────────────────────────────────

type AdminState = Arc<AdminInner>;

struct AdminInner {
    cloud: CloudState,
    admin_key: Option<String>,
}

#[derive(Deserialize)]
struct AdminLoginForm {
    api_key: String,
}

// ── Router ─────────────────────────────────────────────────────

/// Build the admin router with all dashboard routes.
pub fn admin_router(cloud_state: CloudState) -> Router {
    let admin_key = cloud_state.admin_key.clone();
    let inner = Arc::new(AdminInner {
        cloud: cloud_state,
        admin_key,
    });

    Router::new()
        .route("/admin/", get(admin_overview))
        .route("/admin/login", get(admin_login_page))
        .route("/admin/login", post(admin_login_submit))
        .route("/admin/logout", post(admin_logout))
        .route("/admin/nodes", get(admin_nodes))
        .route("/admin/users", get(admin_users))
        .route("/admin/deployments", get(admin_deployments))
        .route("/admin/metrics", get(admin_metrics))
        .with_state(inner)
}

// ── Session helpers ────────────────────────────────────────────

fn extract_admin_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get("cookie")?.to_str().ok()?;
    cookie_header
        .split(';')
        .map(|s| s.trim())
        .find(|s| s.starts_with("wg_admin_session="))
        .and_then(|s| s.strip_prefix("wg_admin_session="))
        .map(|s| s.to_string())
}

/// Validate that the admin session cookie contains a valid admin key.
///
/// If `WARPGRID_ADMIN_KEY` is configured, only that exact key is accepted.
/// Otherwise, as a bootstrap fallback, any valid API key belonging to the
/// first registered user (by `created_at`) is accepted.
async fn require_admin(headers: &HeaderMap, state: &AdminInner) -> Result<(), Redirect> {
    let session_key = extract_admin_cookie(headers).ok_or(Redirect::to("/admin/login"))?;

    if let Some(ref configured_key) = state.admin_key {
        if session_key == *configured_key {
            return Ok(());
        }
        return Err(Redirect::to("/admin/login"));
    }

    // Bootstrap fallback: accept any valid API key belonging to the first user.
    let user = state
        .cloud
        .auth
        .validate_sync(&session_key)
        .ok_or(Redirect::to("/admin/login"))?;

    let is_first = is_first_user(&state.cloud, &user.id).await;
    if is_first {
        Ok(())
    } else {
        Err(Redirect::to("/admin/login"))
    }
}

/// Check whether the given user id is the first registered user.
async fn is_first_user(cloud: &CloudState, user_id: &str) -> bool {
    let result = cloud
        .cloud_db
        .query(
            "SELECT id FROM cloud_users ORDER BY created_at ASC LIMIT 1",
            (),
        )
        .await;
    match result {
        Ok(mut rows) => match rows.next().await {
            Ok(Some(row)) => row.get::<String>(0).ok().as_deref() == Some(user_id),
            _ => false,
        },
        Err(_) => false,
    }
}

/// Validate a submitted key for the login form.
async fn validate_admin_key(state: &AdminInner, key: &str) -> bool {
    if let Some(ref configured_key) = state.admin_key {
        return key == *configured_key;
    }

    // Bootstrap: accept any valid key for the first registered user.
    if let Some(user) = state.cloud.auth.validate_sync(key) {
        return is_first_user(&state.cloud, &user.id).await;
    }

    false
}

// ── Shared HTML fragments ──────────────────────────────────────

const ADMIN_CSS: &str = r#"
    * { margin: 0; padding: 0; box-sizing: border-box; }
    body { background: #0a0a0a; color: #e0e0e0; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, monospace; }
    .layout { display: flex; min-height: 100vh; }
    .sidebar { width: 220px; background: #111111; border-right: 1px solid #222; padding: 24px 0; flex-shrink: 0; }
    .sidebar .brand { padding: 0 20px 24px; border-bottom: 1px solid #222; margin-bottom: 16px; }
    .sidebar .brand h1 { font-size: 18px; color: #00ff88; font-weight: 700; letter-spacing: -0.5px; }
    .sidebar .brand span { font-size: 11px; color: #666; }
    .sidebar nav a { display: block; padding: 10px 20px; color: #888; text-decoration: none; font-size: 14px; transition: all 0.15s; }
    .sidebar nav a:hover { color: #e0e0e0; background: #1a1a1a; }
    .sidebar nav a.active { color: #00ff88; background: #1a1a1a; border-right: 2px solid #00ff88; }
    .main { flex: 1; padding: 32px 40px; overflow-y: auto; }
    .main h2 { font-size: 22px; font-weight: 600; margin-bottom: 24px; }
    .card { background: #111111; border: 1px solid #222; border-radius: 8px; padding: 20px; margin-bottom: 16px; }
    .card h3 { font-size: 14px; color: #666; text-transform: uppercase; letter-spacing: 0.5px; margin-bottom: 12px; }
    .stat { font-size: 32px; font-weight: 700; color: #00ff88; }
    .stat-label { font-size: 12px; color: #666; margin-top: 4px; }
    .grid-2 { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }
    .grid-3 { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 16px; }
    .grid-4 { display: grid; grid-template-columns: 1fr 1fr 1fr 1fr; gap: 16px; }
    table { width: 100%; border-collapse: collapse; }
    th { text-align: left; padding: 10px 12px; color: #666; font-size: 12px; text-transform: uppercase; letter-spacing: 0.5px; border-bottom: 1px solid #222; }
    td { padding: 12px; border-bottom: 1px solid #1a1a1a; font-size: 14px; }
    .badge { display: inline-block; padding: 2px 8px; border-radius: 4px; font-size: 11px; font-weight: 600; }
    .badge-green { background: #00ff8815; color: #00ff88; }
    .badge-yellow { background: #ffaa0015; color: #ffaa00; }
    .badge-red { background: #ff444415; color: #ff4444; }
    .badge-gray { background: #44444430; color: #888; }
    .btn { display: inline-block; padding: 8px 16px; border-radius: 6px; font-size: 13px; font-weight: 600; cursor: pointer; border: none; text-decoration: none; transition: all 0.15s; }
    .btn-primary { background: #00ff88; color: #0a0a0a; }
    .btn-primary:hover { background: #00cc6e; }
    .btn-danger { background: #ff444420; color: #ff4444; border: 1px solid #ff444440; }
    .btn-danger:hover { background: #ff444440; }
    input { background: #1a1a1a; border: 1px solid #222; border-radius: 6px; padding: 10px 14px; color: #e0e0e0; font-size: 14px; width: 100%; font-family: inherit; }
    input:focus { outline: none; border-color: #00ff88; }
    label { display: block; font-size: 13px; color: #888; margin-bottom: 6px; }
    .form-group { margin-bottom: 16px; }
    .footer { padding: 16px 20px; color: #444; font-size: 11px; border-top: 1px solid #222; margin-top: auto; }
    .mono { font-family: 'SF Mono', 'Fira Code', monospace; font-size: 13px; }
    .text-muted { color: #666; }
    .text-green { color: #00ff88; }
    .text-red { color: #ff4444; }
    .text-yellow { color: #ffaa00; }
    .mb-4 { margin-bottom: 16px; }
"#;

fn admin_nav_link(href: &str, label: &str, active_page: &str, page_id: &str) -> String {
    let class = if active_page == page_id { "active" } else { "" };
    format!(r#"<a href="{href}" class="{class}">{label}</a>"#)
}

fn admin_page_shell(title: &str, active_page: &str, content: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} — WarpGrid Admin</title>
  <style>{ADMIN_CSS}</style>
</head>
<body>
  <div class="layout">
    <aside class="sidebar">
      <div class="brand">
        <h1>WarpGrid Admin</h1>
        <span>Platform Dashboard</span>
      </div>
      <nav>
        {nav_overview}
        {nav_nodes}
        {nav_users}
        {nav_deployments}
        {nav_metrics}
      </nav>
      <div class="footer">
        <form method="POST" action="/admin/logout" style="display:inline">
          <button type="submit" class="btn btn-danger" style="padding:4px 10px;font-size:11px">Logout</button>
        </form>
        <div style="margin-top:12px">Served by WarpGrid</div>
      </div>
    </aside>
    <main class="main">
      {content}
    </main>
  </div>
</body>
</html>"#,
        title = title,
        nav_overview = admin_nav_link("/admin/", "Overview", active_page, "overview"),
        nav_nodes = admin_nav_link("/admin/nodes", "Nodes", active_page, "nodes"),
        nav_users = admin_nav_link("/admin/users", "Users", active_page, "users"),
        nav_deployments = admin_nav_link(
            "/admin/deployments",
            "Deployments",
            active_page,
            "deployments"
        ),
        nav_metrics = admin_nav_link("/admin/metrics", "Metrics", active_page, "metrics"),
        content = content,
    )
}

// ── Page handlers ──────────────────────────────────────────────

async fn admin_overview(
    axum::extract::State(state): axum::extract::State<AdminState>,
    headers: HeaderMap,
) -> Result<Html<String>, Redirect> {
    require_admin(&headers, &state).await?;

    let cloud = &state.cloud;

    // Count users.
    let user_count = count_query(&cloud.cloud_db, "SELECT COUNT(*) FROM cloud_users").await;

    // Count teams.
    let team_count = count_query(&cloud.cloud_db, "SELECT COUNT(*) FROM cloud_teams").await;

    // Count deployments.
    let deployment_count =
        count_query(&cloud.cloud_db, "SELECT COUNT(*) FROM cloud_deployments").await;

    // Count running instances.
    let instance_count = count_query(&cloud.cloud_db, "SELECT COUNT(*) FROM cloud_instances").await;

    // Nodes by region.
    let nodes_by_region = nodes_by_region_summary(&cloud.cloud_db).await;

    let content = format!(
        r#"<h2>Overview</h2>
<div class="grid-4">
  <div class="card">
    <h3>Users</h3>
    <div class="stat">{user_count}</div>
    <div class="stat-label">registered</div>
  </div>
  <div class="card">
    <h3>Teams</h3>
    <div class="stat">{team_count}</div>
    <div class="stat-label">created</div>
  </div>
  <div class="card">
    <h3>Deployments</h3>
    <div class="stat">{deployment_count}</div>
    <div class="stat-label">total</div>
  </div>
  <div class="card">
    <h3>Instances</h3>
    <div class="stat">{instance_count}</div>
    <div class="stat-label">running</div>
  </div>
</div>

<div class="card" style="margin-top:8px">
  <h3>Nodes by Region</h3>
  {nodes_by_region}
</div>"#,
    );

    Ok(Html(admin_page_shell("Overview", "overview", &content)))
}

async fn admin_nodes(
    axum::extract::State(state): axum::extract::State<AdminState>,
    headers: HeaderMap,
) -> Result<Html<String>, Redirect> {
    require_admin(&headers, &state).await?;

    let cloud = &state.cloud;
    let rows_html = build_nodes_table(&cloud.cloud_db).await;

    let content = format!(
        r#"<h2>Nodes</h2>
<div class="card">
  <table>
    <thead>
      <tr>
        <th>ID</th>
        <th>Region</th>
        <th>Address</th>
        <th>Memory (Used / Cap)</th>
        <th>CPU (Used / Cap)</th>
        <th>Last Heartbeat</th>
      </tr>
    </thead>
    <tbody>{rows_html}</tbody>
  </table>
</div>"#,
    );

    Ok(Html(admin_page_shell("Nodes", "nodes", &content)))
}

async fn admin_users(
    axum::extract::State(state): axum::extract::State<AdminState>,
    headers: HeaderMap,
) -> Result<Html<String>, Redirect> {
    require_admin(&headers, &state).await?;

    let cloud = &state.cloud;
    let rows_html = build_users_table(&cloud.cloud_db).await;

    let content = format!(
        r#"<h2>Users</h2>
<div class="card">
  <table>
    <thead>
      <tr>
        <th>ID</th>
        <th>Email</th>
        <th>Namespace</th>
        <th>Created At</th>
      </tr>
    </thead>
    <tbody>{rows_html}</tbody>
  </table>
</div>"#,
    );

    Ok(Html(admin_page_shell("Users", "users", &content)))
}

async fn admin_deployments(
    axum::extract::State(state): axum::extract::State<AdminState>,
    headers: HeaderMap,
) -> Result<Html<String>, Redirect> {
    require_admin(&headers, &state).await?;

    let cloud = &state.cloud;
    let rows_html = build_deployments_table(&cloud.cloud_db).await;

    let content = format!(
        r#"<h2>Deployments</h2>
<div class="card">
  <table>
    <thead>
      <tr>
        <th>ID</th>
        <th>Namespace</th>
        <th>Name</th>
        <th>Region</th>
        <th>Status</th>
        <th>Wasm Hash</th>
      </tr>
    </thead>
    <tbody>{rows_html}</tbody>
  </table>
</div>"#,
    );

    Ok(Html(admin_page_shell(
        "Deployments",
        "deployments",
        &content,
    )))
}

async fn admin_metrics(
    axum::extract::State(state): axum::extract::State<AdminState>,
    headers: HeaderMap,
) -> Result<Html<String>, Redirect> {
    require_admin(&headers, &state).await?;

    let cloud = &state.cloud;
    let rows_html = build_metrics_table(&cloud.cloud_db).await;

    let content = format!(
        r#"<h2>Metrics</h2>
<p class="text-muted mb-4" style="font-size:13px">Latest metrics snapshot per deployment per region.</p>
<div class="card">
  <table>
    <thead>
      <tr>
        <th>Deployment</th>
        <th>Region</th>
        <th>RPS</th>
        <th>P50 (ms)</th>
        <th>P99 (ms)</th>
        <th>Error Rate</th>
      </tr>
    </thead>
    <tbody>{rows_html}</tbody>
  </table>
</div>"#,
    );

    Ok(Html(admin_page_shell("Metrics", "metrics", &content)))
}

// ── Login / Logout ─────────────────────────────────────────────

async fn admin_login_page() -> Html<String> {
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Login — WarpGrid Admin</title>
  <style>{ADMIN_CSS}
    .login-container {{ display: flex; align-items: center; justify-content: center; min-height: 100vh; }}
    .login-box {{ width: 400px; }}
  </style>
</head>
<body>
  <div class="login-container">
    <div class="login-box">
      <div style="text-align:center;margin-bottom:32px">
        <h1 style="color:#00ff88;font-size:28px;margin-bottom:4px">WarpGrid Admin</h1>
        <span style="color:#666;font-size:14px">Platform Dashboard</span>
      </div>
      <div class="card">
        <h3>Admin Sign In</h3>
        <form method="POST" action="/admin/login">
          <div class="form-group">
            <label>Admin Key</label>
            <input type="password" name="api_key" placeholder="admin key or bootstrap API key" required autofocus>
          </div>
          <button type="submit" class="btn btn-primary" style="width:100%">Sign In</button>
        </form>
      </div>
      <div style="text-align:center;margin-top:16px;color:#444;font-size:11px">
        Served by WarpGrid
      </div>
    </div>
  </div>
</body>
</html>"#,
    );
    Html(html)
}

async fn admin_login_submit(
    axum::extract::State(state): axum::extract::State<AdminState>,
    axum::Form(form): axum::Form<AdminLoginForm>,
) -> impl IntoResponse {
    let api_key = form.api_key.trim().to_string();

    if !validate_admin_key(&state, &api_key).await {
        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Login — WarpGrid Admin</title>
  <style>{ADMIN_CSS}
    .login-container {{ display: flex; align-items: center; justify-content: center; min-height: 100vh; }}
    .login-box {{ width: 400px; }}
  </style>
</head>
<body>
  <div class="login-container">
    <div class="login-box">
      <div style="text-align:center;margin-bottom:32px">
        <h1 style="color:#00ff88;font-size:28px;margin-bottom:4px">WarpGrid Admin</h1>
        <span style="color:#666;font-size:14px">Platform Dashboard</span>
      </div>
      <div class="card">
        <div style="background:#ff444415;border:1px solid #ff444440;border-radius:6px;padding:10px 14px;margin-bottom:16px;color:#ff4444;font-size:13px">
          Invalid admin key. Please try again.
        </div>
        <h3>Admin Sign In</h3>
        <form method="POST" action="/admin/login">
          <div class="form-group">
            <label>Admin Key</label>
            <input type="password" name="api_key" placeholder="admin key or bootstrap API key" required autofocus>
          </div>
          <button type="submit" class="btn btn-primary" style="width:100%">Sign In</button>
        </form>
      </div>
      <div style="text-align:center;margin-top:16px;color:#444;font-size:11px">
        Served by WarpGrid
      </div>
    </div>
  </div>
</body>
</html>"#,
        );
        return (StatusCode::UNAUTHORIZED, HeaderMap::new(), Html(html));
    }

    let mut response_headers = HeaderMap::new();
    let cookie_value =
        format!("wg_admin_session={api_key}; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800");
    response_headers.insert(
        "set-cookie",
        cookie_value.parse().expect("valid cookie header"),
    );
    response_headers.insert(
        "location",
        "/admin/".parse().expect("valid location header"),
    );

    (StatusCode::SEE_OTHER, response_headers, Html(String::new()))
}

async fn admin_logout() -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    let cookie_value = "wg_admin_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0";
    headers.insert(
        "set-cookie",
        cookie_value.parse().expect("valid cookie header"),
    );
    headers.insert(
        "location",
        "/admin/login".parse().expect("valid location header"),
    );

    (StatusCode::SEE_OTHER, headers, Html(String::new()))
}

// ── Data helpers ───────────────────────────────────────────────

async fn count_query(conn: &libsql::Connection, sql: &str) -> i64 {
    match conn.query(sql, ()).await {
        Ok(mut rows) => match rows.next().await {
            Ok(Some(row)) => row.get::<i64>(0).unwrap_or(0),
            _ => 0,
        },
        Err(_) => 0,
    }
}

async fn nodes_by_region_summary(conn: &libsql::Connection) -> String {
    let result = conn
        .query(
            "SELECT region, COUNT(*) as cnt FROM cloud_nodes GROUP BY region ORDER BY region",
            (),
        )
        .await;

    let mut rows_data: Vec<(String, i64)> = Vec::new();
    if let Ok(mut rows) = result {
        while let Ok(Some(row)) = rows.next().await {
            let region: String = row.get(0).unwrap_or_default();
            let count: i64 = row.get(1).unwrap_or(0);
            rows_data.push((region, count));
        }
    }

    if rows_data.is_empty() {
        return r#"<div style="color:#666;padding:16px">No nodes registered yet.</div>"#
            .to_string();
    }

    let table_rows: String = rows_data
        .iter()
        .map(|(region, count)| {
            format!(
                r#"<tr><td class="mono">{region}</td><td>{count}</td><td><span class="badge badge-green">online</span></td></tr>"#,
            )
        })
        .collect();

    format!(
        r#"<table>
  <thead><tr><th>Region</th><th>Nodes</th><th>Status</th></tr></thead>
  <tbody>{table_rows}</tbody>
</table>"#,
    )
}

async fn build_nodes_table(conn: &libsql::Connection) -> String {
    let result = conn
        .query(
            "SELECT id, region, address, capacity_memory_bytes, capacity_cpu_weight, used_memory_bytes, used_cpu_weight, last_heartbeat FROM cloud_nodes ORDER BY region, id",
            (),
        )
        .await;

    let mut html = String::new();
    if let Ok(mut rows) = result {
        while let Ok(Some(row)) = rows.next().await {
            let id: String = row.get(0).unwrap_or_default();
            let region: String = row.get(1).unwrap_or_default();
            let address: String = row.get(2).unwrap_or_default();
            let cap_mem: i64 = row.get(3).unwrap_or(0);
            let cap_cpu: i64 = row.get(4).unwrap_or(0);
            let used_mem: i64 = row.get(5).unwrap_or(0);
            let used_cpu: i64 = row.get(6).unwrap_or(0);
            let heartbeat: i64 = row.get(7).unwrap_or(0);

            let heartbeat_str = format_epoch(heartbeat);

            html.push_str(&format!(
                r#"<tr>
  <td class="mono">{id}</td>
  <td><span class="badge badge-green">{region}</span></td>
  <td class="mono">{address}</td>
  <td>{used_mem_fmt} / {cap_mem_fmt}</td>
  <td>{used_cpu} / {cap_cpu}</td>
  <td class="text-muted">{heartbeat_str}</td>
</tr>"#,
                used_mem_fmt = format_bytes_i64(used_mem),
                cap_mem_fmt = format_bytes_i64(cap_mem),
            ));
        }
    }

    if html.is_empty() {
        return r#"<tr><td colspan="6" style="color:#666;text-align:center;padding:24px">No nodes registered.</td></tr>"#.to_string();
    }

    html
}

async fn build_users_table(conn: &libsql::Connection) -> String {
    let result = conn
        .query(
            "SELECT id, email, namespace, created_at FROM cloud_users ORDER BY created_at ASC",
            (),
        )
        .await;

    let mut html = String::new();
    if let Ok(mut rows) = result {
        while let Ok(Some(row)) = rows.next().await {
            let id: String = row.get(0).unwrap_or_default();
            let email: String = row.get(1).unwrap_or_default();
            let namespace: String = row.get(2).unwrap_or_default();
            let created_at: i64 = row.get(3).unwrap_or(0);

            let created_str = format_epoch(created_at);

            html.push_str(&format!(
                r#"<tr>
  <td class="mono">{id}</td>
  <td>{email}</td>
  <td class="mono" style="color:#00ff88">{namespace}</td>
  <td class="text-muted">{created_str}</td>
</tr>"#,
            ));
        }
    }

    if html.is_empty() {
        return r#"<tr><td colspan="4" style="color:#666;text-align:center;padding:24px">No users registered.</td></tr>"#.to_string();
    }

    html
}

async fn build_deployments_table(conn: &libsql::Connection) -> String {
    let result = conn
        .query(
            "SELECT id, namespace, name, region, status, wasm_hash FROM cloud_deployments ORDER BY namespace, name",
            (),
        )
        .await;

    let mut html = String::new();
    if let Ok(mut rows) = result {
        while let Ok(Some(row)) = rows.next().await {
            let id: String = row.get(0).unwrap_or_default();
            let namespace: String = row.get(1).unwrap_or_default();
            let name: String = row.get(2).unwrap_or_default();
            let region: String = row.get(3).unwrap_or_default();
            let status: String = row.get(4).unwrap_or_default();
            let wasm_hash: String = row.get(5).unwrap_or_default();

            let status_badge = match status.as_str() {
                "active" => r#"<span class="badge badge-green">active</span>"#,
                "stopped" => r#"<span class="badge badge-gray">stopped</span>"#,
                "error" => r#"<span class="badge badge-red">error</span>"#,
                _ => r#"<span class="badge badge-yellow">unknown</span>"#,
            };

            let hash_short = if wasm_hash.len() > 12 {
                format!("{}...", &wasm_hash[..12])
            } else {
                wasm_hash
            };

            html.push_str(&format!(
                r#"<tr>
  <td class="mono">{id}</td>
  <td class="mono" style="color:#00ff88">{namespace}</td>
  <td>{name}</td>
  <td><span class="badge badge-green">{region}</span></td>
  <td>{status_badge}</td>
  <td class="mono text-muted">{hash_short}</td>
</tr>"#,
            ));
        }
    }

    if html.is_empty() {
        return r#"<tr><td colspan="6" style="color:#666;text-align:center;padding:24px">No deployments found.</td></tr>"#.to_string();
    }

    html
}

async fn build_metrics_table(conn: &libsql::Connection) -> String {
    // Get the latest metrics per deployment per region by using MAX(epoch).
    let result = conn
        .query(
            "SELECT m.deployment_id, m.region, m.rps, m.latency_p50_ms, m.latency_p99_ms, m.error_rate \
             FROM cloud_metrics m \
             INNER JOIN (SELECT deployment_id, region, MAX(epoch) AS max_epoch FROM cloud_metrics GROUP BY deployment_id, region) latest \
             ON m.deployment_id = latest.deployment_id AND m.region = latest.region AND m.epoch = latest.max_epoch \
             ORDER BY m.deployment_id, m.region",
            (),
        )
        .await;

    let mut html = String::new();
    if let Ok(mut rows) = result {
        while let Ok(Some(row)) = rows.next().await {
            let deployment_id: String = row.get(0).unwrap_or_default();
            let region: String = row.get(1).unwrap_or_default();
            let rps: f64 = row.get(2).unwrap_or(0.0);
            let p50: f64 = row.get(3).unwrap_or(0.0);
            let p99: f64 = row.get(4).unwrap_or(0.0);
            let error_rate: f64 = row.get(5).unwrap_or(0.0);

            let error_class = if error_rate > 0.05 {
                "text-red"
            } else if error_rate > 0.01 {
                "text-yellow"
            } else {
                "text-green"
            };

            html.push_str(&format!(
                r#"<tr>
  <td class="mono">{deployment_id}</td>
  <td><span class="badge badge-green">{region}</span></td>
  <td>{rps:.1}</td>
  <td>{p50:.1}</td>
  <td>{p99:.1}</td>
  <td class="{error_class}">{error_pct:.2}%</td>
</tr>"#,
                error_pct = error_rate * 100.0,
            ));
        }
    }

    if html.is_empty() {
        return r#"<tr><td colspan="6" style="color:#666;text-align:center;padding:24px">No metrics data yet.</td></tr>"#.to_string();
    }

    html
}

// ── Formatting helpers ─────────────────────────────────────────

fn format_bytes_i64(bytes: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = 1024 * KB;
    const GB: i64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} B")
    }
}

fn format_epoch(epoch: i64) -> String {
    chrono::DateTime::from_timestamp(epoch, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "--".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::analytics::AnalyticsService;
    use crate::cloud::auth::AuthStore;
    use crate::cloud::billing::BillingService;
    use crate::cloud::domains::DomainStore;
    use crate::cloud::registry::WasmRegistry;
    use crate::cloud::teams::TeamStore;
    use crate::cloud::usage::UsageTracker;

    async fn test_cloud_state() -> CloudState {
        let state_store = warpgrid_state::StateStore::open_in_memory().unwrap();
        let auth = AuthStore::new();
        let registry = WasmRegistry::local(std::path::Path::new("/tmp/wg-test-admin-registry"));
        let db = crate::cloud::db::open_memory().await.unwrap();
        let conn = db.connect().unwrap();
        crate::cloud::db::migrate(&conn).await.unwrap();
        CloudState {
            auth,
            registry,
            state_store,
            cloud_db: conn,
            teams: TeamStore::new(),
            analytics: AnalyticsService::Noop,
            domains: DomainStore::new(),
            billing: BillingService::from_env(None),
            usage: UsageTracker::new(),
            logs: crate::cloud::routes::new_log_buffer(),
            admin_key: None,
        }
    }

    #[test]
    fn extract_admin_cookie_from_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            "wg_admin_session=secret123; other=value".parse().unwrap(),
        );
        assert_eq!(
            extract_admin_cookie(&headers),
            Some("secret123".to_string())
        );
    }

    #[test]
    fn extract_admin_cookie_missing() {
        let headers = HeaderMap::new();
        assert_eq!(extract_admin_cookie(&headers), None);
    }

    #[tokio::test]
    async fn login_page_renders() {
        let html = admin_login_page().await;
        let body = html.0;
        assert!(body.contains("WarpGrid Admin"));
        assert!(body.contains("Admin Sign In"));
        assert!(body.contains("Served by WarpGrid"));
    }

    #[tokio::test]
    async fn require_admin_redirects_without_cookie() {
        let cloud = test_cloud_state().await;
        let inner = AdminInner {
            admin_key: Some("test-admin-key".to_string()),
            cloud,
        };
        let headers = HeaderMap::new();
        let result = require_admin(&headers, &inner).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn require_admin_succeeds_with_valid_key() {
        let cloud = test_cloud_state().await;
        let inner = AdminInner {
            admin_key: Some("test-admin-key".to_string()),
            cloud,
        };
        let mut headers = HeaderMap::new();
        headers.insert("cookie", "wg_admin_session=test-admin-key".parse().unwrap());
        let result = require_admin(&headers, &inner).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn require_admin_rejects_wrong_key() {
        let cloud = test_cloud_state().await;
        let inner = AdminInner {
            admin_key: Some("test-admin-key".to_string()),
            cloud,
        };
        let mut headers = HeaderMap::new();
        headers.insert("cookie", "wg_admin_session=wrong-key".parse().unwrap());
        let result = require_admin(&headers, &inner).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn validate_admin_key_with_configured_key() {
        let cloud = test_cloud_state().await;
        let inner = AdminInner {
            admin_key: Some("my-secret-key".to_string()),
            cloud,
        };
        assert!(validate_admin_key(&inner, "my-secret-key").await);
        assert!(!validate_admin_key(&inner, "wrong-key").await);
    }

    #[tokio::test]
    async fn count_query_returns_zero_on_empty() {
        let cloud = test_cloud_state().await;
        let count = count_query(&cloud.cloud_db, "SELECT COUNT(*) FROM cloud_users").await;
        assert_eq!(count, 0);
    }

    #[test]
    fn format_bytes_i64_units() {
        assert_eq!(format_bytes_i64(0), "0 B");
        assert_eq!(format_bytes_i64(512), "512 B");
        assert_eq!(format_bytes_i64(10 * 1024 * 1024), "10 MB");
        assert_eq!(format_bytes_i64(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    #[test]
    fn format_epoch_valid() {
        let s = format_epoch(1700000000);
        assert!(s.contains("2023"));
        assert!(s.contains("UTC"));
    }

    #[test]
    fn format_epoch_zero() {
        let s = format_epoch(0);
        assert!(s.contains("1970"));
    }
}
