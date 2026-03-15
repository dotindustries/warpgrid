//! Integration tests for warpgrid-raft log store and state machine.
//!
//! All tests use in-memory redb backends — no disk I/O, no external services.

use std::collections::BTreeSet;
use std::io::Cursor;
use std::sync::Arc;

use openraft::storage::{RaftLogReader, RaftLogStorage, RaftSnapshotBuilder, RaftStateMachine};
use openraft::{CommittedLeaderId, Entry, EntryPayload, LogId, Membership, Vote};
use redb::backends::InMemoryBackend;
use redb::{Database, ReadableDatabase};

use warpgrid_raft::log_store::LogStore;
use warpgrid_raft::state_machine::StateMachine;
use warpgrid_raft::typ::{Request, TypeConfig};

// ── Helpers ──────────────────────────────────────────────────────────

fn test_db() -> Arc<Database> {
    let backend = InMemoryBackend::new();
    Arc::new(Database::builder().create_with_backend(backend).unwrap())
}

fn leader(term: u64, node: u64) -> CommittedLeaderId<u64> {
    CommittedLeaderId::new(term, node)
}

fn blank_entry(term: u64, node: u64, index: u64) -> Entry<TypeConfig> {
    Entry {
        log_id: LogId::new(leader(term, node), index),
        payload: EntryPayload::Blank,
    }
}

fn put_deployment_entry(term: u64, node: u64, index: u64, key: &str, value: &str) -> Entry<TypeConfig> {
    Entry {
        log_id: LogId::new(leader(term, node), index),
        payload: EntryPayload::Normal(Request::PutDeployment {
            key: key.to_string(),
            value: value.to_string(),
        }),
    }
}

/// Write entries to the log store via the redb table directly,
/// since `LogFlushed::new` is pub(crate) in openraft.
fn write_entries_direct(db: &Database, entries: &[Entry<TypeConfig>]) {
    let txn = db.begin_write().unwrap();
    {
        let log_table = redb::TableDefinition::<u64, &[u8]>::new("raft_log");
        let mut table = txn.open_table(log_table).unwrap();
        for entry in entries {
            let data = serde_json::to_vec(entry).unwrap();
            table.insert(entry.log_id.index, data.as_slice()).unwrap();
        }
    }
    txn.commit().unwrap();
}

// ── Test 1: Log store append/truncate/purge lifecycle ────────────────

#[tokio::test]
async fn log_store_append_truncate_purge_lifecycle() {
    let db = test_db();
    let mut store = LogStore::new(Arc::clone(&db));

    // Initially empty.
    let state = store.get_log_state().await.unwrap();
    assert!(state.last_log_id.is_none());
    assert!(state.last_purged_log_id.is_none());

    // Append entries via direct write (LogFlushed is pub(crate)).
    let entries = vec![
        blank_entry(1, 1, 0),
        blank_entry(1, 1, 1),
        blank_entry(1, 1, 2),
        blank_entry(1, 1, 3),
        blank_entry(1, 1, 4),
    ];
    write_entries_direct(&db, &entries);

    // Verify all 5 entries readable.
    let read = store.try_get_log_entries(0..=4).await.unwrap();
    assert_eq!(read.len(), 5);

    // Log state shows last entry.
    let state = store.get_log_state().await.unwrap();
    assert_eq!(state.last_log_id.unwrap().index, 4);

    // Truncate from index 3 onwards (removes entries 3 and 4).
    let truncate_id = LogId::new(leader(1, 1), 3);
    store.truncate(truncate_id).await.unwrap();

    let read = store.try_get_log_entries(0..=4).await.unwrap();
    assert_eq!(read.len(), 3);
    assert_eq!(read.last().unwrap().log_id.index, 2);

    // Purge up to index 1 (removes entries 0 and 1).
    let purge_id = LogId::new(leader(1, 1), 1);
    store.purge(purge_id).await.unwrap();

    let read = store.try_get_log_entries(0..=4).await.unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].log_id.index, 2);

    // last_purged_log_id updated.
    let state = store.get_log_state().await.unwrap();
    assert_eq!(state.last_purged_log_id.unwrap().index, 1);
}

// ── Test 2: State machine multi-operation sequence ───────────────────

#[tokio::test]
async fn state_machine_multi_operation_sequence() {
    let db = test_db();
    let mut sm = StateMachine::new(Arc::clone(&db));

    // Apply a mix of PutDeployment, PutInstance, PutNode, then deletes.
    let entries = vec![
        Entry {
            log_id: LogId::new(leader(1, 1), 1),
            payload: EntryPayload::Normal(Request::PutDeployment {
                key: "prod/api".to_string(),
                value: r#"{"replicas":3}"#.to_string(),
            }),
        },
        Entry {
            log_id: LogId::new(leader(1, 1), 2),
            payload: EntryPayload::Normal(Request::PutInstance {
                key: "prod/api:inst-0".to_string(),
                value: r#"{"status":"running"}"#.to_string(),
            }),
        },
        Entry {
            log_id: LogId::new(leader(1, 1), 3),
            payload: EntryPayload::Normal(Request::PutNode {
                key: "node-1".to_string(),
                value: r#"{"addr":"10.0.0.1"}"#.to_string(),
            }),
        },
        Entry {
            log_id: LogId::new(leader(1, 1), 4),
            payload: EntryPayload::Normal(Request::DeleteInstance {
                key: "prod/api:inst-0".to_string(),
            }),
        },
    ];

    let responses = sm.apply(entries).await.unwrap();
    assert_eq!(responses.len(), 4);
    assert!(responses.iter().all(|r| r.success));

    // Verify last applied.
    let (applied, _membership) = sm.applied_state().await.unwrap();
    assert_eq!(applied.unwrap().index, 4);

    // Verify data in SM table.
    let sm_table = redb::TableDefinition::<&str, &[u8]>::new("raft_sm");
    let txn = db.begin_read().unwrap();
    let table = txn.open_table(sm_table).unwrap();

    // Deployment persisted.
    let val = table.get("prod/api").unwrap().unwrap();
    assert_eq!(String::from_utf8_lossy(val.value()), r#"{"replicas":3}"#);

    // Node persisted.
    let val = table.get("node-1").unwrap().unwrap();
    assert_eq!(String::from_utf8_lossy(val.value()), r#"{"addr":"10.0.0.1"}"#);

    // Instance was deleted.
    assert!(table.get("prod/api:inst-0").unwrap().is_none());
}

// ── Test 3: Snapshot install restores state ──────────────────────────

#[tokio::test]
async fn snapshot_install_restores_state() {
    // Build state in one state machine.
    let db1 = test_db();
    let mut sm1 = StateMachine::new(Arc::clone(&db1));

    let entries = vec![
        put_deployment_entry(1, 1, 1, "ns/app-a", "value-a"),
        put_deployment_entry(1, 1, 2, "ns/app-b", "value-b"),
    ];
    sm1.apply(entries).await.unwrap();

    // Build snapshot from sm1.
    let mut builder = sm1.get_snapshot_builder().await;
    let snapshot = builder.build_snapshot().await.unwrap();

    assert_eq!(snapshot.meta.snapshot_id, "snap-2");
    assert_eq!(snapshot.meta.last_log_id.unwrap().index, 2);

    // Install snapshot into a fresh state machine.
    let db2 = test_db();
    let mut sm2 = StateMachine::new(Arc::clone(&db2));

    // Verify sm2 is empty before install.
    let (applied, _) = sm2.applied_state().await.unwrap();
    assert!(applied.is_none());

    // Extract snapshot data for install.
    let snap_data = snapshot.snapshot.into_inner();
    let cursor = Box::new(Cursor::new(snap_data));

    sm2.install_snapshot(&snapshot.meta, cursor).await.unwrap();

    // Verify sm2 now has the data.
    let (applied, _) = sm2.applied_state().await.unwrap();
    assert_eq!(applied.unwrap().index, 2);

    // Verify key-value data restored.
    let sm_table = redb::TableDefinition::<&str, &[u8]>::new("raft_sm");
    let txn = db2.begin_read().unwrap();
    let table = txn.open_table(sm_table).unwrap();

    let val = table.get("ns/app-a").unwrap().unwrap();
    assert_eq!(String::from_utf8_lossy(val.value()), "value-a");

    let val = table.get("ns/app-b").unwrap().unwrap();
    assert_eq!(String::from_utf8_lossy(val.value()), "value-b");
}

// ── Test 4: Membership change persisted ──────────────────────────────

#[tokio::test]
async fn membership_change_persisted() {
    let db = test_db();
    let mut sm = StateMachine::new(Arc::clone(&db));

    // Initial membership is default (empty).
    let (_, membership) = sm.applied_state().await.unwrap();
    assert!(membership.log_id().is_none());

    // Apply a membership change entry.
    let membership_config = Membership::new(
        vec![BTreeSet::from([1u64, 2u64, 3u64])],
        None,
    );

    let entry = Entry::<TypeConfig> {
        log_id: LogId::new(leader(1, 1), 1),
        payload: EntryPayload::Membership(membership_config),
    };

    let responses = sm.apply([entry]).await.unwrap();
    assert_eq!(responses.len(), 1);
    assert!(responses[0].success);

    // Verify membership is now persisted.
    let (applied, stored_membership) = sm.applied_state().await.unwrap();
    assert_eq!(applied.unwrap().index, 1);
    assert!(stored_membership.log_id().is_some());

    // The membership should contain our 3 voter node IDs.
    let voter_ids = stored_membership.membership().voter_ids().collect::<Vec<_>>();
    assert_eq!(voter_ids.len(), 3);
    assert!(voter_ids.contains(&1));
    assert!(voter_ids.contains(&2));
    assert!(voter_ids.contains(&3));

    // Membership survives snapshot roundtrip.
    let mut builder = sm.get_snapshot_builder().await;
    let snapshot = builder.build_snapshot().await.unwrap();
    let snap_voter_ids = snapshot
        .meta
        .last_membership
        .membership()
        .voter_ids()
        .collect::<Vec<_>>();
    assert_eq!(snap_voter_ids.len(), 3);
}

// ── Test 5: Log store committed survives reopen ──────────────────────

#[tokio::test]
async fn log_store_committed_survives_reopen() {
    let db = test_db();

    // Write entries and committed index using first LogStore instance.
    {
        let mut store = LogStore::new(Arc::clone(&db));

        write_entries_direct(&db, &[
            blank_entry(1, 1, 0),
            blank_entry(1, 1, 1),
            blank_entry(1, 1, 2),
        ]);

        let committed = LogId::new(leader(1, 1), 2);
        store.save_committed(Some(committed)).await.unwrap();

        let vote = Vote::new(1, 1);
        store.save_vote(&vote).await.unwrap();
    }

    // "Reopen" by creating a new LogStore from the same database.
    let mut store2 = LogStore::new(Arc::clone(&db));

    // Committed index survives.
    let committed = store2.read_committed().await.unwrap().unwrap();
    assert_eq!(committed.index, 2);

    // Vote survives.
    let vote = store2.read_vote().await.unwrap().unwrap();
    assert_eq!(vote, Vote::new(1, 1));

    // Log entries survive.
    let state = store2.get_log_state().await.unwrap();
    assert_eq!(state.last_log_id.unwrap().index, 2);

    let entries = store2.try_get_log_entries(0..=2).await.unwrap();
    assert_eq!(entries.len(), 3);
}

// ── Test 6: Concurrent reader/writer safety ──────────────────────────

#[tokio::test]
async fn concurrent_reader_writer_safety() {
    let db = test_db();
    let mut store = LogStore::new(Arc::clone(&db));

    // Pre-populate some entries.
    write_entries_direct(&db, &[
        blank_entry(1, 1, 0),
        blank_entry(1, 1, 1),
    ]);

    // Obtain a reader before writing more entries.
    let mut reader = store.get_log_reader().await;

    // Reader can see existing entries.
    let read_before = reader.try_get_log_entries(0..=1).await.unwrap();
    assert_eq!(read_before.len(), 2);

    // Write more entries in a separate task.
    let db_clone = Arc::clone(&db);
    let writer_handle = tokio::spawn(async move {
        write_entries_direct(&db_clone, &[
            blank_entry(1, 1, 2),
            blank_entry(1, 1, 3),
            blank_entry(1, 1, 4),
        ]);
    });
    writer_handle.await.unwrap();

    // Reader should now see all entries (redb reads use snapshots per txn).
    let read_after = reader.try_get_log_entries(0..=4).await.unwrap();
    assert_eq!(read_after.len(), 5);

    // Spawn multiple concurrent readers.
    let mut handles = Vec::new();
    for _ in 0..5 {
        let db_c = Arc::clone(&db);
        handles.push(tokio::spawn(async move {
            let mut r = warpgrid_raft::log_store::LogStore::new(Arc::clone(&db_c));
            let entries = r.try_get_log_entries(0..=4).await.unwrap();
            assert_eq!(entries.len(), 5);
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
}
