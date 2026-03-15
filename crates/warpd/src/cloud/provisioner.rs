//! Fly Machines API client for provisioning edge `warpd agent` nodes.
//!
//! Manages the lifecycle of edge machines across regions:
//! - Create machines running `warpd agent` in specified regions
//! - Start/stop/delete machines
//! - List machines and their status
//!
//! API docs: https://fly.io/docs/machines/api/machines-resource/

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};

const FLY_API_BASE: &str = "https://api.machines.dev/v1";

/// Fly Machines API client.
#[derive(Clone)]
pub struct FlyProvisioner {
    api_token: String,
    app_name: String,
    client: reqwest::Client,
    /// Docker image to use for edge nodes (e.g., "registry.fly.io/warpgrid:latest").
    edge_image: String,
}

/// Configuration for a new edge machine.
#[derive(Debug, Clone)]
pub struct EdgeMachineConfig {
    pub region: String,
    pub cpus: u32,
    pub memory_mb: u32,
    pub cpu_kind: String,
    pub control_plane_url: String,
    pub env: HashMap<String, String>,
}

impl Default for EdgeMachineConfig {
    fn default() -> Self {
        Self {
            region: "iad".to_string(),
            cpus: 1,
            memory_mb: 256,
            cpu_kind: "shared".to_string(),
            control_plane_url: String::new(),
            env: HashMap::new(),
        }
    }
}

/// A provisioned Fly Machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisionedMachine {
    pub id: String,
    pub name: String,
    pub region: String,
    pub state: String,
    pub instance_id: String,
    pub private_ip: String,
}

// ── Fly API request/response types ──────────────────────────────

#[derive(Serialize)]
struct CreateMachineRequest {
    name: String,
    region: String,
    config: MachineConfig,
}

#[derive(Serialize)]
struct MachineConfig {
    image: String,
    guest: GuestConfig,
    env: HashMap<String, String>,
    services: Vec<ServiceConfig>,
}

#[derive(Serialize)]
struct GuestConfig {
    cpus: u32,
    memory_mb: u32,
    cpu_kind: String,
}

#[derive(Serialize)]
struct ServiceConfig {
    protocol: String,
    internal_port: u16,
    ports: Vec<PortConfig>,
}

#[derive(Serialize)]
struct PortConfig {
    port: u16,
    handlers: Vec<String>,
}

#[derive(Deserialize)]
struct FlyMachineResponse {
    id: String,
    name: String,
    region: String,
    state: String,
    instance_id: String,
    private_ip: String,
}

#[derive(Deserialize)]
struct FlyErrorResponse {
    error: Option<String>,
    message: Option<String>,
}

impl FlyProvisioner {
    /// Create a new provisioner.
    ///
    /// - `api_token`: Fly.io API token
    /// - `app_name`: Fly app name (e.g., "warpgrid-edge")
    /// - `edge_image`: Docker image for edge nodes
    pub fn new(api_token: &str, app_name: &str, edge_image: &str) -> Self {
        Self {
            api_token: api_token.to_string(),
            app_name: app_name.to_string(),
            client: reqwest::Client::new(),
            edge_image: edge_image.to_string(),
        }
    }

    /// Provision a new edge machine in the specified region.
    pub async fn create_edge_machine(
        &self,
        config: &EdgeMachineConfig,
    ) -> anyhow::Result<ProvisionedMachine> {
        let machine_name = format!("warpgrid-edge-{}", config.region);

        let mut env = config.env.clone();
        env.insert(
            "WARPGRID_CONTROL_PLANE".to_string(),
            config.control_plane_url.clone(),
        );
        env.insert("WARPGRID_REGION".to_string(), config.region.clone());
        env.insert("RUST_LOG".to_string(), "info,warpd=debug".to_string());

        let body = CreateMachineRequest {
            name: machine_name.clone(),
            region: config.region.clone(),
            config: MachineConfig {
                image: self.edge_image.clone(),
                guest: GuestConfig {
                    cpus: config.cpus,
                    memory_mb: config.memory_mb,
                    cpu_kind: config.cpu_kind.clone(),
                },
                env,
                services: vec![ServiceConfig {
                    protocol: "tcp".to_string(),
                    internal_port: 8443,
                    ports: vec![
                        PortConfig {
                            port: 443,
                            handlers: vec!["tls".to_string(), "http".to_string()],
                        },
                        PortConfig {
                            port: 80,
                            handlers: vec!["http".to_string()],
                        },
                    ],
                }],
            },
        };

        info!(region = %config.region, name = %machine_name, "provisioning edge machine");

        let resp = self
            .client
            .post(format!("{}/apps/{}/machines", FLY_API_BASE, self.app_name))
            .header("Authorization", format!("Bearer {}", self.api_token))
            .json(&body)
            .send()
            .await
            .context("Failed to call Fly Machines API")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let error: FlyErrorResponse = resp.json().await.unwrap_or(FlyErrorResponse {
                error: Some("Unknown error".to_string()),
                message: None,
            });
            bail!(
                "Fly API error ({}): {}",
                status,
                error.error.or(error.message).unwrap_or_default()
            );
        }

        let machine: FlyMachineResponse = resp.json().await?;
        info!(
            id = %machine.id,
            region = %machine.region,
            state = %machine.state,
            "edge machine provisioned"
        );

        Ok(ProvisionedMachine {
            id: machine.id,
            name: machine.name,
            region: machine.region,
            state: machine.state,
            instance_id: machine.instance_id,
            private_ip: machine.private_ip,
        })
    }

    /// Stop a machine.
    pub async fn stop_machine(&self, machine_id: &str) -> anyhow::Result<()> {
        debug!(machine_id, "stopping machine");
        let resp = self
            .client
            .post(format!(
                "{}/apps/{}/machines/{}/stop",
                FLY_API_BASE, self.app_name, machine_id
            ))
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await?;

        if !resp.status().is_success() {
            warn!(
                machine_id,
                status = %resp.status(),
                "failed to stop machine"
            );
        }
        Ok(())
    }

    /// Start a machine.
    pub async fn start_machine(&self, machine_id: &str) -> anyhow::Result<()> {
        debug!(machine_id, "starting machine");
        let resp = self
            .client
            .post(format!(
                "{}/apps/{}/machines/{}/start",
                FLY_API_BASE, self.app_name, machine_id
            ))
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await?;

        if !resp.status().is_success() {
            warn!(
                machine_id,
                status = %resp.status(),
                "failed to start machine"
            );
        }
        Ok(())
    }

    /// Delete a machine.
    pub async fn delete_machine(&self, machine_id: &str, force: bool) -> anyhow::Result<()> {
        info!(machine_id, force, "deleting machine");
        let url = if force {
            format!(
                "{}/apps/{}/machines/{}?force=true",
                FLY_API_BASE, self.app_name, machine_id
            )
        } else {
            format!(
                "{}/apps/{}/machines/{}",
                FLY_API_BASE, self.app_name, machine_id
            )
        };

        let resp = self
            .client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let error: FlyErrorResponse = resp.json().await.unwrap_or(FlyErrorResponse {
                error: Some("Unknown error".to_string()),
                message: None,
            });
            bail!(
                "Failed to delete machine {} ({}): {}",
                machine_id,
                status,
                error.error.or(error.message).unwrap_or_default()
            );
        }
        Ok(())
    }

    /// List all machines in the app.
    pub async fn list_machines(&self) -> anyhow::Result<Vec<ProvisionedMachine>> {
        let resp = self
            .client
            .get(format!("{}/apps/{}/machines", FLY_API_BASE, self.app_name))
            .header("Authorization", format!("Bearer {}", self.api_token))
            .send()
            .await
            .context("Failed to list Fly machines")?;

        if !resp.status().is_success() {
            bail!("Fly API error: {}", resp.status());
        }

        let machines: Vec<FlyMachineResponse> = resp.json().await?;
        Ok(machines
            .into_iter()
            .map(|m| ProvisionedMachine {
                id: m.id,
                name: m.name,
                region: m.region,
                state: m.state,
                instance_id: m.instance_id,
                private_ip: m.private_ip,
            })
            .collect())
    }

    /// Provision edge machines across multiple regions.
    /// Skips regions that already have a running machine.
    pub async fn provision_regions(
        &self,
        regions: &[String],
        control_plane_url: &str,
    ) -> anyhow::Result<Vec<ProvisionedMachine>> {
        let existing = self.list_machines().await.unwrap_or_default();
        let existing_regions: Vec<&str> = existing.iter().map(|m| m.region.as_str()).collect();

        let mut results = Vec::new();
        for region in regions {
            if existing_regions.contains(&region.as_str()) {
                info!(region, "edge machine already exists, skipping");
                continue;
            }

            let config = EdgeMachineConfig {
                region: region.clone(),
                control_plane_url: control_plane_url.to_string(),
                ..Default::default()
            };

            match self.create_edge_machine(&config).await {
                Ok(machine) => results.push(machine),
                Err(e) => warn!(region, error = %e, "failed to provision edge machine"),
            }
        }

        Ok(results)
    }
}
