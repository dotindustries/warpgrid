//! Raft type configuration for WarpGrid.
//!
//! Defines the `TypeConfig` that wires together all openraft
//! associated types: node IDs, request/response payloads, and
//! the async runtime.

use std::io::Cursor;

use openraft::TokioRuntime;

/// Client write request submitted to the Raft cluster.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Request {
    /// Store or update a deployment spec (JSON payload).
    PutDeployment { key: String, value: String },
    /// Remove a deployment.
    DeleteDeployment { key: String },
    /// Store or update an instance state.
    PutInstance { key: String, value: String },
    /// Remove an instance.
    DeleteInstance { key: String },
    /// Store or update a node record.
    PutNode { key: String, value: String },
    /// Remove a node.
    DeleteNode { key: String },
}

/// Response returned after a write is applied to the state machine.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Response {
    pub success: bool,
}

openraft::declare_raft_types!(
    /// WarpGrid Raft type configuration.
    pub TypeConfig:
        D = Request,
        R = Response,
        NodeId = u64,
        Node = openraft::BasicNode,
        Entry = openraft::Entry<TypeConfig>,
        SnapshotData = Cursor<Vec<u8>>,
        AsyncRuntime = TokioRuntime,
);

/// Convenience alias for the Raft instance.
pub type WarpGridRaft = openraft::Raft<TypeConfig>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serializes_roundtrip() {
        let req = Request::PutDeployment {
            key: "ns/app".to_string(),
            value: r#"{"name":"app"}"#.to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        match back {
            Request::PutDeployment { key, value } => {
                assert_eq!(key, "ns/app");
                assert!(value.contains("app"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_serializes_roundtrip() {
        let resp = Response { success: true };
        let json = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        assert!(back.success);
    }
}
