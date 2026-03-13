//! Integration tests for the DNS shim.
//!
//! These tests compile a real Wasm guest component that calls the
//! `warpgrid:shim/dns@0.1.0` imported functions, then instantiate
//! it inside a real Wasmtime engine with the DNS shim enabled.
//!
//! The guest component is built from `tests/fixtures/dns-shim-guest/` and
//! exercises the DNS resolution chain: service registry, virtual `/etc/hosts`,
//! system DNS, round-robin address selection, and TTL caching.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use wasmtime::component::Component;
use wasmtime::Store;

use warpgrid_host::dns::cache::DnsCacheConfig;
use warpgrid_host::dns::host::DnsHost;
use warpgrid_host::dns::{CachedDnsResolver, DnsResolver};
use warpgrid_host::engine::{HostState, WarpGridEngine};

// ── Build helpers ─────────────────────────────────────────────────

/// Workspace root, resolved from CARGO_MANIFEST_DIR.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Build the guest fixture once per test run and return the component bytes.
static COMPONENT_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

fn build_guest_component() -> &'static [u8] {
    COMPONENT_BYTES.get_or_init(|| {
        let root = workspace_root();
        let guest_dir = root.join("tests/fixtures/dns-shim-guest");

        // Step 1: Build the guest crate to a core Wasm module
        let status = Command::new("cargo")
            .args([
                "build",
                "--target",
                "wasm32-unknown-unknown",
                "--release",
            ])
            .current_dir(&guest_dir)
            .status()
            .expect("failed to run cargo build for guest fixture");
        assert!(
            status.success(),
            "guest fixture build failed with exit code {:?}",
            status.code()
        );

        let core_wasm_path =
            guest_dir.join("target/wasm32-unknown-unknown/release/dns_shim_guest.wasm");

        // Step 2: Convert core module to component with wasm-tools
        let component_path = guest_dir.join("target/dns-shim-guest.component.wasm");
        let status = Command::new("wasm-tools")
            .args([
                "component",
                "new",
                core_wasm_path.to_str().unwrap(),
                "-o",
                component_path.to_str().unwrap(),
            ])
            .status()
            .expect("failed to run wasm-tools component new");
        assert!(
            status.success(),
            "wasm-tools component new failed with exit code {:?}",
            status.code()
        );

        std::fs::read(&component_path).expect("failed to read compiled component")
    })
}

// ── Test host state builder ───────────────────────────────────────

/// Create a HostState with the DNS shim enabled and a custom service registry.
///
/// The service registry maps hostnames to IP addresses for testing the
/// first tier of the DNS resolution chain. The etc_hosts content provides
/// the second tier (virtual /etc/hosts).
fn test_host_state(
    service_registry: HashMap<String, Vec<IpAddr>>,
    etc_hosts_content: &str,
) -> HostState {
    let resolver = DnsResolver::new(service_registry, etc_hosts_content);
    let cached = Arc::new(CachedDnsResolver::new(resolver, DnsCacheConfig::default()));
    let runtime_handle = tokio::runtime::Handle::current();

    HostState {
        filesystem: None,
        dns: Some(DnsHost::new(cached, runtime_handle)),
        db_proxy: None,
        signal_queue: Vec::new(),
        threading_model: None,
        limiter: None,
    }
}

/// Standard test service registry with entries for testing.
fn test_service_registry() -> HashMap<String, Vec<IpAddr>> {
    let mut registry = HashMap::new();
    registry.insert(
        "db.test.warp.local".to_string(),
        vec!["10.0.0.5".parse().unwrap()],
    );
    registry.insert(
        "api.test.warp.local".to_string(),
        vec![
            "10.0.0.1".parse().unwrap(),
            "10.0.0.2".parse().unwrap(),
            "10.0.0.3".parse().unwrap(),
        ],
    );
    registry
}

// ── Integration tests ─────────────────────────────────────────────

/// AC1: Wasm component resolves a service name via the injected registry.
#[tokio::test(flavor = "multi_thread")]
async fn test_wasm_resolves_service_registry_name() {
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = test_host_state(
        test_service_registry(),
        "10.0.1.10 cache.test.warp.local\n",
    );
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-resolve-registry")
        .unwrap();
    let (result,) = func.call_async(&mut store, ()).await.unwrap();

    let address = result.expect("should resolve registry entry");
    assert_eq!(
        address, "10.0.0.5",
        "service registry should return the configured IP"
    );
}

/// AC2: Wasm component resolves a hostname from virtual /etc/hosts.
#[tokio::test(flavor = "multi_thread")]
async fn test_wasm_resolves_etc_hosts_hostname() {
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = test_host_state(
        test_service_registry(),
        "10.0.1.10 cache.test.warp.local\n",
    );
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-resolve-etc-hosts")
        .unwrap();
    let (result,) = func.call_async(&mut store, ()).await.unwrap();

    let address = result.expect("should resolve /etc/hosts entry");
    assert_eq!(
        address, "10.0.1.10",
        "/etc/hosts should return the configured IP"
    );
}

/// AC3: Wasm component resolves an external hostname via host system DNS.
///
/// The guest resolves "localhost" which falls through the service registry
/// and /etc/hosts chain to system DNS. We verify a valid IP address is returned,
/// accepting any result since DNS behavior varies across environments.
#[tokio::test(flavor = "multi_thread")]
async fn test_wasm_resolves_external_via_system_dns() {
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    // No registry entry or /etc/hosts for "localhost" — falls through to system DNS
    let host_state = test_host_state(test_service_registry(), "");
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-resolve-system-dns")
        .unwrap();
    let (result,) = func.call_async(&mut store, ()).await.unwrap();

    let address = result.expect("should resolve localhost via system DNS");
    // Verify a valid IP address was returned (the specific address varies by environment)
    let _ip: IpAddr = address
        .parse()
        .unwrap_or_else(|_| panic!("system DNS should return a valid IP address, got: {address}"));
}

/// AC4: Round-robin behavior — multiple addresses are returned from a multi-address hostname.
///
/// The guest calls resolve-address 3 times for "api.test.warp.local" (configured with 3 IPs)
/// and collects all unique addresses. The host test verifies all configured IPs are present.
/// Additionally, tests host-side round-robin via CachedDnsResolver directly.
#[tokio::test(flavor = "multi_thread")]
async fn test_wasm_round_robin_across_calls() {
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let registry = test_service_registry();
    let host_state = test_host_state(registry.clone(), "");
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-resolve-round-robin")
        .unwrap();
    let (result,) = func.call_async(&mut store, ()).await.unwrap();

    let addresses_str = result.expect("should resolve multi-address hostname");
    let addresses: Vec<&str> = addresses_str.split(',').collect();

    assert_eq!(
        addresses.len(),
        3,
        "should return all 3 configured addresses, got: {addresses_str}"
    );
    assert!(
        addresses.contains(&"10.0.0.1"),
        "should contain 10.0.0.1, got: {addresses_str}"
    );
    assert!(
        addresses.contains(&"10.0.0.2"),
        "should contain 10.0.0.2, got: {addresses_str}"
    );
    assert!(
        addresses.contains(&"10.0.0.3"),
        "should contain 10.0.0.3, got: {addresses_str}"
    );

    // Verify host-side round-robin via CachedDnsResolver directly
    let resolver = DnsResolver::new(test_service_registry(), "");
    let cached = CachedDnsResolver::new(resolver, DnsCacheConfig::default());

    let first = cached
        .resolve_round_robin("api.test.warp.local")
        .await
        .unwrap();
    let second = cached
        .resolve_round_robin("api.test.warp.local")
        .await
        .unwrap();
    let third = cached
        .resolve_round_robin("api.test.warp.local")
        .await
        .unwrap();

    // Round-robin should cycle through different addresses
    // (first call returns index 0, second returns index 1, third returns index 2)
    let rr_addrs = [first, second, third];
    assert_eq!(
        rr_addrs.len(),
        3,
        "round-robin should return 3 addresses across 3 calls"
    );
    // Verify they cycle (not all the same)
    assert!(
        !(first == second && second == third),
        "round-robin should not return the same address every time, got: {first}, {second}, {third}"
    );
}

/// AC5: TTL caching — cached value within TTL, re-resolved after expiry.
///
/// Uses a short TTL (50ms) to verify:
/// 1. First call: cache miss (resolution happens)
/// 2. Second call (immediate): cache hit
/// 3. Third call (after TTL expiry): cache miss again
///
/// Each Wasm invocation uses a fresh Store+Instance (component model requires this)
/// but shares the same Arc<CachedDnsResolver> so cache state persists.
#[tokio::test(flavor = "multi_thread")]
async fn test_wasm_ttl_caching_within_and_after_expiry() {
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let cache_config = DnsCacheConfig {
        ttl: Duration::from_millis(50),
        max_entries: 1024,
    };

    // Build a shared CachedDnsResolver that persists across calls
    let resolver = DnsResolver::new(test_service_registry(), "10.0.1.10 cache.test.warp.local\n");
    let cached_resolver = Arc::new(CachedDnsResolver::new(resolver, cache_config));

    // Helper closure: create a fresh store+instance sharing the same resolver
    let call_resolve = |engine: &WarpGridEngine,
                        component: &Component,
                        cached: &Arc<CachedDnsResolver>| {
        let runtime_handle = tokio::runtime::Handle::current();
        let host_state = HostState {
            filesystem: None,
            dns: Some(DnsHost::new(Arc::clone(cached), runtime_handle)),
            db_proxy: None,
            signal_queue: Vec::new(),
            threading_model: None,
            limiter: None,
        };
        let engine = engine.clone();
        let component = component.clone();
        async move {
            let mut store = Store::new(engine.engine(), host_state);
            let instance = engine
                .linker()
                .instantiate_async(&mut store, &component)
                .await
                .unwrap();
            let func = instance
                .get_typed_func::<(), (Result<String, String>,)>(
                    &mut store,
                    "test-resolve-registry",
                )
                .unwrap();
            let (result,) = func.call_async(&mut store, ()).await.unwrap();
            result.expect("resolve should succeed")
        }
    };

    // First call: should resolve (cache miss)
    let address = call_resolve(&engine, &component, &cached_resolver).await;
    assert_eq!(address, "10.0.0.5");

    let (hits, misses, _) = cached_resolver.cache_stats();
    assert_eq!(misses, 1, "first call should be a cache miss");
    assert_eq!(hits, 0, "no cache hits yet");

    // Second call (immediate): should hit cache
    let address = call_resolve(&engine, &component, &cached_resolver).await;
    assert_eq!(address, "10.0.0.5");

    let (hits, misses, _) = cached_resolver.cache_stats();
    assert_eq!(hits, 1, "second call should be a cache hit");
    assert_eq!(misses, 1, "still only 1 miss");

    // Sleep past TTL
    tokio::time::sleep(Duration::from_millis(80)).await;

    // Third call: should re-resolve (cache entry expired)
    let address = call_resolve(&engine, &component, &cached_resolver).await;
    assert_eq!(address, "10.0.0.5");

    let (_, misses, _) = cached_resolver.cache_stats();
    assert_eq!(
        misses, 2,
        "third call should be another cache miss after TTL expiry"
    );
}

/// Negative case: resolving a nonexistent hostname returns an error.
#[tokio::test(flavor = "multi_thread")]
async fn test_wasm_nonexistent_hostname_returns_error() {
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = test_host_state(test_service_registry(), "");
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<(), (Result<String, String>,)>(
            &mut store,
            "test-resolve-nonexistent",
        )
        .unwrap();
    let (result,) = func.call_async(&mut store, ()).await.unwrap();

    // The guest returns Ok(error_msg) when resolve-address correctly fails
    let error_msg = result.expect("should return the error message for nonexistent hostname");
    assert!(
        error_msg.contains("HostNotFound"),
        "error should contain 'HostNotFound', got: {error_msg}"
    );
}
