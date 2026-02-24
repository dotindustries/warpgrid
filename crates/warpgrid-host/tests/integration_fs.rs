//! Integration tests for the filesystem shim.
//!
//! These tests compile a real Wasm guest component that calls the
//! `warpgrid:shim/filesystem@0.1.0` imported functions, then instantiate
//! it inside a real Wasmtime engine with the filesystem shim enabled.
//!
//! The guest component is built from `tests/fixtures/fs-shim-guest/` and
//! exercises each virtual path: `/etc/resolv.conf`, `/dev/urandom`,
//! `/dev/null`, `/etc/hosts`, and non-virtual fallthrough.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use wasmtime::component::Component;
use wasmtime::Store;

use warpgrid_host::engine::{HostState, WarpGridEngine};
use warpgrid_host::filesystem::host::FilesystemHost;
use warpgrid_host::filesystem::VirtualFileMapBuilder;

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
        let guest_dir = root.join("tests/fixtures/fs-shim-guest");

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

        let core_wasm_path = guest_dir
            .join("target/wasm32-unknown-unknown/release/fs_shim_guest.wasm");

        // Step 2: Convert core module to component with wasm-tools
        let component_path = guest_dir.join("target/fs-shim-guest.component.wasm");
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

        std::fs::read(&component_path)
            .expect("failed to read compiled component")
    })
}

// ── Test host state builder ───────────────────────────────────────

/// Create a HostState with a custom VirtualFileMap for testing.
///
/// The /etc/hosts content is injected with test-specific service registry
/// entries so we can verify that host entries come from the WarpGrid runtime,
/// not the real host system.
fn test_host_state(etc_hosts: &str) -> HostState {
    let file_map = VirtualFileMapBuilder::new()
        .with_dev_null()
        .with_dev_urandom()
        .with_resolv_conf("nameserver 127.0.0.1\nnameserver 10.0.0.53\n")
        .with_etc_hosts(etc_hosts)
        .build();

    HostState {
        filesystem: Some(FilesystemHost::new(Arc::new(file_map))),
        dns: None,
        db_proxy: None,
        signal_queue: Vec::new(),
        threading_model: None,
        limiter: None,
    }
}

// ── Integration tests ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn test_resolv_conf_contains_nameserver() {
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = test_host_state("127.0.0.1 localhost\n");
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    // Call the test-resolv-conf export
    let func = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-resolv-conf")
        .unwrap();
    let (result,) = func.call_async(&mut store, ()).await.unwrap();

    let content = result.expect("test-resolv-conf should succeed");
    assert!(
        content.contains("nameserver"),
        "resolv.conf should contain 'nameserver', got: {content}"
    );
    assert!(
        content.contains("127.0.0.1"),
        "resolv.conf should contain '127.0.0.1', got: {content}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dev_urandom_returns_32_random_bytes() {
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = test_host_state("127.0.0.1 localhost\n");
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<(), (Result<Vec<u8>, String>,)>(&mut store, "test-dev-urandom")
        .unwrap();
    let (result,) = func.call_async(&mut store, ()).await.unwrap();

    let data = result.expect("test-dev-urandom should succeed");
    assert_eq!(data.len(), 32, "should read exactly 32 bytes");
    assert!(
        data.iter().any(|&b| b != 0),
        "32 random bytes should not all be zero"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dev_null_returns_empty() {
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = test_host_state("127.0.0.1 localhost\n");
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<(), (Result<bool, String>,)>(&mut store, "test-dev-null")
        .unwrap();
    let (result,) = func.call_async(&mut store, ()).await.unwrap();

    let is_empty = result.expect("test-dev-null should succeed");
    assert!(is_empty, "/dev/null should return empty content");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_nonvirtual_path_returns_error() {
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = test_host_state("127.0.0.1 localhost\n");
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-nonvirtual")
        .unwrap();
    let (result,) = func.call_async(&mut store, ()).await.unwrap();

    // The guest returns Ok(error_msg) when open_virtual correctly fails
    // for a non-virtual path (fall-through behavior).
    let error_msg = result.expect("test-nonvirtual should return the error message");
    assert!(
        error_msg.contains("not a virtual path"),
        "non-virtual path should get 'not a virtual path' error, got: {error_msg}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_etc_hosts_contains_injected_entries() {
    let wasm_bytes = build_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    // Inject custom /etc/hosts entries simulating a service registry
    let custom_hosts = "\
        127.0.0.1 localhost\n\
        10.0.1.5 db.production.warp.local\n\
        10.0.1.6 cache.staging.warp.local\n\
        10.0.1.7 api.internal.warp.local\n";

    let host_state = test_host_state(custom_hosts);
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    let func = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-etc-hosts")
        .unwrap();
    let (result,) = func.call_async(&mut store, ()).await.unwrap();

    let content = result.expect("test-etc-hosts should succeed");
    assert!(
        content.contains("db.production.warp.local"),
        "hosts should contain injected service entry 'db.production.warp.local', got: {content}"
    );
    assert!(
        content.contains("cache.staging.warp.local"),
        "hosts should contain injected service entry 'cache.staging.warp.local', got: {content}"
    );
    assert!(
        content.contains("10.0.1.5"),
        "hosts should contain injected IP '10.0.1.5', got: {content}"
    );
}
