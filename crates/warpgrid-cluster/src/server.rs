//! Cluster gRPC server — control plane side.
//!
//! Implements the `ClusterService` gRPC interface. Runs on the
//! control plane node and handles join, heartbeat, and leave RPCs
//! from agent nodes.

use std::collections::HashMap;
use std::sync::Arc;

use tonic::{Request, Response, Status};
use tracing::info;

use crate::membership::MembershipManager;
use crate::proto;
use crate::proto::cluster_service_server::ClusterService;

/// gRPC implementation of the cluster service.
pub struct ClusterServer {
    membership: Arc<MembershipManager>,
}

impl ClusterServer {
    /// Create a new cluster server.
    pub fn new(membership: Arc<MembershipManager>) -> Self {
        Self { membership }
    }

    /// Get the tonic service for mounting on a gRPC server.
    pub fn into_service(
        self,
    ) -> proto::cluster_service_server::ClusterServiceServer<Self> {
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

        let labels: HashMap<String, String> = req.labels.into_iter().collect();

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
