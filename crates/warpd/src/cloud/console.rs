//! Cloud console — server-rendered web UI for the hosted WarpGrid platform.
//!
//! Serves HTML pages at `/console/*` that let users manage deployments,
//! view logs, and configure teams through a browser. Uses raw HTML strings
//! with inline CSS (no Askama templates) to avoid cross-crate template issues.
//!
//! Authentication is cookie-based: the login page accepts an API key,
//! validates it via `AuthStore`, and sets a `wg_session` cookie. All
//! other pages read from the cookie and redirect to login if missing.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;

use super::routes::CloudState;

// ── Types ──────────────────────────────────────────────────────

type ConsoleState = Arc<CloudState>;

#[derive(Deserialize)]
struct LoginForm {
    api_key: String,
}

// ── Router ─────────────────────────────────────────────────────

/// Build the console router with all page routes.
pub fn console_router(state: CloudState) -> Router {
    Router::new()
        .route("/console/", get(console_overview))
        .route("/console/deployments", get(console_deployments))
        .route("/console/deploy", get(console_deploy))
        .route("/console/teams", get(console_teams))
        .route("/console/settings", get(console_settings))
        .route("/console/login", get(console_login_page))
        .route("/console/login", post(console_login_submit))
        .route("/console/logout", post(console_logout))
        .with_state(Arc::new(state))
}

// ── Session helpers ────────────────────────────────────────────

fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get("cookie")?.to_str().ok()?;
    cookie_header
        .split(';')
        .map(|s| s.trim())
        .find(|s| s.starts_with("wg_session="))
        .and_then(|s| s.strip_prefix("wg_session="))
        .map(|s| s.to_string())
}

fn require_user(
    headers: &HeaderMap,
    state: &ConsoleState,
) -> Result<super::auth::User, Redirect> {
    let api_key = extract_session_cookie(headers)
        .ok_or(Redirect::to("/console/login"))?;
    state
        .auth
        .validate(&api_key)
        .ok_or(Redirect::to("/console/login"))
}

// ── Shared HTML fragments ──────────────────────────────────────

const CSS: &str = r#"
    * { margin: 0; padding: 0; box-sizing: border-box; }
    body { background: #0f1117; color: #e2e8f0; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, monospace; }
    .layout { display: flex; min-height: 100vh; }
    .sidebar { width: 220px; background: #161822; border-right: 1px solid #2a2d3a; padding: 24px 0; flex-shrink: 0; }
    .sidebar .brand { padding: 0 20px 24px; border-bottom: 1px solid #2a2d3a; margin-bottom: 16px; }
    .sidebar .brand h1 { font-size: 18px; color: #818cf8; font-weight: 700; letter-spacing: -0.5px; }
    .sidebar .brand span { font-size: 11px; color: #64748b; }
    .sidebar nav a { display: block; padding: 10px 20px; color: #94a3b8; text-decoration: none; font-size: 14px; transition: all 0.15s; }
    .sidebar nav a:hover { color: #e2e8f0; background: #1e2030; }
    .sidebar nav a.active { color: #818cf8; background: #1e2030; border-right: 2px solid #818cf8; }
    .main { flex: 1; padding: 32px 40px; overflow-y: auto; }
    .main h2 { font-size: 22px; font-weight: 600; margin-bottom: 24px; }
    .card { background: #161822; border: 1px solid #2a2d3a; border-radius: 8px; padding: 20px; margin-bottom: 16px; }
    .card h3 { font-size: 14px; color: #64748b; text-transform: uppercase; letter-spacing: 0.5px; margin-bottom: 12px; }
    .stat { font-size: 32px; font-weight: 700; color: #818cf8; }
    .stat-label { font-size: 12px; color: #64748b; margin-top: 4px; }
    .grid-2 { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }
    .grid-3 { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 16px; }
    table { width: 100%; border-collapse: collapse; }
    th { text-align: left; padding: 10px 12px; color: #64748b; font-size: 12px; text-transform: uppercase; letter-spacing: 0.5px; border-bottom: 1px solid #2a2d3a; }
    td { padding: 12px; border-bottom: 1px solid #1e2030; font-size: 14px; }
    .badge { display: inline-block; padding: 2px 8px; border-radius: 4px; font-size: 11px; font-weight: 600; }
    .badge-green { background: #065f4620; color: #34d399; }
    .badge-yellow { background: #854d0e20; color: #fbbf24; }
    .badge-red { background: #7f1d1d20; color: #f87171; }
    .btn { display: inline-block; padding: 8px 16px; border-radius: 6px; font-size: 13px; font-weight: 600; cursor: pointer; border: none; text-decoration: none; transition: all 0.15s; }
    .btn-primary { background: #818cf8; color: #0f1117; }
    .btn-primary:hover { background: #6366f1; }
    .btn-danger { background: #f8717120; color: #f87171; border: 1px solid #f8717140; }
    .btn-danger:hover { background: #f8717140; }
    input, textarea { background: #1e2030; border: 1px solid #2a2d3a; border-radius: 6px; padding: 10px 14px; color: #e2e8f0; font-size: 14px; width: 100%; font-family: inherit; }
    input:focus, textarea:focus { outline: none; border-color: #818cf8; }
    label { display: block; font-size: 13px; color: #94a3b8; margin-bottom: 6px; }
    .form-group { margin-bottom: 16px; }
    .footer { padding: 16px 20px; color: #475569; font-size: 11px; border-top: 1px solid #2a2d3a; margin-top: auto; }
    .mono { font-family: 'SF Mono', 'Fira Code', monospace; font-size: 13px; }
    .text-muted { color: #64748b; }
    .text-green { color: #34d399; }
    .text-red { color: #f87171; }
    .mb-4 { margin-bottom: 16px; }
    .mt-2 { margin-top: 8px; }
    .flex { display: flex; }
    .items-center { align-items: center; }
    .justify-between { justify-content: space-between; }
    .gap-2 { gap: 8px; }
    .gap-4 { gap: 16px; }
"#;

fn nav_link(href: &str, label: &str, active_page: &str, page_id: &str) -> String {
    let class = if active_page == page_id {
        "active"
    } else {
        ""
    };
    format!(r#"<a href="{href}" class="{class}">{label}</a>"#)
}

fn page_shell(title: &str, active_page: &str, user_email: &str, content: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} — WarpGrid Console</title>
  <style>{CSS}</style>
</head>
<body>
  <div class="layout">
    <aside class="sidebar">
      <div class="brand">
        <h1>WarpGrid</h1>
        <span>Cloud Console</span>
      </div>
      <nav>
        {nav_overview}
        {nav_deployments}
        {nav_deploy}
        {nav_teams}
        {nav_settings}
      </nav>
      <div class="footer">
        <div style="margin-bottom:8px;font-size:12px;color:#94a3b8">{user_email}</div>
        <form method="POST" action="/console/logout" style="display:inline">
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
        nav_overview = nav_link("/console/", "Overview", active_page, "overview"),
        nav_deployments = nav_link("/console/deployments", "Deployments", active_page, "deployments"),
        nav_deploy = nav_link("/console/deploy", "Deploy", active_page, "deploy"),
        nav_teams = nav_link("/console/teams", "Teams", active_page, "teams"),
        nav_settings = nav_link("/console/settings", "Settings", active_page, "settings"),
        user_email = user_email,
        content = content,
    )
}

// ── Page handlers ──────────────────────────────────────────────

async fn console_overview(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
) -> Result<Html<String>, Redirect> {
    let user = require_user(&headers, &state)?;

    let all_deployments = state
        .state_store
        .list_deployments()
        .unwrap_or_default();
    let user_deployments: Vec<_> = all_deployments
        .iter()
        .filter(|d| d.namespace == user.namespace)
        .collect();

    let deployment_count = user_deployments.len();

    let mut total_instances = 0usize;
    let mut running_instances = 0usize;
    for d in &user_deployments {
        let instances = state
            .state_store
            .list_instances_for_deployment(&d.id)
            .unwrap_or_default();
        total_instances += instances.len();
        running_instances += instances
            .iter()
            .filter(|i| i.status == warpgrid_state::InstanceStatus::Running)
            .count();
    }

    let content = format!(
        r#"<h2>Overview</h2>
<div class="grid-3">
  <div class="card">
    <h3>Deployments</h3>
    <div class="stat">{deployment_count}</div>
    <div class="stat-label">in namespace <span class="mono">{namespace}</span></div>
  </div>
  <div class="card">
    <h3>Instances</h3>
    <div class="stat">{running_instances}<span style="font-size:16px;color:#64748b"> / {total_instances}</span></div>
    <div class="stat-label">running / total</div>
  </div>
  <div class="card">
    <h3>Region Status</h3>
    <div class="stat text-green" style="font-size:18px">Operational</div>
    <div class="stat-label">iad (primary)</div>
  </div>
</div>

<div class="card" style="margin-top:8px">
  <h3>Recent Deployments</h3>
  {deployments_table}
</div>"#,
        namespace = user.namespace,
        deployments_table = build_mini_deployments_table(&user_deployments),
    );

    Ok(Html(page_shell("Overview", "overview", &user.email, &content)))
}

async fn console_deployments(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
) -> Result<Html<String>, Redirect> {
    let user = require_user(&headers, &state)?;

    let all_deployments = state
        .state_store
        .list_deployments()
        .unwrap_or_default();
    let user_deployments: Vec<_> = all_deployments
        .iter()
        .filter(|d| d.namespace == user.namespace)
        .collect();

    let rows: String = user_deployments
        .iter()
        .map(|d| {
            let instances = state
                .state_store
                .list_instances_for_deployment(&d.id)
                .unwrap_or_default();
            let running = instances
                .iter()
                .filter(|i| i.status == warpgrid_state::InstanceStatus::Running)
                .count();
            format!(
                r#"<tr>
  <td class="mono">{name}</td>
  <td><span class="badge badge-green">running</span></td>
  <td>{running} / {max}</td>
  <td class="mono text-muted">{source}</td>
  <td>
    <a href="/console/deploy" class="btn btn-primary" style="padding:4px 10px;font-size:11px;margin-right:4px">Redeploy</a>
    <form method="POST" action="/api/v1/cloud/deploy/{id}" style="display:inline">
      <button type="submit" class="btn btn-danger" style="padding:4px 10px;font-size:11px">Delete</button>
    </form>
  </td>
</tr>"#,
                name = d.name,
                running = running,
                max = d.instances.max,
                source = truncate_source(&d.source),
                id = d.id,
            )
        })
        .collect();

    let empty_state = if user_deployments.is_empty() {
        r#"<div style="text-align:center;padding:40px;color:#64748b">
  <div style="font-size:24px;margin-bottom:8px">No deployments yet</div>
  <a href="/console/deploy" class="btn btn-primary">Deploy your first app</a>
</div>"#
    } else {
        ""
    };

    let content = format!(
        r#"<div class="flex justify-between items-center mb-4">
  <h2>Deployments</h2>
  <a href="/console/deploy" class="btn btn-primary">New Deployment</a>
</div>
<div class="card">
  <table>
    <thead>
      <tr>
        <th>Name</th>
        <th>Status</th>
        <th>Instances</th>
        <th>Source</th>
        <th>Actions</th>
      </tr>
    </thead>
    <tbody>{rows}</tbody>
  </table>
  {empty_state}
</div>"#,
    );

    Ok(Html(page_shell("Deployments", "deployments", &user.email, &content)))
}

async fn console_deploy(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
) -> Result<Html<String>, Redirect> {
    let user = require_user(&headers, &state)?;

    let content = format!(
        r#"<h2>Deploy</h2>
<div class="card">
  <h3>Deploy a Wasm Component</h3>
  <p class="text-muted" style="margin-bottom:20px;font-size:14px">
    Upload a compiled <code>.wasm</code> component or provide a URL to deploy.
    Deployments run in namespace <span class="mono" style="color:#818cf8">{namespace}</span>.
  </p>
  <form method="POST" action="/api/v1/cloud/deploy" enctype="multipart/form-data">
    <div class="grid-2">
      <div>
        <div class="form-group">
          <label>Deployment Name</label>
          <input type="text" name="name" placeholder="my-app" required>
        </div>
        <div class="form-group">
          <label>Wasm Source URL</label>
          <input type="url" name="source_url" placeholder="https://example.com/app.wasm">
        </div>
        <div class="form-group">
          <label>Or Upload .wasm File</label>
          <input type="file" name="wasm_file" accept=".wasm" style="padding:8px">
        </div>
      </div>
      <div>
        <div class="form-group">
          <label>Min Instances</label>
          <input type="number" name="min_instances" value="1" min="0" max="10">
        </div>
        <div class="form-group">
          <label>Max Instances</label>
          <input type="number" name="max_instances" value="3" min="1" max="{max_per_deployment}">
        </div>
        <div class="form-group">
          <label>Memory Limit</label>
          <input type="text" name="memory" value="64MB" placeholder="64MB">
        </div>
      </div>
    </div>
    <div class="form-group">
      <label>Environment Variables (KEY=VALUE, one per line)</label>
      <textarea name="env_vars" rows="3" placeholder="DATABASE_URL=postgres://..."></textarea>
    </div>
    <button type="submit" class="btn btn-primary">Deploy</button>
  </form>
</div>"#,
        namespace = user.namespace,
        max_per_deployment = user.quota.max_instances_per_deployment,
    );

    Ok(Html(page_shell("Deploy", "deploy", &user.email, &content)))
}

async fn console_teams(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
) -> Result<Html<String>, Redirect> {
    let user = require_user(&headers, &state)?;

    let content = format!(
        r#"<h2>Teams</h2>
<div class="card">
  <h3>Team Members</h3>
  <table>
    <thead>
      <tr>
        <th>Email</th>
        <th>Role</th>
        <th>Joined</th>
      </tr>
    </thead>
    <tbody>
      <tr>
        <td>{email}</td>
        <td><span class="badge badge-green">Owner</span></td>
        <td class="text-muted">—</td>
      </tr>
    </tbody>
  </table>
</div>
<div class="card">
  <h3>Invite Member</h3>
  <p class="text-muted" style="margin-bottom:16px;font-size:14px">
    Team management is coming soon. Members will share the
    <span class="mono" style="color:#818cf8">{namespace}</span> namespace.
  </p>
  <div class="flex gap-2">
    <input type="email" placeholder="teammate@company.com" disabled style="opacity:0.5">
    <button class="btn btn-primary" disabled style="opacity:0.5;white-space:nowrap">Invite</button>
  </div>
</div>"#,
        email = user.email,
        namespace = user.namespace,
    );

    Ok(Html(page_shell("Teams", "teams", &user.email, &content)))
}

async fn console_settings(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
) -> Result<Html<String>, Redirect> {
    let user = require_user(&headers, &state)?;

    let api_key_display = extract_session_cookie(&headers)
        .map(|k| {
            if k.len() > 16 {
                format!("{}...{}", &k[..12], &k[k.len() - 4..])
            } else {
                k
            }
        })
        .unwrap_or_else(|| "—".to_string());

    let content = format!(
        r#"<h2>Settings</h2>
<div class="grid-2">
  <div class="card">
    <h3>API Key</h3>
    <div class="mono" style="background:#1e2030;padding:12px;border-radius:6px;margin-bottom:8px;word-break:break-all">{api_key_display}</div>
    <p class="text-muted" style="font-size:12px">Use this key in the <code>Authorization: Bearer</code> header for API calls.</p>
  </div>
  <div class="card">
    <h3>Namespace</h3>
    <div class="stat" style="font-size:24px">{namespace}</div>
    <div class="stat-label">All deployments are scoped to this namespace</div>
  </div>
</div>
<div class="card">
  <h3>Quotas</h3>
  <table>
    <thead>
      <tr>
        <th>Resource</th>
        <th>Limit</th>
      </tr>
    </thead>
    <tbody>
      <tr>
        <td>Max deployments</td>
        <td class="mono">{max_deployments}</td>
      </tr>
      <tr>
        <td>Max instances per deployment</td>
        <td class="mono">{max_instances}</td>
      </tr>
      <tr>
        <td>Max Wasm binary size</td>
        <td class="mono">{max_wasm_size}</td>
      </tr>
      <tr>
        <td>Max memory per instance</td>
        <td class="mono">{max_memory}</td>
      </tr>
      <tr>
        <td>Max request rate</td>
        <td class="mono">{max_rps} req/s</td>
      </tr>
    </tbody>
  </table>
</div>"#,
        namespace = user.namespace,
        max_deployments = user.quota.max_deployments,
        max_instances = user.quota.max_instances_per_deployment,
        max_wasm_size = format_bytes(user.quota.max_wasm_size_bytes),
        max_memory = format_bytes(user.quota.max_memory_per_instance_bytes),
        max_rps = user.quota.max_request_rate,
    );

    Ok(Html(page_shell("Settings", "settings", &user.email, &content)))
}

async fn console_login_page() -> Html<String> {
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Login — WarpGrid Console</title>
  <style>{CSS}
    .login-container {{ display: flex; align-items: center; justify-content: center; min-height: 100vh; }}
    .login-box {{ width: 400px; }}
  </style>
</head>
<body>
  <div class="login-container">
    <div class="login-box">
      <div style="text-align:center;margin-bottom:32px">
        <h1 style="color:#818cf8;font-size:28px;margin-bottom:4px">WarpGrid</h1>
        <span style="color:#64748b;font-size:14px">Cloud Console</span>
      </div>
      <div class="card">
        <h3>Sign In</h3>
        <form method="POST" action="/console/login">
          <div class="form-group">
            <label>API Key</label>
            <input type="password" name="api_key" placeholder="wg_live_..." required autofocus>
          </div>
          <button type="submit" class="btn btn-primary" style="width:100%">Sign In</button>
        </form>
        <p class="text-muted" style="margin-top:16px;font-size:12px;text-align:center">
          Use the API key from <code>warp register</code>
        </p>
      </div>
      <div style="text-align:center;margin-top:16px;color:#475569;font-size:11px">
        Served by WarpGrid
      </div>
    </div>
  </div>
</body>
</html>"#,
    );
    Html(html)
}

async fn console_login_submit(
    State(state): State<ConsoleState>,
    axum::Form(form): axum::Form<LoginForm>,
) -> impl IntoResponse {
    let api_key = form.api_key.trim().to_string();

    if state.auth.validate(&api_key).is_none() {
        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Login — WarpGrid Console</title>
  <style>{CSS}
    .login-container {{ display: flex; align-items: center; justify-content: center; min-height: 100vh; }}
    .login-box {{ width: 400px; }}
  </style>
</head>
<body>
  <div class="login-container">
    <div class="login-box">
      <div style="text-align:center;margin-bottom:32px">
        <h1 style="color:#818cf8;font-size:28px;margin-bottom:4px">WarpGrid</h1>
        <span style="color:#64748b;font-size:14px">Cloud Console</span>
      </div>
      <div class="card">
        <div style="background:#7f1d1d20;border:1px solid #f8717140;border-radius:6px;padding:10px 14px;margin-bottom:16px;color:#f87171;font-size:13px">
          Invalid API key. Please try again.
        </div>
        <h3>Sign In</h3>
        <form method="POST" action="/console/login">
          <div class="form-group">
            <label>API Key</label>
            <input type="password" name="api_key" placeholder="wg_live_..." required autofocus>
          </div>
          <button type="submit" class="btn btn-primary" style="width:100%">Sign In</button>
        </form>
        <p class="text-muted" style="margin-top:16px;font-size:12px;text-align:center">
          Use the API key from <code>warp register</code>
        </p>
      </div>
      <div style="text-align:center;margin-top:16px;color:#475569;font-size:11px">
        Served by WarpGrid
      </div>
    </div>
  </div>
</body>
</html>"#,
        );
        return (
            StatusCode::UNAUTHORIZED,
            HeaderMap::new(),
            Html(html),
        );
    }

    let mut response_headers = HeaderMap::new();
    let cookie_value = format!(
        "wg_session={api_key}; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800"
    );
    response_headers.insert(
        "set-cookie",
        cookie_value.parse().expect("valid cookie header"),
    );
    response_headers.insert(
        "location",
        "/console/".parse().expect("valid location header"),
    );

    (StatusCode::SEE_OTHER, response_headers, Html(String::new()))
}

async fn console_logout() -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    let cookie_value =
        "wg_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0";
    headers.insert(
        "set-cookie",
        cookie_value.parse().expect("valid cookie header"),
    );
    headers.insert(
        "location",
        "/console/login".parse().expect("valid location header"),
    );

    (StatusCode::SEE_OTHER, headers, Html(String::new()))
}

// ── Helper functions ───────────────────────────────────────────

fn build_mini_deployments_table(deployments: &[&warpgrid_state::DeploymentSpec]) -> String {
    if deployments.is_empty() {
        return r#"<div style="text-align:center;padding:24px;color:#64748b">
  No deployments yet. <a href="/console/deploy" style="color:#818cf8">Deploy your first app</a>
</div>"#
            .to_string();
    }

    let rows: String = deployments
        .iter()
        .take(5)
        .map(|d| {
            format!(
                r#"<tr>
  <td class="mono">{name}</td>
  <td><span class="badge badge-green">running</span></td>
  <td class="mono text-muted">{source}</td>
</tr>"#,
                name = d.name,
                source = truncate_source(&d.source),
            )
        })
        .collect();

    format!(
        r#"<table>
  <thead><tr><th>Name</th><th>Status</th><th>Source</th></tr></thead>
  <tbody>{rows}</tbody>
</table>"#,
    )
}

fn truncate_source(source: &str) -> String {
    if source.len() > 40 {
        format!("{}...", &source[..37])
    } else {
        source.to_string()
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::auth::AuthStore;
    use crate::cloud::registry::WasmRegistry;

    fn test_cloud_state() -> CloudState {
        let state_store = warpgrid_state::StateStore::open_in_memory().unwrap();
        let auth = AuthStore::new();
        let registry = WasmRegistry::local(std::path::Path::new("/tmp/wg-test-registry"));
        CloudState {
            auth,
            registry,
            state_store,
        }
    }

    #[test]
    fn extract_session_cookie_from_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            "wg_session=wg_live_abc123; other=value"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            extract_session_cookie(&headers),
            Some("wg_live_abc123".to_string())
        );
    }

    #[test]
    fn extract_session_cookie_missing() {
        let headers = HeaderMap::new();
        assert_eq!(extract_session_cookie(&headers), None);
    }

    #[test]
    fn extract_session_cookie_no_wg_session() {
        let mut headers = HeaderMap::new();
        headers.insert("cookie", "other=value".parse().unwrap());
        assert_eq!(extract_session_cookie(&headers), None);
    }

    #[test]
    fn truncate_source_short() {
        assert_eq!(truncate_source("file://test.wasm"), "file://test.wasm");
    }

    #[test]
    fn truncate_source_long() {
        let long = "https://registry.example.com/very/long/path/to/component.wasm";
        let result = truncate_source(long);
        assert!(result.len() <= 43);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn format_bytes_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(10 * 1024 * 1024), "10 MB");
        assert_eq!(format_bytes(256 * 1024 * 1024), "256 MB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    #[tokio::test]
    async fn login_page_renders() {
        let html = console_login_page().await;
        let body = html.0;
        assert!(body.contains("WarpGrid"));
        assert!(body.contains("Sign In"));
        assert!(body.contains("wg_live_"));
        assert!(body.contains("Served by WarpGrid"));
    }

    #[tokio::test]
    async fn require_user_redirects_without_cookie() {
        let state = Arc::new(test_cloud_state());
        let headers = HeaderMap::new();
        let result = require_user(&headers, &state);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn require_user_redirects_with_invalid_key() {
        let state = Arc::new(test_cloud_state());
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            "wg_session=wg_live_invalid_key_0000000000"
                .parse()
                .unwrap(),
        );
        let result = require_user(&headers, &state);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn require_user_succeeds_with_valid_key() {
        let cloud = test_cloud_state();
        let (api_key, _user) = cloud.auth.register("test@example.com");
        let state = Arc::new(cloud);
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            format!("wg_session={api_key}").parse().unwrap(),
        );
        let result = require_user(&headers, &state);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().email, "test@example.com");
    }
}
