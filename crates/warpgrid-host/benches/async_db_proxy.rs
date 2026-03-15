//! Criterion benchmarks: async vs sync database proxy throughput.
//!
//! Measures queries-per-second for both the async (`send_query`/`receive_results`)
//! and sync (`send`/`recv`) paths in `ConnectionPoolManager`, parameterised across
//! concurrency levels (1, 10, 50, 100) and simulated I/O latencies (0ms, 5ms, 50ms).
//!
//! # Why this matters
//!
//! The sync path holds the pool manager's internal mutex for the entire I/O
//! duration, serialising concurrent access. The async path releases the mutex
//! before I/O, allowing concurrent query execution. At higher latencies and
//! concurrency, async should yield dramatically higher throughput.
//!
//! # Running
//!
//! ```bash
//! cargo bench --package warpgrid-host --features bench --bench async_db_proxy
//! ```

use std::sync::Arc;
use std::time::Duration;

use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, BenchmarkId, Criterion,
};

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

/// Run N concurrent sync operations (checkout → send → recv → release) for a
/// fixed number of iterations and return the total ops completed.
fn run_sync_ops(
    rt: &tokio::runtime::Runtime,
    manager: &Arc<ConnectionPoolManager>,
    key: &PoolKey,
    concurrency: usize,
    ops_per_task: u64,
) {
    rt.block_on(async {
        let mut handles = Vec::with_capacity(concurrency);
        for _ in 0..concurrency {
            let mgr = manager.clone();
            let k = key.clone();
            handles.push(tokio::spawn(async move {
                for _ in 0..ops_per_task {
                    let h = mgr.checkout(&k, None).await.unwrap();
                    mgr.send(h, b"SELECT 1").await.unwrap();
                    mgr.recv(h, 1024).await.unwrap();
                    mgr.release(h).await.unwrap();
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
    });
}

/// Run N concurrent async operations (checkout_async → send_query →
/// receive_results → release) for a fixed number of iterations.
fn run_async_ops(
    rt: &tokio::runtime::Runtime,
    manager: &Arc<ConnectionPoolManager>,
    key: &PoolKey,
    concurrency: usize,
    ops_per_task: u64,
) {
    rt.block_on(async {
        let mut handles = Vec::with_capacity(concurrency);
        for _ in 0..concurrency {
            let mgr = manager.clone();
            let k = key.clone();
            handles.push(tokio::spawn(async move {
                for _ in 0..ops_per_task {
                    let h = mgr.checkout_async(&k, None).await.unwrap();
                    mgr.send_query(h, b"SELECT 1").await.unwrap();
                    mgr.receive_results(h, 1024).await.unwrap();
                    mgr.release(h).await.unwrap();
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
    });
}

/// Add sync and async benchmarks for a given latency level to the group.
fn bench_latency_group(group: &mut BenchmarkGroup<WallTime>, latency: Duration) {
    let concurrency_levels: &[usize] = &[1, 10, 50, 100];

    // Scale ops_per_task inversely with latency to keep benchmark duration
    // reasonable while still producing stable measurements.
    let ops_per_task: u64 = if latency.is_zero() {
        100
    } else if latency <= Duration::from_millis(5) {
        10
    } else {
        2
    };

    for &concurrency in concurrency_levels {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();

        // ── Sync path ──────────────────────────────────────────────
        {
            let factory = Arc::new(MockFactory::with_latency(latency));
            let manager = Arc::new(ConnectionPoolManager::new(
                bench_config(concurrency),
                factory,
            ));
            let key = test_key();

            group.bench_with_input(
                BenchmarkId::new("sync", concurrency),
                &concurrency,
                |b, &conc| {
                    b.iter(|| run_sync_ops(&rt, &manager, &key, conc, ops_per_task));
                },
            );
        }

        // ── Async path ─────────────────────────────────────────────
        {
            let factory = Arc::new(MockFactory::new());
            let async_factory = Arc::new(MockAsyncFactory::with_latency(latency));
            let manager = Arc::new(ConnectionPoolManager::new_with_async(
                bench_config(concurrency),
                factory,
                async_factory,
            ));
            let key = test_key();

            group.bench_with_input(
                BenchmarkId::new("async", concurrency),
                &concurrency,
                |b, &conc| {
                    b.iter(|| run_async_ops(&rt, &manager, &key, conc, ops_per_task));
                },
            );
        }
    }
}

fn bench_0ms(c: &mut Criterion) {
    let mut group = c.benchmark_group("latency_0ms");
    group.sample_size(20);
    bench_latency_group(&mut group, Duration::ZERO);
    group.finish();
}

fn bench_5ms(c: &mut Criterion) {
    let mut group = c.benchmark_group("latency_5ms");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(10));
    bench_latency_group(&mut group, Duration::from_millis(5));
    group.finish();
}

fn bench_50ms(c: &mut Criterion) {
    let mut group = c.benchmark_group("latency_50ms");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(15));
    bench_latency_group(&mut group, Duration::from_millis(50));
    group.finish();
}

criterion_group!(benches, bench_0ms, bench_5ms, bench_50ms);
criterion_main!(benches);
