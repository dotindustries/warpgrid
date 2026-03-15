//! Multi-tenant namespace isolation.
//!
//! Each user gets a namespace that scopes their deployments, instances,
//! and resource quotas. Deployment IDs are prefixed with the namespace:
//! `{namespace}/{deployment_name}`.
//!
//! The WASI sandbox provides memory/CPU isolation per component.
//! The scheduler places workloads using namespace-aware placement logic.

use serde::{Deserialize, Serialize};

/// A tenant namespace in the cloud platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub namespace: String,
    pub owner_user_id: String,
    pub created_at: u64,
    pub resource_limits: TenantLimits,
    pub usage: TenantUsage,
}

/// Resource limits for a tenant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantLimits {
    pub max_deployments: u32,
    pub max_total_instances: u32,
    pub max_total_memory_bytes: u64,
    pub max_wasm_size_bytes: u64,
    pub regions: Vec<String>,
}

impl Default for TenantLimits {
    fn default() -> Self {
        Self {
            max_deployments: 3,
            max_total_instances: 15,
            max_total_memory_bytes: 1024 * 1024 * 1024, // 1 GB total
            max_wasm_size_bytes: 10 * 1024 * 1024,      // 10 MB per component
            regions: vec!["iad".to_string()],
        }
    }
}

/// Current resource usage for a tenant.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TenantUsage {
    pub deployment_count: u32,
    pub instance_count: u32,
    pub total_memory_bytes: u64,
    pub total_wasm_bytes: u64,
}

/// Scoped deployment ID: `{namespace}/{name}`.
pub fn scoped_deployment_id(namespace: &str, name: &str) -> String {
    format!("{namespace}/{name}")
}

/// Extract namespace from a scoped deployment ID.
pub fn extract_namespace(scoped_id: &str) -> Option<(&str, &str)> {
    scoped_id.split_once('/')
}

/// Check if a tenant can create a new deployment given current usage.
pub fn check_deployment_quota(
    limits: &TenantLimits,
    usage: &TenantUsage,
    wasm_size_bytes: u64,
) -> Result<(), QuotaError> {
    if usage.deployment_count >= limits.max_deployments {
        return Err(QuotaError::MaxDeployments {
            current: usage.deployment_count,
            max: limits.max_deployments,
        });
    }
    if wasm_size_bytes > limits.max_wasm_size_bytes {
        return Err(QuotaError::WasmTooLarge {
            size: wasm_size_bytes,
            max: limits.max_wasm_size_bytes,
        });
    }
    Ok(())
}

/// Quota violation error.
#[derive(Debug, Clone, Serialize)]
pub enum QuotaError {
    MaxDeployments { current: u32, max: u32 },
    MaxInstances { current: u32, max: u32 },
    WasmTooLarge { size: u64, max: u64 },
    MemoryExceeded { used: u64, max: u64 },
    RegionNotAllowed { region: String },
}

impl std::fmt::Display for QuotaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MaxDeployments { current, max } => {
                write!(f, "deployment limit reached ({current}/{max})")
            }
            Self::MaxInstances { current, max } => {
                write!(f, "instance limit reached ({current}/{max})")
            }
            Self::WasmTooLarge { size, max } => {
                write!(
                    f,
                    "Wasm component too large ({} KB, max {} KB)",
                    size / 1024,
                    max / 1024
                )
            }
            Self::MemoryExceeded { used, max } => {
                write!(
                    f,
                    "memory limit exceeded ({} MB / {} MB)",
                    used / (1024 * 1024),
                    max / (1024 * 1024)
                )
            }
            Self::RegionNotAllowed { region } => {
                write!(f, "region '{region}' not allowed for this plan")
            }
        }
    }
}

impl std::error::Error for QuotaError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_id_roundtrip() {
        let id = scoped_deployment_id("alice", "my-api");
        assert_eq!(id, "alice/my-api");

        let (ns, name) = extract_namespace(&id).unwrap();
        assert_eq!(ns, "alice");
        assert_eq!(name, "my-api");
    }

    #[test]
    fn quota_check_allows_within_limits() {
        let limits = TenantLimits::default();
        let usage = TenantUsage {
            deployment_count: 1,
            ..Default::default()
        };
        assert!(check_deployment_quota(&limits, &usage, 1024).is_ok());
    }

    #[test]
    fn quota_check_rejects_at_max_deployments() {
        let limits = TenantLimits::default();
        let usage = TenantUsage {
            deployment_count: 3,
            ..Default::default()
        };
        let err = check_deployment_quota(&limits, &usage, 1024).unwrap_err();
        assert!(matches!(err, QuotaError::MaxDeployments { .. }));
    }

    #[test]
    fn quota_check_rejects_oversized_wasm() {
        let limits = TenantLimits::default();
        let usage = TenantUsage::default();
        let err = check_deployment_quota(&limits, &usage, 20 * 1024 * 1024).unwrap_err();
        assert!(matches!(err, QuotaError::WasmTooLarge { .. }));
    }
}
