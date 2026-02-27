//! Integration tests for US-508: Verify Rust async handlers compile against
//! WASI 0.3 WIT with wit-bindgen.
//!
//! These tests prove that:
//! 1. A Rust fixture using `wit-bindgen` generates valid bindings from WarpGrid's WIT
//! 2. `cargo build` + `wasm-tools component new` produces a valid Wasm component
//! 3. `wasm-tools component wit` validates the component exports `warpgrid:shim/async-handler`
//! 4. The component loads into `WarpGridEngine` and handles a full JSON request/response cycle
//!
//! The guest component (`tests/fixtures/rust-async-handler/`) implements a JSON API
//! handler that reads a JSON body, resolves a hostname via the DNS shim, and returns
//! a transformed JSON response.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use wasmtime::component::Component;
use wasmtime::Store;

use warpgrid_host::bindings::async_handler_bindings::warpgrid::shim::http_types::{
    HttpHeader, HttpRequest,
};
use warpgrid_host::bindings::async_handler_bindings::WarpgridAsyncHandler;
use warpgrid_host::dns::cache::DnsCacheConfig;
use warpgrid_host::dns::host::DnsHost;
use warpgrid_host::dns::{CachedDnsResolver, DnsResolver};
use warpgrid_host::engine::{HostState, WarpGridEngine};
use warpgrid_host::filesystem::host::FilesystemHost;
use warpgrid_host::filesystem::VirtualFileMapBuilder;

// ── Build helpers ─────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

static RUST_ASYNC_COMPONENT_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

fn build_rust_async_handler_component() -> &'static [u8] {
    RUST_ASYNC_COMPONENT_BYTES.get_or_init(|| {
        let root = workspace_root();
        let guest_dir = root.join("tests/fixtures/rust-async-handler");

        // Step 1: Build the guest crate to a core Wasm module
        let status = Command::new("cargo")
            .args([
                "build",
                "--manifest-path",
                guest_dir.join("Cargo.toml").to_str().unwrap(),
                "--target",
                "wasm32-unknown-unknown",
                "--release",
            ])
            .status()
            .expect("failed to run cargo build for rust-async-handler");
        assert!(
            status.success(),
            "rust-async-handler build failed with exit code {:?}",
            status.code()
        );

        let core_wasm_path = guest_dir
            .join("target/wasm32-unknown-unknown/release/rust_async_handler.wasm");

        // Step 2: Convert core module to component with wasm-tools
        let component_path = guest_dir.join("target/rust-async-handler.component.wasm");
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

        std::fs::read(&component_path).expect("failed to read compiled rust-async-handler component")
    })
}

// ── Test host state builder ───────────────────────────────────────

fn host_state_with_dns() -> HostState {
    let file_map = VirtualFileMapBuilder::new()
        .with_dev_null()
        .with_dev_urandom()
        .with_resolv_conf("nameserver 127.0.0.1\n")
        .with_etc_hosts("127.0.0.1 localhost\n")
        .build();

    // Configure DNS with a mock service registry
    let mut service_registry: HashMap<String, Vec<IpAddr>> = HashMap::new();
    service_registry.insert(
        "db.test.warp.local".to_string(),
        vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))],
    );
    service_registry.insert(
        "cache.test.warp.local".to_string(),
        vec![
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3)),
        ],
    );

    let resolver = DnsResolver::new(service_registry, "127.0.0.1 localhost\n");
    let cached = Arc::new(CachedDnsResolver::new(
        resolver,
        DnsCacheConfig::default(),
    ));
    let runtime_handle = tokio::runtime::Handle::current();
    let dns = DnsHost::new(cached, runtime_handle);

    HostState {
        filesystem: Some(FilesystemHost::new(Arc::new(file_map))),
        dns: Some(dns),
        db_proxy: None,
        signal_queue: Vec::new(),
        threading_model: None,
        limiter: None,
    }
}

// ── Integration tests ─────────────────────────────────────────────

/// Verify the component builds and `wasm-tools component wit` validates
/// that it exports the `warpgrid:shim/async-handler` interface.
#[tokio::test(flavor = "multi_thread")]
async fn rust_async_handler_builds_and_wit_validates() {
    let wasm_bytes = build_rust_async_handler_component();

    // Write component to a temp file for wasm-tools validation
    let root = workspace_root();
    let component_path = root.join("tests/fixtures/rust-async-handler/target/rust-async-handler.component.wasm");

    // Validate with wasm-tools component wit
    let output = Command::new("wasm-tools")
        .args(["component", "wit", component_path.to_str().unwrap()])
        .output()
        .expect("failed to run wasm-tools component wit");

    assert!(
        output.status.success(),
        "wasm-tools component wit validation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let wit_output = String::from_utf8_lossy(&output.stdout);
    assert!(
        wit_output.contains("async-handler"),
        "component WIT should export async-handler interface, got:\n{wit_output}"
    );

    // Also verify it loads into a Wasmtime engine
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes);
    assert!(
        component.is_ok(),
        "component should load into Wasmtime engine"
    );
}

/// Send a valid JSON request with a resolvable hostname and verify
/// the handler returns a JSON response with resolved addresses.
#[tokio::test(flavor = "multi_thread")]
async fn rust_async_handler_processes_json_request() {
    let wasm_bytes = build_rust_async_handler_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let linker = engine.async_handler_linker().unwrap();
    let host_state = host_state_with_dns();
    let mut store = Store::new(engine.engine(), host_state);

    let handler = WarpgridAsyncHandler::instantiate_async(
        &mut store,
        &component,
        &linker,
    )
    .await
    .unwrap();

    let body = br#"{"hostname":"db.test.warp.local","action":"lookup"}"#.to_vec();
    let request = HttpRequest {
        method: "POST".into(),
        uri: "/resolve".into(),
        headers: vec![HttpHeader {
            name: "content-type".into(),
            value: "application/json".into(),
        }],
        body,
    };

    let response = handler
        .warpgrid_shim_async_handler()
        .call_handle_request(&mut store, &request)
        .await
        .unwrap();

    assert_eq!(response.status, 200, "successful lookup should return 200");

    // Verify content-type header
    let has_json_content_type = response.headers.iter().any(|h| {
        h.name == "content-type" && h.value == "application/json"
    });
    assert!(has_json_content_type, "response should have application/json content-type");

    // Verify x-handler header
    let has_handler_header = response.headers.iter().any(|h| {
        h.name == "x-handler" && h.value == "rust-async"
    });
    assert!(has_handler_header, "response should have x-handler: rust-async header");

    // Parse the JSON response body
    let body_str = String::from_utf8(response.body).unwrap();
    assert!(
        body_str.contains("db.test.warp.local"),
        "response should contain the queried hostname, got: {body_str}"
    );
    assert!(
        body_str.contains("10.0.0.1"),
        "response should contain the resolved address, got: {body_str}"
    );
}

/// Send a JSON request with a hostname that resolves to multiple addresses.
#[tokio::test(flavor = "multi_thread")]
async fn rust_async_handler_resolves_multi_address_hostname() {
    let wasm_bytes = build_rust_async_handler_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let linker = engine.async_handler_linker().unwrap();
    let host_state = host_state_with_dns();
    let mut store = Store::new(engine.engine(), host_state);

    let handler = WarpgridAsyncHandler::instantiate_async(
        &mut store,
        &component,
        &linker,
    )
    .await
    .unwrap();

    let body = br#"{"hostname":"cache.test.warp.local"}"#.to_vec();
    let request = HttpRequest {
        method: "POST".into(),
        uri: "/resolve".into(),
        headers: vec![],
        body,
    };

    let response = handler
        .warpgrid_shim_async_handler()
        .call_handle_request(&mut store, &request)
        .await
        .unwrap();

    assert_eq!(response.status, 200);
    let body_str = String::from_utf8(response.body).unwrap();
    assert!(
        body_str.contains("10.0.0.2"),
        "response should contain first resolved address, got: {body_str}"
    );
    assert!(
        body_str.contains("10.0.0.3"),
        "response should contain second resolved address, got: {body_str}"
    );
    assert!(
        body_str.contains("2 address(es)"),
        "response should report 2 addresses, got: {body_str}"
    );
}

/// Send an invalid JSON body and verify the handler returns 400.
#[tokio::test(flavor = "multi_thread")]
async fn rust_async_handler_returns_400_for_invalid_json() {
    let wasm_bytes = build_rust_async_handler_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let linker = engine.async_handler_linker().unwrap();
    let host_state = host_state_with_dns();
    let mut store = Store::new(engine.engine(), host_state);

    let handler = WarpgridAsyncHandler::instantiate_async(
        &mut store,
        &component,
        &linker,
    )
    .await
    .unwrap();

    let request = HttpRequest {
        method: "POST".into(),
        uri: "/resolve".into(),
        headers: vec![],
        body: b"not valid json{{{".to_vec(),
    };

    let response = handler
        .warpgrid_shim_async_handler()
        .call_handle_request(&mut store, &request)
        .await
        .unwrap();

    assert_eq!(response.status, 400, "invalid JSON should return 400");
    let body_str = String::from_utf8(response.body).unwrap();
    assert!(
        body_str.contains("error"),
        "error response should contain 'error' field, got: {body_str}"
    );
}

/// Send a JSON request with a hostname that cannot be resolved and
/// verify the handler returns 502.
#[tokio::test(flavor = "multi_thread")]
async fn rust_async_handler_returns_502_for_dns_failure() {
    let wasm_bytes = build_rust_async_handler_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let linker = engine.async_handler_linker().unwrap();
    let host_state = host_state_with_dns();
    let mut store = Store::new(engine.engine(), host_state);

    let handler = WarpgridAsyncHandler::instantiate_async(
        &mut store,
        &component,
        &linker,
    )
    .await
    .unwrap();

    // Use a hostname that's not in the service registry and won't resolve
    // via system DNS either (non-existent TLD)
    let body = br#"{"hostname":"nonexistent.invalid.warp.local"}"#.to_vec();
    let request = HttpRequest {
        method: "POST".into(),
        uri: "/resolve".into(),
        headers: vec![],
        body,
    };

    let response = handler
        .warpgrid_shim_async_handler()
        .call_handle_request(&mut store, &request)
        .await
        .unwrap();

    assert_eq!(response.status, 502, "unresolvable hostname should return 502");
    let body_str = String::from_utf8(response.body).unwrap();
    assert!(
        body_str.contains("error"),
        "error response should contain 'error' field, got: {body_str}"
    );
}
