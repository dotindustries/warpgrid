//! Cluster gRPC server — control plane side.
//!
//! Implements the `ClusterService` gRPC interface. Runs on the
//! control plane node and handles join, heartbeat, and leave RPCs
//! from agent nodes.

use std::collections::HashMap;
use std::sync::Arc;

use tonic::{Request, Response, Status};
use tracing::{info, warn};

use crate::membership::MembershipManager;
use crate::proto;
use crate::proto::cluster_service_server::ClusterService;

/// Trait for validating agent auth tokens on Join.
///
/// Cloud mode plugs in `AgentTokenStore`; self-hosted mode uses `NoopTokenValidator`.
#[tonic::async_trait]
pub trait TokenValidator: Send + Sync + 'static {
    /// Validate a raw agent token. Returns `(token_id, namespace)` if valid.
    async fn validate_agent_token(&self, raw_token: &str) -> Option<(String, String)>;
}

/// No-op validator — accepts all agents (for self-hosted / standalone mode).
pub struct NoopTokenValidator;

#[tonic::async_trait]
impl TokenValidator for NoopTokenValidator {
    async fn validate_agent_token(&self, _raw_token: &str) -> Option<(String, String)> {
        Some(("unmanaged".to_string(), String::new()))
    }
}

/// gRPC implementation of the cluster service.
pub struct ClusterServer {
    membership: Arc<MembershipManager>,
    token_validator: Arc<dyn TokenValidator>,
    /// When true, agents MUST present a valid auth_token to join.
    require_auth: bool,
}

impl ClusterServer {
    /// Create a new cluster server (no auth required — for self-hosted mode).
    pub fn new(membership: Arc<MembershipManager>) -> Self {
        Self {
            membership,
            token_validator: Arc::new(NoopTokenValidator),
            require_auth: false,
        }
    }

    /// Create a cluster server that requires agent auth tokens (for cloud mode).
    pub fn with_auth(
        membership: Arc<MembershipManager>,
        validator: Arc<dyn TokenValidator>,
    ) -> Self {
        Self {
            membership,
            token_validator: validator,
            require_auth: true,
        }
    }

    /// Get the tonic service for mounting on a gRPC server.
    pub fn into_service(self) -> proto::cluster_service_server::ClusterServiceServer<Self> {
        proto::cluster_service_server::ClusterServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl ClusterService for ClusterServer {
    async fn join(
        &self,
        request: Request<proto::JoinRequest>,
    ) -> Result<Response<proto::JoinResponse>, Status> {
        let req = request.into_inner();

        // ── Auth token validation ───────────────────────────────────
        let mut namespace = String::new();
        if !req.auth_token.is_empty() {
            match self.token_validator.validate_agent_token(&req.auth_token).await {
                Some((_token_id, ns)) => {
                    info!(namespace = %ns, "agent authenticated via token");
                    namespace = ns;
                }
                None => {
                    warn!(address = %req.address, "agent join rejected: invalid or revoked token");
                    return Err(Status::unauthenticated("invalid or revoked agent token"));
                }
            }
        } else if self.require_auth {
            warn!(address = %req.address, "agent join rejected: auth token required");
            return Err(Status::unauthenticated("auth_token is required to join this cluster"));
        }

        // Inject namespace into labels for tenant-scoped placement.
        let mut labels: HashMap<String, String> = req.labels.into_iter().collect();
        if !namespace.is_empty() {
            labels.insert("namespace".to_string(), namespace.clone());
        }

        let node_id = self
            .membership
            .join(
                &req.address,
                req.port as u16,
                labels,
                req.capacity_memory_bytes,
                req.capacity_cpu_weight,
            )
            .map_err(|e| Status::internal(e.to_string()))?;

        let members = self
            .membership
            .list_members()
            .map_err(|e| Status::internal(e.to_string()))?;

        let proto_members: Vec<proto::NodeMember> = members
            .iter()
            .map(|m| proto::NodeMember {
                node_id: m.node_id.clone(),
                address: m.address.clone(),
                port: m.port as u32,
                status: match m.status {
                    crate::membership::MemberStatus::Joining => proto::NodeStatus::Joining.into(),
                    crate::membership::MemberStatus::Ready => proto::NodeStatus::Ready.into(),
                    crate::membership::MemberStatus::Draining => proto::NodeStatus::Draining.into(),
                    crate::membership::MemberStatus::Left => proto::NodeStatus::Left.into(),
                    crate::membership::MemberStatus::Dead => proto::NodeStatus::Unknown.into(),
                },
                labels: m.labels.clone(),
                capacity_memory_bytes: m.capacity_memory_bytes,
                capacity_cpu_weight: m.capacity_cpu_weight,
                used_memory_bytes: m.used_memory_bytes,
                used_cpu_weight: m.used_cpu_weight,
                last_heartbeat_epoch: m.last_heartbeat,
            })
            .collect();

        info!(%node_id, members = proto_members.len(), "node joined via gRPC");

        Ok(Response::new(proto::JoinResponse {
            node_id,
            members: proto_members,
            heartbeat_interval_secs: self.membership.heartbeat_interval_secs(),
            namespace,
        }))
    }

    async fn leave(
        &self,
        request: Request<proto::LeaveRequest>,
    ) -> Result<Response<proto::LeaveResponse>, Status> {
        let req = request.into_inner();

        let success = self
            .membership
            .leave(&req.node_id)
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(proto::LeaveResponse { success }))
    }

    async fn heartbeat(
        &self,
        request: Request<proto::HeartbeatRequest>,
    ) -> Result<Response<proto::HeartbeatResponse>, Status> {
        let req = request.into_inner();

        let acknowledged = self
            .membership
            .heartbeat(&req.node_id, req.used_memory_bytes, req.used_cpu_weight)
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(proto::HeartbeatResponse {
            acknowledged,
            commands: vec![], // Commands are populated by the scheduler.
        }))
    }

    async fn get_members(
        &self,
        _request: Request<proto::GetMembersRequest>,
    ) -> Result<Response<proto::GetMembersResponse>, Status> {
        let members = self
            .membership
            .list_members()
            .map_err(|e| Status::internal(e.to_string()))?;

        let proto_members: Vec<proto::NodeMember> = members
            .iter()
            .map(|m| proto::NodeMember {
                node_id: m.node_id.clone(),
                address: m.address.clone(),
                port: m.port as u32,
                status: match m.status {
                    crate::membership::MemberStatus::Joining => proto::NodeStatus::Joining.into(),
                    crate::membership::MemberStatus::Ready => proto::NodeStatus::Ready.into(),
                    crate::membership::MemberStatus::Draining => proto::NodeStatus::Draining.into(),
                    crate::membership::MemberStatus::Left => proto::NodeStatus::Left.into(),
                    crate::membership::MemberStatus::Dead => proto::NodeStatus::Unknown.into(),
                },
                labels: m.labels.clone(),
                capacity_memory_bytes: m.capacity_memory_bytes,
                capacity_cpu_weight: m.capacity_cpu_weight,
                used_memory_bytes: m.used_memory_bytes,
                used_cpu_weight: m.used_cpu_weight,
                last_heartbeat_epoch: m.last_heartbeat,
            })
            .collect();

        Ok(Response::new(proto::GetMembersResponse {
            members: proto_members,
        }))
    }
}
