//! Sanity-check tests for the benchmark harness.
//!
//! Gated behind the `bench` feature because they depend on `bench_utils` mock
//! types. Run with: `cargo test --package warpgrid-host --features bench --test bench_sanity`
//!
//! ## Why these tests matter
//!
//! The sync path (`send()`/`recv()`) holds the pool manager's internal mutex for
//! the entire I/O duration, serialising concurrent access. The async path
//! (`send_query()`/`receive_results()`) releases the mutex before I/O, allowing
//! concurrent query execution. These tests confirm:
//!
//! 1. **Sanity (0ms, concurrency=1):** Both paths produce equivalent throughput
//!    when there is no I/O delay and no concurrency benefit — within acceptable range.
//!
//! 2. **Throughput advantage (50ms, concurrency=50):** The async path achieves
//!    at least 5× the sync path's throughput, validating the concurrency model.

#![cfg(feature = "bench")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use warpgrid_host::db_proxy::bench_utils::{MockAsyncFactory, MockFactory};
use warpgrid_host::db_proxy::{ConnectionPoolManager, PoolConfig, PoolKey};

fn test_key() -> PoolKey {
    PoolKey::new("bench.local", 5432, "benchdb", "benchuser")
}

fn bench_config(max_size: usize) -> PoolConfig {
    PoolConfig {
        max_size,
        idle_timeout: Duration::from_secs(300),
        health_check_interval: Duration::from_secs(300),
        connect_timeout: Duration::from_secs(5),
        recv_timeout: Duration::from_secs(30),
        use_tls: false,
        verify_certificates: false,
        drain_timeout: Duration::from_secs(5),
    }
}

/// Measure sync path throughput: checkout → send → recv → release, repeated.
/// Returns operations per second.
async fn measure_sync_throughput(
    latency: Duration,
    concurrency: usize,
    measurement_duration: Duration,
) -> f64 {
    let factory = Arc::new(MockFactory::with_latency(latency));
    let manager = Arc::new(ConnectionPoolManager::new(
        bench_config(concurrency),
        factory,
    ));
    let key = test_key();

    let mut handles = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        let mgr = manager.clone();
        let k = key.clone();
        handles.push(tokio::spawn(async move {
            let mut ops: u64 = 0;
            let start = Instant::now();
            while start.elapsed() < measurement_duration {
                let h = mgr.checkout(&k, None).await.unwrap();
                mgr.send(h, b"SELECT 1").await.unwrap();
                mgr.recv(h, 1024).await.unwrap();
                mgr.release(h).await.unwrap();
                ops += 1;
            }
            ops
        }));
    }

    let mut total_ops: u64 = 0;
    for h in handles {
        total_ops += h.await.unwrap();
    }
    total_ops as f64 / measurement_duration.as_secs_f64()
}

/// Measure async path throughput: checkout_async → send_query → receive_results → release.
/// Returns operations per second.
async fn measure_async_throughput(
    latency: Duration,
    concurrency: usize,
    measurement_duration: Duration,
) -> f64 {
    let factory = Arc::new(MockFactory::new());
    let async_factory = Arc::new(MockAsyncFactory::with_latency(latency));
    let manager = Arc::new(ConnectionPoolManager::new_with_async(
        bench_config(concurrency),
        factory,
        async_factory,
    ));
    let key = test_key();

    let mut handles = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        let mgr = manager.clone();
        let k = key.clone();
        handles.push(tokio::spawn(async move {
            let mut ops: u64 = 0;
            let start = Instant::now();
            while start.elapsed() < measurement_duration {
                let h = mgr.checkout_async(&k, None).await.unwrap();
                mgr.send_query(h, b"SELECT 1").await.unwrap();
                mgr.receive_results(h, 1024).await.unwrap();
                mgr.release(h).await.unwrap();
                ops += 1;
            }
            ops
        }));
    }

    let mut total_ops: u64 = 0;
    for h in handles {
        total_ops += h.await.unwrap();
    }
    total_ops as f64 / measurement_duration.as_secs_f64()
}

/// At concurrency=1 with 0ms latency, sync and async throughput should be
/// within a reasonable range of each other — the async overhead is negligible
/// when there is no I/O blocking and no concurrency benefit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sanity_zero_latency_single_connection() {
    let duration = Duration::from_millis(500);

    let sync_tps = measure_sync_throughput(Duration::ZERO, 1, duration).await;
    let async_tps = measure_async_throughput(Duration::ZERO, 1, duration).await;

    let ratio = async_tps / sync_tps;
    eprintln!(
        "sanity check — sync: {sync_tps:.0} ops/s, async: {async_tps:.0} ops/s, ratio: {ratio:.2}"
    );

    assert!(
        ratio > 0.5 && ratio < 2.0,
        "async/sync ratio {ratio:.2} outside acceptable range [0.5, 2.0] — \
         sync={sync_tps:.0}, async={async_tps:.0}"
    );
}

/// At 50ms latency and concurrency=50, the async path should achieve at least
/// 5× the sync path's throughput.
///
/// Why: The sync path holds the mutex for the entire 50ms sleep per I/O call,
/// serialising all 50 concurrent tasks. The async path releases the mutex
/// before sleeping, allowing all 50 tasks to overlap their I/O.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn async_throughput_advantage_under_latency() {
    let latency = Duration::from_millis(50);
    let concurrency = 50;
    let duration = Duration::from_secs(2);

    let sync_tps = measure_sync_throughput(latency, concurrency, duration).await;
    let async_tps = measure_async_throughput(latency, concurrency, duration).await;

    let ratio = async_tps / sync_tps;
    eprintln!(
        "throughput test — sync: {sync_tps:.0} ops/s, async: {async_tps:.0} ops/s, \
         ratio: {ratio:.1}x"
    );

    assert!(
        ratio >= 5.0,
        "async/sync ratio {ratio:.1}x is below 5x threshold — \
         sync={sync_tps:.0}, async={async_tps:.0}"
    );
}
