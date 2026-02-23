//! Bidirectional mapping between String node IDs and u64 Raft node IDs.
//!
//! The cluster layer uses String node IDs (e.g., "node-abc123") while
//! openraft requires u64 IDs. This module provides a deterministic
//! mapping using a simple hash, with persistence via redb.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use tracing::debug;

/// redb table: u64 raft_id → string node_id.
const ID_TABLE: TableDefinition<u64, &str> = TableDefinition::new("raft_node_map");

/// Bidirectional map between String node_id and u64 Raft NodeId.
pub struct NodeIdMap {
    db: Arc<Database>,
    /// In-memory cache: string → u64.
    forward: RwLock<HashMap<String, u64>>,
    /// In-memory cache: u64 → string.
    reverse: RwLock<HashMap<u64, String>>,
}

impl NodeIdMap {
    /// Create a new map backed by the given redb database.
    ///
    /// Loads existing mappings from the database.
    pub fn new(db: Arc<Database>) -> Self {
        // Ensure the table exists.
        let txn = db.begin_write().expect("begin_write for node_map init");
        txn.open_table(ID_TABLE).expect("open ID_TABLE");
        txn.commit().expect("commit node_map init");

        let map = Self {
            db,
            forward: RwLock::new(HashMap::new()),
            reverse: RwLock::new(HashMap::new()),
        };
        map.load_from_db();
        map
    }

    /// Get or create a u64 ID for the given string node ID.
    ///
    /// Uses a deterministic hash. On collision, adds an offset.
    pub fn get_or_insert(&self, node_id: &str) -> u64 {
        // Check cache first.
        {
            let forward = self.forward.read().expect("forward lock");
            if let Some(&id) = forward.get(node_id) {
                return id;
            }
        }

        // Compute a deterministic ID.
        let mut raft_id = deterministic_hash(node_id);

        // Handle collisions.
        {
            let reverse = self.reverse.read().expect("reverse lock");
            while reverse.contains_key(&raft_id) {
                raft_id = raft_id.wrapping_add(1);
            }
        }

        // Persist to redb.
        if let Err(e) = self.persist(raft_id, node_id) {
            tracing::error!(node_id, raft_id, error = %e, "failed to persist node ID mapping");
        }

        // Update caches.
        {
            let mut forward = self.forward.write().expect("forward lock");
            let mut reverse = self.reverse.write().expect("reverse lock");
            forward.insert(node_id.to_string(), raft_id);
            reverse.insert(raft_id, node_id.to_string());
        }

        debug!(node_id, raft_id, "mapped node ID");
        raft_id
    }

    /// Look up the u64 Raft ID for a string node ID.
    pub fn get_raft_id(&self, node_id: &str) -> Option<u64> {
        let forward = self.forward.read().expect("forward lock");
        forward.get(node_id).copied()
    }

    /// Look up the string node ID for a u64 Raft ID.
    pub fn get_node_id(&self, raft_id: u64) -> Option<String> {
        let reverse = self.reverse.read().expect("reverse lock");
        reverse.get(&raft_id).cloned()
    }

    /// Number of mappings.
    pub fn len(&self) -> usize {
        let forward = self.forward.read().expect("forward lock");
        forward.len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn persist(&self, raft_id: u64, node_id: &str) -> Result<(), redb::Error> {
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(ID_TABLE)?;
            table.insert(raft_id, node_id)?;
        }
        txn.commit()?;
        Ok(())
    }

    fn load_from_db(&self) {
        let txn = match self.db.begin_read() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "failed to load node map from DB");
                return;
            }
        };

        let table = match txn.open_table(ID_TABLE) {
            Ok(t) => t,
            Err(_) => return,
        };

        let mut forward = self.forward.write().expect("forward lock");
        let mut reverse = self.reverse.write().expect("reverse lock");

        if let Ok(iter) = table.iter() {
            for item in iter {
                if let Ok((k, v)) = item {
                    let raft_id = k.value();
                    let node_id = v.value().to_string();
                    forward.insert(node_id.clone(), raft_id);
                    reverse.insert(raft_id, node_id);
                }
            }
        }
    }
}

/// Deterministic hash: FNV-1a 64-bit.
fn deterministic_hash(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Ensure non-zero (openraft often uses 0 as "no leader").
    if hash == 0 {
        hash = 1;
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use redb::backends::InMemoryBackend;

    fn test_db() -> Arc<Database> {
        let backend = InMemoryBackend::new();
        Arc::new(Database::builder().create_with_backend(backend).unwrap())
    }

    #[test]
    fn deterministic_hash_consistent() {
        let h1 = deterministic_hash("node-1");
        let h2 = deterministic_hash("node-1");
        assert_eq!(h1, h2);
    }

    #[test]
    fn deterministic_hash_different_inputs() {
        let h1 = deterministic_hash("node-1");
        let h2 = deterministic_hash("node-2");
        assert_ne!(h1, h2);
    }

    #[test]
    fn deterministic_hash_nonzero() {
        // Even for pathological inputs, should never return 0.
        assert_ne!(deterministic_hash(""), 0);
    }

    #[test]
    fn get_or_insert_creates_mapping() {
        let map = NodeIdMap::new(test_db());
        let id = map.get_or_insert("node-abc");

        assert_eq!(map.get_raft_id("node-abc"), Some(id));
        assert_eq!(map.get_node_id(id), Some("node-abc".to_string()));
    }

    #[test]
    fn get_or_insert_idempotent() {
        let map = NodeIdMap::new(test_db());
        let id1 = map.get_or_insert("node-1");
        let id2 = map.get_or_insert("node-1");
        assert_eq!(id1, id2);
    }

    #[test]
    fn multiple_nodes_get_unique_ids() {
        let map = NodeIdMap::new(test_db());
        let id1 = map.get_or_insert("node-1");
        let id2 = map.get_or_insert("node-2");
        let id3 = map.get_or_insert("node-3");

        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn persistence_survives_reload() {
        let db = test_db();

        // Insert some mappings.
        {
            let map = NodeIdMap::new(Arc::clone(&db));
            map.get_or_insert("node-x");
            map.get_or_insert("node-y");
        }

        // Create a new map from the same DB.
        let map = NodeIdMap::new(db);
        assert_eq!(map.len(), 2);
        assert!(map.get_raft_id("node-x").is_some());
        assert!(map.get_raft_id("node-y").is_some());
    }

    #[test]
    fn unknown_id_returns_none() {
        let map = NodeIdMap::new(test_db());
        assert!(map.get_raft_id("unknown").is_none());
        assert!(map.get_node_id(9999).is_none());
    }

    #[test]
    fn empty_map() {
        let map = NodeIdMap::new(test_db());
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
    }
}
