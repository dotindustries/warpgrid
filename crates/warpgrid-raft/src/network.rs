//! Raft network layer backed by tonic gRPC.
//!
//! Implements `RaftNetworkFactory` and `RaftNetwork` so that openraft
//! can communicate between cluster nodes. Each RPC serializes the
//! openraft request to JSON, sends it via gRPC, and deserializes the
//! response.

use openraft::error::{InstallSnapshotError, RPCError, RaftError, RemoteError, Unreachable};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest,
    InstallSnapshotResponse, VoteRequest, VoteResponse,
};
use openraft::BasicNode;
use tracing::{debug, warn};

use crate::proto::raft_service_client::RaftServiceClient;
use crate::proto::RaftRequest;
use crate::typ::TypeConfig;

/// Factory that creates per-peer gRPC connections.
pub struct NetworkFactory;

/// A single peer connection backed by a tonic gRPC channel.
pub struct NetworkConnection {
    target: u64,
    addr: String,
    client: Option<RaftServiceClient<tonic::transport::Channel>>,
}

impl NetworkConnection {
    fn mk_unreachable<E: std::error::Error>(target: u64, addr: &str, msg: &str) -> RPCError<u64, BasicNode, E> {
        RPCError::Unreachable(Unreachable::new(&std::io::Error::other(format!(
            "raft gRPC to node {target} ({addr}): {msg}",
        ))))
    }

    async fn get_client(&mut self) -> Result<&mut RaftServiceClient<tonic::transport::Channel>, String> {
        if self.client.is_some() {
            return Ok(self.client.as_mut().unwrap());
        }

        let endpoint = format!("http://{}", self.addr);
        let ep = tonic::transport::Endpoint::from_shared(endpoint.clone())
            .map_err(|e| format!("invalid endpoint {endpoint}: {e}"))?;

        let channel = ep.connect().await
            .map_err(|e| {
                warn!(target_node = self.target, addr = %self.addr, error = %e, "failed to connect");
                format!("connect to {endpoint}: {e}")
            })?;

        debug!(target_node = self.target, addr = %self.addr, "connected to raft peer");
        self.client = Some(RaftServiceClient::new(channel));
        Ok(self.client.as_mut().unwrap())
    }
}

impl RaftNetworkFactory<TypeConfig> for NetworkFactory {
    type Network = NetworkConnection;

    async fn new_client(&mut self, target: u64, node: &BasicNode) -> Self::Network {
        debug!(target, addr = %node.addr, "creating raft network connection");
        NetworkConnection {
            target,
            addr: node.addr.clone(),
            client: None,
        }
    }
}

impl RaftNetwork<TypeConfig> for NetworkConnection {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        AppendEntriesResponse<u64>,
        RPCError<u64, BasicNode, RaftError<u64>>,
    > {
        let target = self.target;
        let addr = self.addr.clone();

        let data = serde_json::to_vec(&rpc)
            .map_err(|e| Self::mk_unreachable::<RaftError<u64>>(target, &addr, &format!("serialize: {e}")))?;

        let client = self.get_client().await
            .map_err(|e| Self::mk_unreachable::<RaftError<u64>>(target, &addr, &e))?;

        let response = client
            .append_entries(RaftRequest { data })
            .await
            .map_err(|e| {
                self.client = None;
                Self::mk_unreachable::<RaftError<u64>>(target, &addr, &format!("gRPC: {e}"))
            })?;

        let inner = response.into_inner();
        if !inner.error.is_empty() {
            let raft_err: RaftError<u64> = serde_json::from_str(&inner.error)
                .unwrap_or_else(|_| RaftError::Fatal(openraft::error::Fatal::Panicked));
            return Err(RPCError::RemoteError(RemoteError::new(target, raft_err)));
        }

        serde_json::from_slice(&inner.data)
            .map_err(|e| Self::mk_unreachable(target, &addr, &format!("deserialize response: {e}")))
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<u64>,
        RPCError<u64, BasicNode, RaftError<u64, InstallSnapshotError>>,
    > {
        let target = self.target;
        let addr = self.addr.clone();

        let data = serde_json::to_vec(&rpc)
            .map_err(|e| Self::mk_unreachable::<RaftError<u64, InstallSnapshotError>>(target, &addr, &format!("serialize: {e}")))?;

        let client = self.get_client().await
            .map_err(|e| Self::mk_unreachable::<RaftError<u64, InstallSnapshotError>>(target, &addr, &e))?;

        let response = client
            .install_snapshot(RaftRequest { data })
            .await
            .map_err(|e| {
                self.client = None;
                Self::mk_unreachable::<RaftError<u64, InstallSnapshotError>>(target, &addr, &format!("gRPC: {e}"))
            })?;

        let inner = response.into_inner();
        if !inner.error.is_empty() {
            let raft_err: RaftError<u64, InstallSnapshotError> = serde_json::from_str(&inner.error)
                .unwrap_or_else(|_| RaftError::Fatal(openraft::error::Fatal::Panicked));
            return Err(RPCError::RemoteError(RemoteError::new(target, raft_err)));
        }

        serde_json::from_slice(&inner.data)
            .map_err(|e| Self::mk_unreachable(target, &addr, &format!("deserialize response: {e}")))
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<u64>,
        _option: RPCOption,
    ) -> Result<VoteResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        let target = self.target;
        let addr = self.addr.clone();

        let data = serde_json::to_vec(&rpc)
            .map_err(|e| Self::mk_unreachable::<RaftError<u64>>(target, &addr, &format!("serialize: {e}")))?;

        let client = self.get_client().await
            .map_err(|e| Self::mk_unreachable::<RaftError<u64>>(target, &addr, &e))?;

        let response = client
            .vote(RaftRequest { data })
            .await
            .map_err(|e| {
                self.client = None;
                Self::mk_unreachable::<RaftError<u64>>(target, &addr, &format!("gRPC: {e}"))
            })?;

        let inner = response.into_inner();
        if !inner.error.is_empty() {
            let raft_err: RaftError<u64> = serde_json::from_str(&inner.error)
                .unwrap_or_else(|_| RaftError::Fatal(openraft::error::Fatal::Panicked));
            return Err(RPCError::RemoteError(RemoteError::new(target, raft_err)));
        }

        serde_json::from_slice(&inner.data)
            .map_err(|e| Self::mk_unreachable(target, &addr, &format!("deserialize response: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn factory_creates_connection() {
        let mut factory = NetworkFactory;
        let node = BasicNode::new("127.0.0.1:9100");
        let conn = factory.new_client(1, &node).await;
        assert_eq!(conn.target, 1);
        assert_eq!(conn.addr, "127.0.0.1:9100");
        assert!(conn.client.is_none()); // Lazy connect.
    }

    #[test]
    fn serialization_roundtrips() {
        let vote = openraft::Vote::<u64>::new(1, 2);
        let req = VoteRequest::<u64> { vote, last_log_id: None };
        let data = serde_json::to_vec(&req).unwrap();
        let back: VoteRequest<u64> = serde_json::from_slice(&data).unwrap();
        assert_eq!(back.vote, vote);
    }
}
