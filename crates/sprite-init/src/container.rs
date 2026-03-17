//! Inner container (user namespace) management.
//!
//! The user's workload (Claude Code) runs in an inner Linux namespace with
//! its own PID, mount, and optionally network namespace.

use std::collections::HashMap;

use tracing::info;

/// Configuration for the inner container.
pub struct ContainerConfig {
    /// Entrypoint command (default: claude code session).
    pub entrypoint: Vec<String>,
    /// Environment variables for the inner namespace.
    pub env: HashMap<String, String>,
    /// Working directory inside the container.
    pub workdir: String,
}

impl ContainerConfig {
    /// Build config from environment variables set by the host.
    pub fn from_env() -> Self {
        let entrypoint = std::env::var("SPRITE_ENTRYPOINT")
            .unwrap_or_else(|_| "claude --dangerously-skip-permissions".to_string());

        let workdir = std::env::var("SPRITE_WORKDIR").unwrap_or_else(|_| "/workspace".to_string());

        let mut env = HashMap::new();
        // Pass through common env vars.
        for key in &[
            "ANTHROPIC_API_KEY",
            "PATH",
            "HOME",
            "USER",
            "TERM",
            "LANG",
        ] {
            if let Ok(val) = std::env::var(key) {
                env.insert(key.to_string(), val);
            }
        }

        // Default PATH if not set.
        env.entry("PATH".to_string()).or_insert_with(|| {
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string()
        });
        env.entry("HOME".to_string())
            .or_insert_with(|| "/root".to_string());

        Self {
            entrypoint: entrypoint.split_whitespace().map(String::from).collect(),
            env,
            workdir,
        }
    }
}

/// Spawn the inner container process with namespace isolation.
pub async fn spawn_inner(
    config: &ContainerConfig,
) -> anyhow::Result<tokio::process::Child> {
    info!(
        entrypoint = ?config.entrypoint,
        workdir = %config.workdir,
        "spawning inner container"
    );

    let program = config
        .entrypoint
        .first()
        .ok_or_else(|| anyhow::anyhow!("empty entrypoint"))?;

    let args = &config.entrypoint[1..];

    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args);
    cmd.current_dir(&config.workdir);
    cmd.envs(&config.env);

    // In a real implementation, we'd use clone(2) with CLONE_NEWPID | CLONE_NEWNS
    // to create a new PID and mount namespace. For now, spawn as a regular child.
    let child = cmd.spawn()?;

    info!(pid = ?child.id(), "inner container spawned");
    Ok(child)
}

/// Execute a one-off command inside the inner namespace.
pub async fn exec_command(
    command: &str,
    env: &[(String, String)],
) -> anyhow::Result<(i32, String, String)> {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd.current_dir("/workspace");

    for (key, value) in env {
        cmd.env(key, value);
    }

    let output = cmd.output().await?;

    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok((exit_code, stdout, stderr))
}
