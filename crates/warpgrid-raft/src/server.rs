//! Raft gRPC server — handles incoming Raft RPCs.
//!
//! Wraps a `WarpGridRaft` instance and implements the `RaftService`
//! gRPC interface. Each RPC deserializes the JSON payload, calls the
//! corresponding openraft method, and serializes the response back.

use std::sync::Arc;

use tonic::{Request, Response, Status};
use tracing::debug;

use crate::proto;
use crate::proto::raft_service_server::RaftService;
use crate::typ::{TypeConfig, WarpGridRaft};

/// gRPC implementation of the Raft service.
pub struct RaftGrpcServer {
    raft: Arc<WarpGridRaft>,
}

impl RaftGrpcServer {
    /// Create a new Raft gRPC server wrapping the given openraft instance.
    pub fn new(raft: Arc<WarpGridRaft>) -> Self {
        Self { raft }
    }

    /// Get the tonic service for mounting on a gRPC server.
    pub fn into_service(
        self,
    ) -> proto::raft_service_server::RaftServiceServer<Self> {
        proto::raft_service_server::RaftServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl RaftService for RaftGrpcServer {
    async fn append_entries(
        &self,
        request: Request<proto::RaftRequest>,
    ) -> Result<Response<proto::RaftResponse>, Status> {
        let data = request.into_inner().data;

        let req: openraft::raft::AppendEntriesRequest<TypeConfig> =
            serde_json::from_slice(&data)
                .map_err(|e| Status::invalid_argument(format!("deserialize: {e}")))?;

        debug!(
            term = req.vote.leader_id().term,
            "handling append_entries RPC"
        );

        let result = self.raft.append_entries(req).await;

        match result {
            Ok(resp) => {
                let data = serde_json::to_vec(&resp)
                    .map_err(|e| Status::internal(format!("serialize: {e}")))?;
                Ok(Response::new(proto::RaftResponse {
                    data,
                    error: String::new(),
                }))
            }
            Err(e) => Ok(Response::new(proto::RaftResponse {
                data: Vec::new(),
                error: e.to_string(),
            })),
        }
    }

    async fn vote(
        &self,
        request: Request<proto::RaftRequest>,
    ) -> Result<Response<proto::RaftResponse>, Status> {
        let data = request.into_inner().data;

        let req: openraft::raft::VoteRequest<u64> = serde_json::from_slice(&data)
            .map_err(|e| Status::invalid_argument(format!("deserialize: {e}")))?;

        debug!(term = req.vote.leader_id().term, "handling vote RPC");

        let result = self.raft.vote(req).await;

        match result {
            Ok(resp) => {
                let data = serde_json::to_vec(&resp)
                    .map_err(|e| Status::internal(format!("serialize: {e}")))?;
                Ok(Response::new(proto::RaftResponse {
                    data,
                    error: String::new(),
                }))
            }
            Err(e) => Ok(Response::new(proto::RaftResponse {
                data: Vec::new(),
                error: e.to_string(),
            })),
        }
    }

    async fn install_snapshot(
        &self,
        request: Request<proto::RaftRequest>,
    ) -> Result<Response<proto::RaftResponse>, Status> {
        let data = request.into_inner().data;

        let req: openraft::raft::InstallSnapshotRequest<TypeConfig> =
            serde_json::from_slice(&data)
                .map_err(|e| Status::invalid_argument(format!("deserialize: {e}")))?;

        debug!("handling install_snapshot RPC");

        let result = self.raft.install_snapshot(req).await;

        match result {
            Ok(resp) => {
                let data = serde_json::to_vec(&resp)
                    .map_err(|e| Status::internal(format!("serialize: {e}")))?;
                Ok(Response::new(proto::RaftResponse {
                    data,
                    error: String::new(),
                }))
            }
            Err(e) => Ok(Response::new(proto::RaftResponse {
                data: Vec::new(),
                error: e.to_string(),
            })),
        }
    }
}
