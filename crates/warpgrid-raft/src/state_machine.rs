//! Raft state machine backed by redb.
//!
//! Applies committed Raft entries to produce the cluster's key-value
//! state. Supports snapshots for log compaction.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

use openraft::storage::{RaftSnapshotBuilder, RaftStateMachine};
use openraft::{
    Entry, EntryPayload, ErrorSubject, ErrorVerb, LogId, Snapshot, SnapshotMeta, StorageError,
    StoredMembership,
};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use tracing::{debug, info};

use crate::typ::{Request, Response, TypeConfig};

/// redb table for state machine key-value data.
const SM_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("raft_sm");

/// redb table for state machine metadata.
const SM_META_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("raft_sm_meta");

const APPLIED_KEY: &str = "last_applied";
const MEMBERSHIP_KEY: &str = "membership";

fn read_err(e: impl std::fmt::Display) -> StorageError<u64> {
    StorageError::from_io_error(
        ErrorSubject::Store,
        ErrorVerb::Read,
        std::io::Error::other(e.to_string()),
    )
}

fn write_err(e: impl std::fmt::Display) -> StorageError<u64> {
    StorageError::from_io_error(
        ErrorSubject::Store,
        ErrorVerb::Write,
        std::io::Error::other(e.to_string()),
    )
}

/// Raft state machine backed by redb.
pub struct StateMachine {
    db: Arc<Database>,
}

/// Snapshot builder that reads current state machine contents.
pub struct SmSnapshotBuilder {
    db: Arc<Database>,
}

impl StateMachine {
    /// Create a new state machine sharing the given redb database.
    pub fn new(db: Arc<Database>) -> Self {
        let txn = db.begin_write().expect("begin_write for SM table init");
        txn.open_table(SM_TABLE).expect("open SM_TABLE");
        txn.open_table(SM_META_TABLE).expect("open SM_META_TABLE");
        txn.commit().expect("commit SM table init");

        Self { db }
    }

    fn get_applied(&self) -> Result<Option<LogId<u64>>, StorageError<u64>> {
        let txn = self.db.begin_read().map_err(read_err)?;
        let table = txn.open_table(SM_META_TABLE).map_err(read_err)?;
        match table.get(APPLIED_KEY).map_err(read_err)? {
            Some(val) => Ok(Some(serde_json::from_slice(val.value()).map_err(read_err)?)),
            None => Ok(None),
        }
    }

    fn get_membership(
        &self,
    ) -> Result<StoredMembership<u64, openraft::BasicNode>, StorageError<u64>> {
        let txn = self.db.begin_read().map_err(read_err)?;
        let table = txn.open_table(SM_META_TABLE).map_err(read_err)?;
        match table.get(MEMBERSHIP_KEY).map_err(read_err)? {
            Some(val) => Ok(serde_json::from_slice(val.value()).map_err(read_err)?),
            None => Ok(StoredMembership::default()),
        }
    }

    fn apply_request(&self, req: &Request) -> Result<(), StorageError<u64>> {
        let txn = self.db.begin_write().map_err(write_err)?;
        {
            let mut table = txn.open_table(SM_TABLE).map_err(write_err)?;
            match req {
                Request::PutDeployment { key, value }
                | Request::PutInstance { key, value }
                | Request::PutNode { key, value } => {
                    table
                        .insert(key.as_str(), value.as_bytes())
                        .map_err(write_err)?;
                }
                Request::DeleteDeployment { key }
                | Request::DeleteInstance { key }
                | Request::DeleteNode { key } => {
                    table.remove(key.as_str()).map_err(write_err)?;
                }
            }
        }
        txn.commit().map_err(write_err)?;
        debug!("applied request to state machine");
        Ok(())
    }

    fn save_meta(&self, key: &str, data: &[u8]) -> Result<(), StorageError<u64>> {
        let txn = self.db.begin_write().map_err(write_err)?;
        {
            let mut table = txn.open_table(SM_META_TABLE).map_err(write_err)?;
            table.insert(key, data).map_err(write_err)?;
        }
        txn.commit().map_err(write_err)?;
        Ok(())
    }
}

impl RaftStateMachine<TypeConfig> for StateMachine {
    type SnapshotBuilder = SmSnapshotBuilder;

    async fn applied_state(
        &mut self,
    ) -> Result<
        (
            Option<LogId<u64>>,
            StoredMembership<u64, openraft::BasicNode>,
        ),
        StorageError<u64>,
    > {
        let applied = self.get_applied()?;
        let membership = self.get_membership()?;
        Ok((applied, membership))
    }

    async fn apply<I>(&mut self, entries: I) -> Result<Vec<Response>, StorageError<u64>>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + Send,
        I::IntoIter: Send,
    {
        let mut responses = Vec::new();

        for entry in entries {
            let log_id = entry.log_id;

            match entry.payload {
                EntryPayload::Blank => {
                    responses.push(Response { success: true });
                }
                EntryPayload::Normal(req) => {
                    self.apply_request(&req)?;
                    responses.push(Response { success: true });
                }
                EntryPayload::Membership(membership) => {
                    let stored = StoredMembership::new(Some(log_id), membership);
                    let data = serde_json::to_vec(&stored).map_err(write_err)?;
                    self.save_meta(MEMBERSHIP_KEY, &data)?;
                    responses.push(Response { success: true });
                }
            }

            // Update last applied.
            let data = serde_json::to_vec(&log_id).map_err(write_err)?;
            self.save_meta(APPLIED_KEY, &data)?;
        }

        Ok(responses)
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        SmSnapshotBuilder {
            db: Arc::clone(&self.db),
        }
    }

    async fn begin_receiving_snapshot(
        &mut self,
    ) -> Result<Box<Cursor<Vec<u8>>>, StorageError<u64>> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }

    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta<u64, openraft::BasicNode>,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError<u64>> {
        let data = snapshot.into_inner();
        let kv_pairs: BTreeMap<String, String> =
            serde_json::from_slice(&data).map_err(read_err)?;

        // Clear current state and load from snapshot.
        let txn = self.db.begin_write().map_err(write_err)?;
        {
            let mut table = txn.open_table(SM_TABLE).map_err(write_err)?;

            let keys: Vec<String> = table
                .iter()
                .map_err(write_err)?
                .map(|item| item.map(|(k, _)| k.value().to_string()))
                .collect::<Result<_, _>>()
                .map_err(write_err)?;
            for key in &keys {
                table.remove(key.as_str()).map_err(write_err)?;
            }

            for (k, v) in &kv_pairs {
                table
                    .insert(k.as_str(), v.as_bytes())
                    .map_err(write_err)?;
            }
        }
        txn.commit().map_err(write_err)?;

        // Update metadata from snapshot.
        let applied_data =
            serde_json::to_vec(&meta.last_log_id).map_err(write_err)?;
        self.save_meta(APPLIED_KEY, &applied_data)?;

        let membership_data =
            serde_json::to_vec(&meta.last_membership).map_err(write_err)?;
        self.save_meta(MEMBERSHIP_KEY, &membership_data)?;

        info!("installed snapshot");
        Ok(())
    }

    async fn get_current_snapshot(
        &mut self,
    ) -> Result<Option<Snapshot<TypeConfig>>, StorageError<u64>> {
        let applied = self.get_applied()?;
        if applied.is_none() {
            return Ok(None);
        }

        let mut builder = SmSnapshotBuilder {
            db: Arc::clone(&self.db),
        };
        let snapshot = builder.build_snapshot().await?;
        Ok(Some(snapshot))
    }
}

impl RaftSnapshotBuilder<TypeConfig> for SmSnapshotBuilder {
    async fn build_snapshot(&mut self) -> Result<Snapshot<TypeConfig>, StorageError<u64>> {
        let txn = self.db.begin_read().map_err(read_err)?;

        let table = txn.open_table(SM_TABLE).map_err(read_err)?;
        let mut kv_pairs = BTreeMap::new();
        for item in table.iter().map_err(read_err)? {
            let (k, v) = item.map_err(read_err)?;
            kv_pairs.insert(
                k.value().to_string(),
                String::from_utf8_lossy(v.value()).to_string(),
            );
        }

        let meta_table = txn.open_table(SM_META_TABLE).map_err(read_err)?;

        let last_applied: Option<LogId<u64>> = match meta_table
            .get(APPLIED_KEY)
            .map_err(read_err)?
        {
            Some(val) => Some(serde_json::from_slice(val.value()).map_err(read_err)?),
            None => None,
        };

        let membership: StoredMembership<u64, openraft::BasicNode> = match meta_table
            .get(MEMBERSHIP_KEY)
            .map_err(read_err)?
        {
            Some(val) => serde_json::from_slice(val.value()).map_err(read_err)?,
            None => StoredMembership::default(),
        };

        drop(meta_table);
        drop(table);
        drop(txn);

        let data = serde_json::to_vec(&kv_pairs).map_err(read_err)?;

        let snapshot_id = format!("snap-{}", last_applied.map_or(0, |l| l.index));

        let meta = SnapshotMeta {
            last_log_id: last_applied,
            last_membership: membership,
            snapshot_id,
        };

        Ok(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(data)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openraft::CommittedLeaderId;
    use redb::backends::InMemoryBackend;

    fn test_db() -> Arc<Database> {
        let backend = InMemoryBackend::new();
        Arc::new(Database::builder().create_with_backend(backend).unwrap())
    }

    #[tokio::test]
    async fn empty_state_machine() {
        let mut sm = StateMachine::new(test_db());
        let (applied, _membership) = sm.applied_state().await.unwrap();
        assert!(applied.is_none());
    }

    #[tokio::test]
    async fn apply_put_and_read_back() {
        let db = test_db();
        let mut sm = StateMachine::new(Arc::clone(&db));

        let entry = Entry::<TypeConfig> {
            log_id: LogId::new(CommittedLeaderId::new(1, 1), 1),
            payload: EntryPayload::Normal(Request::PutDeployment {
                key: "ns/app".to_string(),
                value: r#"{"name":"app"}"#.to_string(),
            }),
        };

        let responses = sm.apply([entry]).await.unwrap();
        assert_eq!(responses.len(), 1);
        assert!(responses[0].success);

        // Verify the value in the SM table.
        let txn = db.begin_read().unwrap();
        let table = txn.open_table(SM_TABLE).unwrap();
        let val = table.get("ns/app").unwrap().unwrap();
        assert_eq!(
            String::from_utf8_lossy(val.value()),
            r#"{"name":"app"}"#
        );
    }

    #[tokio::test]
    async fn snapshot_roundtrip() {
        let db = test_db();
        let mut sm = StateMachine::new(Arc::clone(&db));

        let entry = Entry::<TypeConfig> {
            log_id: LogId::new(CommittedLeaderId::new(1, 1), 1),
            payload: EntryPayload::Normal(Request::PutDeployment {
                key: "test/key".to_string(),
                value: "test-value".to_string(),
            }),
        };
        sm.apply([entry]).await.unwrap();

        let mut builder = sm.get_snapshot_builder().await;
        let snapshot = builder.build_snapshot().await.unwrap();

        assert_eq!(snapshot.meta.snapshot_id, "snap-1");
    }
}
