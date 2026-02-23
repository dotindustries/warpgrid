//! Raft network layer backed by tonic gRPC.
//!
//! Implements `RaftNetworkFactory` and `RaftNetwork` so that openraft
//! can communicate between cluster nodes. For Phase 2 MVP the actual
//! gRPC transport returns `Unreachable` — integration with the
//! warpgrid-cluster proto comes in a later milestone.

use openraft::error::{InstallSnapshotError, RPCError, RaftError, Unreachable};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest,
    InstallSnapshotResponse, VoteRequest, VoteResponse,
};
use openraft::BasicNode;
use tracing::{debug, warn};

use crate::typ::TypeConfig;

/// Factory that creates per-peer gRPC connections.
pub struct NetworkFactory;

/// A single peer connection.
pub struct NetworkConnection {
    target: u64,
    addr: String,
}

impl NetworkConnection {
    fn unreachable_err<E: std::error::Error>(&self) -> RPCError<u64, BasicNode, E> {
        RPCError::Unreachable(Unreachable::new(&std::io::Error::other(format!(
            "raft gRPC transport not yet wired (target={}, addr={})",
            self.target, self.addr
        ))))
    }
}

impl RaftNetworkFactory<TypeConfig> for NetworkFactory {
    type Network = NetworkConnection;

    async fn new_client(&mut self, target: u64, node: &BasicNode) -> Self::Network {
        debug!(target, addr = %node.addr, "creating raft network connection");
        NetworkConnection {
            target,
            addr: node.addr.clone(),
        }
    }
}

impl RaftNetwork<TypeConfig> for NetworkConnection {
    async fn append_entries(
        &mut self,
        _rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        AppendEntriesResponse<u64>,
        RPCError<u64, BasicNode, RaftError<u64>>,
    > {
        warn!(target = self.target, "append_entries: transport not wired");
        Err(self.unreachable_err())
    }

    async fn install_snapshot(
        &mut self,
        _rpc: InstallSnapshotRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<u64>,
        RPCError<u64, BasicNode, RaftError<u64, InstallSnapshotError>>,
    > {
        warn!(target = self.target, "install_snapshot: transport not wired");
        Err(self.unreachable_err())
    }

    async fn vote(
        &mut self,
        _rpc: VoteRequest<u64>,
        _option: RPCOption,
    ) -> Result<VoteResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        warn!(target = self.target, "vote: transport not wired");
        Err(self.unreachable_err())
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
    }
}
