//! Wasm component registry backed by local filesystem (beta) or S3-compatible storage.
//!
//! Compiled `.wasm` files are stored with content-addressed naming:
//! `{namespace}/{deployment_name}/{sha256}.wasm`

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Registry for Wasm component storage.
#[derive(Clone)]
pub struct WasmRegistry {
    base_dir: PathBuf,
}

impl WasmRegistry {
    /// Create a registry backed by a local directory.
    pub fn local(base_dir: &Path) -> Self {
        Self {
            base_dir: base_dir.to_path_buf(),
        }
    }

    /// Store a Wasm component and return its content hash.
    pub fn store(
        &self,
        namespace: &str,
        deployment_name: &str,
        wasm_bytes: &[u8],
    ) -> anyhow::Result<StoredComponent> {
        let hash = content_hash(wasm_bytes);
        let dir = self.base_dir.join(namespace).join(deployment_name);
        std::fs::create_dir_all(&dir)?;

        let filename = format!("{hash}.wasm");
        let path = dir.join(&filename);
        std::fs::write(&path, wasm_bytes)?;

        Ok(StoredComponent {
            hash,
            path,
            size_bytes: wasm_bytes.len() as u64,
        })
    }

    /// Retrieve a stored component by namespace, deployment, and hash.
    pub fn get(
        &self,
        namespace: &str,
        deployment_name: &str,
        hash: &str,
    ) -> anyhow::Result<Vec<u8>> {
        let path = self
            .base_dir
            .join(namespace)
            .join(deployment_name)
            .join(format!("{hash}.wasm"));
        Ok(std::fs::read(&path)?)
    }

    /// Delete all stored components for a deployment.
    pub fn delete_deployment(&self, namespace: &str, deployment_name: &str) -> anyhow::Result<()> {
        let dir = self.base_dir.join(namespace).join(deployment_name);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }
}

/// A stored Wasm component with metadata.
#[derive(Debug, Clone)]
pub struct StoredComponent {
    pub hash: String,
    pub path: PathBuf,
    pub size_bytes: u64,
}

/// Compute SHA-256 content hash of bytes.
fn content_hash(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_and_retrieve_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let registry = WasmRegistry::local(dir.path());

        let wasm = b"\x00asm\x01\x00\x00\x00"; // minimal wasm header
        let stored = registry.store("alice", "my-api", wasm).unwrap();

        assert_eq!(stored.size_bytes, 8);
        assert!(!stored.hash.is_empty());

        let retrieved = registry.get("alice", "my-api", &stored.hash).unwrap();
        assert_eq!(retrieved, wasm);
    }

    #[test]
    fn delete_deployment_removes_all() {
        let dir = tempfile::tempdir().unwrap();
        let registry = WasmRegistry::local(dir.path());

        registry.store("alice", "my-api", b"v1").unwrap();
        registry.store("alice", "my-api", b"v2").unwrap();

        registry.delete_deployment("alice", "my-api").unwrap();
        assert!(!dir.path().join("alice/my-api").exists());
    }

    #[test]
    fn content_hash_is_deterministic() {
        let h1 = content_hash(b"hello");
        let h2 = content_hash(b"hello");
        let h3 = content_hash(b"world");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }
}
