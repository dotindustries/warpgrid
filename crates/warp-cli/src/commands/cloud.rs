//! Cloud CLI commands — login, deploy, status, logs, destroy.
//!
//! All commands communicate with the WarpGrid cloud control plane
//! via its REST API. Credentials are stored in ~/.warpgrid/config.toml.

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ── Config ──────────────────────────────────────────────────────

const CONFIG_DIR: &str = ".warpgrid";
const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct CloudConfig {
    pub api_key: Option<String>,
    pub api_url: Option<String>,
    pub namespace: Option<String>,
}

impl CloudConfig {
    /// Load config from ~/.warpgrid/config.toml.
    pub fn load() -> anyhow::Result<Self> {
        let path = config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let config: Self = toml_edit::de::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        Ok(config)
    }

    /// Save config to ~/.warpgrid/config.toml.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml_edit::ser::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Get the API URL, defaulting to localhost for development.
    pub fn api_url(&self) -> &str {
        self.api_url
            .as_deref()
            .unwrap_or("http://localhost:8443")
    }

    /// Get the API key or bail if not logged in.
    pub fn require_api_key(&self) -> anyhow::Result<&str> {
        self.api_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Not logged in. Run `warp login` first."))
    }
}

fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(CONFIG_DIR)
        .join(CONFIG_FILE)
}

// ── API Response Types ──────────────────────────────────────────

#[derive(Deserialize)]
struct ApiResponse<T> {
    success: bool,
    data: Option<T>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct RegisterData {
    api_key: String,
    user_id: String,
    namespace: String,
}

#[derive(Deserialize)]
struct DeploymentInfo {
    id: String,
    namespace: String,
    name: String,
    status: String,
    instances: u32,
    region: String,
}

#[derive(Deserialize)]
struct StatusData {
    status: String,
    version: String,
    mode: String,
}

// ── Commands ────────────────────────────────────────────────────

/// Login with an API key or register a new account.
pub fn login(api_key: Option<&str>, api_url: Option<&str>, email: Option<&str>) -> anyhow::Result<()> {
    let mut config = CloudConfig::load()?;

    if let Some(url) = api_url {
        config.api_url = Some(url.to_string());
    }

    if let Some(key) = api_key {
        // Login with existing key.
        config.api_key = Some(key.to_string());
        config.save()?;
        println!("Logged in with API key.");

        // Verify the key works.
        let client = reqwest::blocking::Client::new();
        let resp = client
            .get(format!("{}/api/v1/cloud/deployments", config.api_url()))
            .header("Authorization", format!("Bearer {key}"))
            .send()?;

        if resp.status().is_success() {
            println!("API key verified.");
        } else {
            println!("Warning: API key could not be verified (server returned {}).", resp.status());
        }
    } else if let Some(email) = email {
        // Register new account.
        let client = reqwest::blocking::Client::new();
        let resp = client
            .post(format!("{}/api/v1/auth/register", config.api_url()))
            .json(&serde_json::json!({ "email": email }))
            .send()
            .context("Failed to connect to WarpGrid cloud")?;

        let body: ApiResponse<RegisterData> = resp.json()?;
        if let Some(data) = body.data {
            config.api_key = Some(data.api_key.clone());
            config.namespace = Some(data.namespace.clone());
            config.save()?;

            println!("Account created!");
            println!("  User ID:   {}", data.user_id);
            println!("  Namespace: {}", data.namespace);
            println!("  API Key:   {}", data.api_key);
            println!();
            println!("Save your API key — it won't be shown again.");
            println!("Config saved to {}", config_path().display());
        } else {
            bail!("Registration failed: {}", body.error.unwrap_or_default());
        }
    } else {
        bail!("Provide --api-key to login or --email to register a new account.");
    }

    Ok(())
}

/// Deploy a Wasm component to the cloud platform.
pub fn deploy(path: &str, region: Option<&str>, lang: Option<&str>) -> anyhow::Result<()> {
    let config = CloudConfig::load()?;
    let api_key = config.require_api_key()?;
    let api_url = config.api_url();

    // Step 1: Pack the project.
    println!("Compiling project...");
    let pack_result = warp_pack::pack_with_lang(Path::new(path), lang)?;
    println!(
        "  Compiled: {} ({} bytes, sha256: {})",
        pack_result.output_path,
        pack_result.size_bytes,
        &pack_result.sha256[..12]
    );

    // Step 2: Read deployment name from warp.toml.
    let warp_toml_path = Path::new(path).join("warp.toml");
    let deploy_name = if warp_toml_path.exists() {
        let content = std::fs::read_to_string(&warp_toml_path)?;
        let doc: toml_edit::DocumentMut = content.parse()?;
        doc.get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("unnamed")
            .to_string()
    } else {
        // Fall back to directory name.
        Path::new(path)
            .canonicalize()?
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed")
            .to_string()
    };

    // Step 3: Read the compiled Wasm.
    let wasm_bytes = std::fs::read(&pack_result.output_path)
        .context("Failed to read compiled Wasm component")?;

    let region = region.unwrap_or("iad");
    println!("Deploying '{}' to {} ({} bytes)...", deploy_name, region, wasm_bytes.len());

    // Step 4: Upload to cloud.
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(format!("{api_url}/api/v1/cloud/deploy/upload"))
        .header("Authorization", format!("Bearer {api_key}"))
        .header("X-WarpGrid-Name", &deploy_name)
        .header("X-WarpGrid-Region", region)
        .body(wasm_bytes)
        .send()
        .context("Failed to connect to WarpGrid cloud")?;

    if resp.status().is_success() {
        let body: ApiResponse<serde_json::Value> = resp.json()?;
        if let Some(data) = body.data {
            let url = data.get("url").and_then(|u| u.as_str()).unwrap_or("(unknown)");
            let hash = data.get("wasm_hash").and_then(|h| h.as_str()).unwrap_or("");
            println!("Deployed successfully!");
            println!("  Name:      {}", deploy_name);
            println!("  URL:       {}", url);
            println!("  Wasm hash: {}", &hash[..12.min(hash.len())]);
        } else {
            println!("Deployed successfully!");
        }
    } else {
        let body: ApiResponse<()> = resp.json()?;
        bail!(
            "Deploy failed: {}",
            body.error.unwrap_or_else(|| "Unknown error".to_string())
        );
    }

    Ok(())
}

/// Show deployment status.
pub fn status() -> anyhow::Result<()> {
    let config = CloudConfig::load()?;
    let api_key = config.require_api_key()?;

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(format!(
            "{}/api/v1/cloud/deployments",
            config.api_url()
        ))
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .context("Failed to connect to WarpGrid cloud")?;

    let body: ApiResponse<Vec<DeploymentInfo>> = resp.json()?;

    if let Some(deployments) = body.data {
        if deployments.is_empty() {
            println!("No deployments. Run `warp deploy` to create one.");
            return Ok(());
        }
        println!(
            "{:<30} {:<10} {:<10} {:<8} {:<6}",
            "DEPLOYMENT", "STATUS", "REGION", "INST", "NS"
        );
        println!("{}", "-".repeat(70));
        for d in &deployments {
            println!(
                "{:<30} {:<10} {:<10} {:<8} {:<6}",
                d.id, d.status, d.region, d.instances, d.namespace
            );
        }
        println!("\n{} deployment(s)", deployments.len());
    } else {
        bail!(
            "Failed to list deployments: {}",
            body.error.unwrap_or_default()
        );
    }

    Ok(())
}

/// Destroy a deployment.
pub fn destroy(deployment_id: &str) -> anyhow::Result<()> {
    let config = CloudConfig::load()?;
    let api_key = config.require_api_key()?;

    let client = reqwest::blocking::Client::new();
    let resp = client
        .delete(format!(
            "{}/api/v1/cloud/deploy/{}",
            config.api_url(),
            deployment_id
        ))
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .context("Failed to connect to WarpGrid cloud")?;

    if resp.status().is_success() {
        println!("Deployment '{}' destroyed.", deployment_id);
    } else {
        let body: ApiResponse<()> = resp.json()?;
        bail!(
            "Destroy failed: {}",
            body.error.unwrap_or_else(|| "Unknown error".to_string())
        );
    }

    Ok(())
}

/// Show platform status.
pub fn platform_status(api_url_override: Option<&str>) -> anyhow::Result<()> {
    let config = CloudConfig::load()?;
    let api_url = api_url_override.unwrap_or_else(|| config.api_url());

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(format!("{api_url}/api/v1/cloud/status"))
        .send()
        .context("Failed to connect to WarpGrid cloud")?;

    let body: ApiResponse<StatusData> = resp.json()?;

    if let Some(data) = body.data {
        println!("WarpGrid Cloud");
        println!("  Status:  {}", data.status);
        println!("  Version: {}", data.version);
        println!("  Mode:    {}", data.mode);
        println!("  API:     {}", api_url);
        if let Some(ns) = &config.namespace {
            println!("  Namespace: {}", ns);
        }
    } else {
        bail!("Failed: {}", body.error.unwrap_or_default());
    }

    Ok(())
}
