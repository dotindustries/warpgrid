//! Raft log storage backed by redb.
//!
//! Stores the Raft vote, committed index, and log entries in redb
//! tables. Each entry is JSON-serialized for simplicity.

use std::fmt::Debug;
use std::ops::RangeBounds;
use std::sync::Arc;

use openraft::storage::{LogFlushed, LogState, RaftLogReader, RaftLogStorage};
use openraft::{Entry, ErrorSubject, ErrorVerb, LogId, StorageError, Vote};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use tracing::debug;

use crate::typ::TypeConfig;

/// redb table for log entries: key = log index (u64), value = JSON bytes.
const LOG_TABLE: TableDefinition<u64, &[u8]> = TableDefinition::new("raft_log");

/// redb table for metadata: key = name string, value = JSON bytes.
const META_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("raft_meta");

const VOTE_KEY: &str = "vote";
const COMMITTED_KEY: &str = "committed";
const LAST_PURGED_KEY: &str = "last_purged";

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

/// Raft log storage backed by redb.
pub struct LogStore {
    db: Arc<Database>,
}

/// Read-only log reader (cloned from LogStore).
pub struct LogReader {
    db: Arc<Database>,
}

impl LogStore {
    /// Create a new log store sharing the given redb database.
    pub fn new(db: Arc<Database>) -> Self {
        let txn = db.begin_write().expect("begin_write for table init");
        txn.open_table(LOG_TABLE).expect("open LOG_TABLE");
        txn.open_table(META_TABLE).expect("open META_TABLE");
        txn.commit().expect("commit table init");

        Self { db }
    }

    fn write_meta(&self, key: &str, data: &[u8]) -> Result<(), StorageError<u64>> {
        let txn = self.db.begin_write().map_err(write_err)?;
        {
            let mut table = txn.open_table(META_TABLE).map_err(write_err)?;
            table.insert(key, data).map_err(write_err)?;
        }
        txn.commit().map_err(write_err)?;
        Ok(())
    }

    fn read_meta(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError<u64>> {
        let txn = self.db.begin_read().map_err(read_err)?;
        let table = txn.open_table(META_TABLE).map_err(read_err)?;
        match table.get(key).map_err(read_err)? {
            Some(val) => Ok(Some(val.value().to_vec())),
            None => Ok(None),
        }
    }
}

impl RaftLogReader<TypeConfig> for LogReader {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + Send>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry<TypeConfig>>, StorageError<u64>> {
        let txn = self.db.begin_read().map_err(read_err)?;
        let table = txn.open_table(LOG_TABLE).map_err(read_err)?;

        let mut entries = Vec::new();
        let iter = table.range(range).map_err(read_err)?;

        for item in iter {
            let (_, val) = item.map_err(read_err)?;
            let entry: Entry<TypeConfig> =
                serde_json::from_slice(val.value()).map_err(read_err)?;
            entries.push(entry);
        }

        Ok(entries)
    }
}

impl RaftLogReader<TypeConfig> for LogStore {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + Send>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry<TypeConfig>>, StorageError<u64>> {
        let mut reader = LogReader {
            db: Arc::clone(&self.db),
        };
        reader.try_get_log_entries(range).await
    }
}

impl RaftLogStorage<TypeConfig> for LogStore {
    type LogReader = LogReader;

    async fn get_log_state(&mut self) -> Result<LogState<TypeConfig>, StorageError<u64>> {
        let txn = self.db.begin_read().map_err(read_err)?;
        let table = txn.open_table(LOG_TABLE).map_err(read_err)?;

        let last_log_id = match table.last().map_err(read_err)? {
            Some((_, val)) => {
                let entry: Entry<TypeConfig> =
                    serde_json::from_slice(val.value()).map_err(read_err)?;
                Some(entry.log_id)
            }
            None => None,
        };

        drop(table);
        drop(txn);

        let last_purged_log_id = match self.read_meta(LAST_PURGED_KEY)? {
            Some(data) => Some(serde_json::from_slice::<LogId<u64>>(&data).map_err(read_err)?),
            None => None,
        };

        Ok(LogState {
            last_purged_log_id,
            last_log_id,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        LogReader {
            db: Arc::clone(&self.db),
        }
    }

    async fn save_vote(&mut self, vote: &Vote<u64>) -> Result<(), StorageError<u64>> {
        let data = serde_json::to_vec(vote).map_err(write_err)?;
        self.write_meta(VOTE_KEY, &data)?;
        debug!("saved vote");
        Ok(())
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<u64>>, StorageError<u64>> {
        match self.read_meta(VOTE_KEY)? {
            Some(data) => {
                let vote: Vote<u64> = serde_json::from_slice(&data).map_err(read_err)?;
                Ok(Some(vote))
            }
            None => Ok(None),
        }
    }

    async fn append<I>(
        &mut self,
        entries: I,
        callback: LogFlushed<TypeConfig>,
    ) -> Result<(), StorageError<u64>>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + Send,
        I::IntoIter: Send,
    {
        let txn = self.db.begin_write().map_err(write_err)?;
        {
            let mut table = txn.open_table(LOG_TABLE).map_err(write_err)?;
            for entry in entries {
                let index = entry.log_id.index;
                let data = serde_json::to_vec(&entry).map_err(write_err)?;
                table.insert(index, data.as_slice()).map_err(write_err)?;
            }
        }
        txn.commit().map_err(write_err)?;

        callback.log_io_completed(Ok(()));
        Ok(())
    }

    async fn truncate(&mut self, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
        let txn = self.db.begin_write().map_err(write_err)?;
        {
            let mut table = txn.open_table(LOG_TABLE).map_err(write_err)?;
            let keys: Vec<u64> = table
                .range(log_id.index..)
                .map_err(write_err)?
                .map(|item| item.map(|(k, _)| k.value()))
                .collect::<Result<_, _>>()
                .map_err(write_err)?;

            for key in keys {
                table.remove(key).map_err(write_err)?;
            }
        }
        txn.commit().map_err(write_err)?;
        debug!(index = log_id.index, "truncated log");
        Ok(())
    }

    async fn purge(&mut self, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
        let data = serde_json::to_vec(&log_id).map_err(write_err)?;
        self.write_meta(LAST_PURGED_KEY, &data)?;

        let txn = self.db.begin_write().map_err(write_err)?;
        {
            let mut table = txn.open_table(LOG_TABLE).map_err(write_err)?;
            let keys: Vec<u64> = table
                .range(..=log_id.index)
                .map_err(write_err)?
                .map(|item| item.map(|(k, _)| k.value()))
                .collect::<Result<_, _>>()
                .map_err(write_err)?;

            for key in keys {
                table.remove(key).map_err(write_err)?;
            }
        }
        txn.commit().map_err(write_err)?;
        debug!(index = log_id.index, "purged log");
        Ok(())
    }

    async fn save_committed(
        &mut self,
        committed: Option<LogId<u64>>,
    ) -> Result<(), StorageError<u64>> {
        if let Some(log_id) = committed {
            let data = serde_json::to_vec(&log_id).map_err(write_err)?;
            self.write_meta(COMMITTED_KEY, &data)?;
        }
        Ok(())
    }

    async fn read_committed(&mut self) -> Result<Option<LogId<u64>>, StorageError<u64>> {
        match self.read_meta(COMMITTED_KEY)? {
            Some(data) => {
                let log_id: LogId<u64> = serde_json::from_slice(&data).map_err(read_err)?;
                Ok(Some(log_id))
            }
            None => Ok(None),
        }
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
    async fn log_store_empty_state() {
        let mut store = LogStore::new(test_db());
        let state = store.get_log_state().await.unwrap();
        assert!(state.last_log_id.is_none());
    }

    #[tokio::test]
    async fn vote_save_and_read() {
        let mut store = LogStore::new(test_db());
        assert!(store.read_vote().await.unwrap().is_none());

        let vote = Vote::new(1, 1);
        store.save_vote(&vote).await.unwrap();

        let read_back = store.read_vote().await.unwrap().unwrap();
        assert_eq!(read_back, vote);
    }

    #[tokio::test]
    async fn write_and_read_entries_via_redb() {
        // LogFlushed::new is pub(crate) in openraft, so we test the
        // reader by writing entries directly to the redb table.
        let db = test_db();
        let mut store = LogStore::new(Arc::clone(&db));

        let entry = Entry::<TypeConfig> {
            log_id: LogId::new(CommittedLeaderId::new(1, 1), 0),
            payload: openraft::EntryPayload::Blank,
        };
        let data = serde_json::to_vec(&entry).unwrap();

        // Write directly to the log table.
        let txn = db.begin_write().unwrap();
        {
            let mut table = txn.open_table(LOG_TABLE).unwrap();
            table.insert(0u64, data.as_slice()).unwrap();
        }
        txn.commit().unwrap();

        // Verify the reader can retrieve the entry.
        let entries = store.try_get_log_entries(0..=0).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].log_id.index, 0);
    }

    #[tokio::test]
    async fn committed_save_and_read() {
        let mut store = LogStore::new(test_db());
        assert!(store.read_committed().await.unwrap().is_none());

        let log_id = LogId::new(CommittedLeaderId::new(1, 1), 5);
        store.save_committed(Some(log_id)).await.unwrap();

        let read_back = store.read_committed().await.unwrap().unwrap();
        assert_eq!(read_back, log_id);
    }
}
