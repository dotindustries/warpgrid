//! Integration tests for WASI 0.3 async handler support (US-501 & US-502).
//!
//! These tests prove that:
//! 1. The Wasmtime engine with `component-model-async` can compile and instantiate
//!    a component that exports `warpgrid:shim/async-handler@0.1.0`
//! 2. The `handle-request` export can be invoked and returns correct responses
//! 3. Multi-chunk request bodies are echoed correctly with `x-async: true` header
//! 4. 10 concurrent requests complete without deadlock within 5 seconds
//!
//! The guest component (`tests/fixtures/async-echo-handler/`) is a minimal echo
//! handler that returns the request body as the response body with status 200
//! and an `x-async: true` header.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock, Once};

use wasmtime::component::Component;
use wasmtime::Store;

use warpgrid_host::bindings::async_handler_bindings::warpgrid::shim::http_types::{
    HttpHeader, HttpRequest,
};
use warpgrid_host::bindings::async_handler_bindings::WarpgridAsyncHandler;
use warpgrid_host::engine::{HostState, WarpGridEngine};
use warpgrid_host::filesystem::host::FilesystemHost;
use warpgrid_host::filesystem::VirtualFileMapBuilder;

// ── Tracing setup ────────────────────────────────────────────────

static TRACING_INIT: Once = Once::new();

/// Initialize tracing subscriber for debug output in CI.
/// Controlled by `RUST_LOG` env var (e.g. `RUST_LOG=debug`).
/// Safe to call multiple times — only the first call takes effect.
fn init_tracing() {
    TRACING_INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env(),
            )
            .with_test_writer()
            .try_init()
            .ok();
    });
}

// ── Build helpers ─────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

static ASYNC_COMPONENT_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

fn build_async_echo_component() -> &'static [u8] {
    ASYNC_COMPONENT_BYTES.get_or_init(|| {
        let root = workspace_root();
        let guest_dir = root.join("tests/fixtures/async-echo-handler");

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
            .expect("failed to run cargo build for async echo handler");
        assert!(
            status.success(),
            "async echo handler build failed with exit code {:?}",
            status.code()
        );

        let core_wasm_path = guest_dir
            .join("target/wasm32-unknown-unknown/release/async_echo_handler.wasm");

        // Step 2: Convert core module to component with wasm-tools
        let component_path = guest_dir.join("target/async-echo-handler.component.wasm");
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

        std::fs::read(&component_path).expect("failed to read compiled async component")
    })
}

// ── Test host state builder ───────────────────────────────────────

fn minimal_host_state() -> HostState {
    let file_map = VirtualFileMapBuilder::new()
        .with_dev_null()
        .with_dev_urandom()
        .with_resolv_conf("nameserver 127.0.0.1\n")
        .with_etc_hosts("127.0.0.1 localhost\n")
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
async fn async_handler_instantiates_and_returns_200() {
    let wasm_bytes = build_async_echo_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let linker = engine.async_handler_linker().unwrap();
    let host_state = minimal_host_state();
    let mut store = Store::new(engine.engine(), host_state);

    let handler = WarpgridAsyncHandler::instantiate_async(
        &mut store,
        &component,
        &linker,
    )
    .await
    .unwrap();

    let request = HttpRequest {
        method: "GET".into(),
        uri: "/health".into(),
        headers: vec![],
        body: vec![],
    };

    let response = handler
        .warpgrid_shim_async_handler()
        .call_handle_request(&mut store, &request)
        .await
        .unwrap();

    assert_eq!(response.status, 200, "echo handler should return 200");
}

#[tokio::test(flavor = "multi_thread")]
async fn async_handler_echoes_request_body() {
    let wasm_bytes = build_async_echo_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let linker = engine.async_handler_linker().unwrap();
    let host_state = minimal_host_state();
    let mut store = Store::new(engine.engine(), host_state);

    let handler = WarpgridAsyncHandler::instantiate_async(
        &mut store,
        &component,
        &linker,
    )
    .await
    .unwrap();

    let body = b"Hello, WarpGrid async!".to_vec();
    let request = HttpRequest {
        method: "POST".into(),
        uri: "/echo".into(),
        headers: vec![HttpHeader {
            name: "content-type".into(),
            value: "text/plain".into(),
        }],
        body: body.clone(),
    };

    let response = handler
        .warpgrid_shim_async_handler()
        .call_handle_request(&mut store, &request)
        .await
        .unwrap();

    assert_eq!(response.status, 200);
    assert_eq!(response.body, body, "response body should echo request body");
}

#[tokio::test(flavor = "multi_thread")]
async fn async_handler_sets_x_async_header() {
    let wasm_bytes = build_async_echo_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let linker = engine.async_handler_linker().unwrap();
    let host_state = minimal_host_state();
    let mut store = Store::new(engine.engine(), host_state);

    let handler = WarpgridAsyncHandler::instantiate_async(
        &mut store,
        &component,
        &linker,
    )
    .await
    .unwrap();

    let request = HttpRequest {
        method: "GET".into(),
        uri: "/".into(),
        headers: vec![],
        body: vec![],
    };

    let response = handler
        .warpgrid_shim_async_handler()
        .call_handle_request(&mut store, &request)
        .await
        .unwrap();

    let has_async_header = response.headers.iter().any(|h| {
        h.name == "x-async" && h.value == "true"
    });
    assert!(
        has_async_header,
        "response should have x-async: true header, got: {:?}",
        response.headers
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn async_handler_multiple_sequential_requests() {
    let wasm_bytes = build_async_echo_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let linker = engine.async_handler_linker().unwrap();
    let host_state = minimal_host_state();
    let mut store = Store::new(engine.engine(), host_state);

    let handler = WarpgridAsyncHandler::instantiate_async(
        &mut store,
        &component,
        &linker,
    )
    .await
    .unwrap();

    // Send 10 sequential requests to verify the component handles
    // multiple invocations without issues.
    for i in 0..10 {
        let body = format!("request-{i}").into_bytes();
        let request = HttpRequest {
            method: "POST".into(),
            uri: format!("/test/{i}"),
            headers: vec![],
            body: body.clone(),
        };

        let response = handler
            .warpgrid_shim_async_handler()
            .call_handle_request(&mut store, &request)
            .await
            .unwrap();

        assert_eq!(response.status, 200, "request {i} should return 200");
        assert_eq!(
            response.body, body,
            "request {i} body should echo correctly"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn async_handler_empty_body_returns_empty_response() {
    let wasm_bytes = build_async_echo_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let linker = engine.async_handler_linker().unwrap();
    let host_state = minimal_host_state();
    let mut store = Store::new(engine.engine(), host_state);

    let handler = WarpgridAsyncHandler::instantiate_async(
        &mut store,
        &component,
        &linker,
    )
    .await
    .unwrap();

    let request = HttpRequest {
        method: "HEAD".into(),
        uri: "/".into(),
        headers: vec![],
        body: vec![],
    };

    let response = handler
        .warpgrid_shim_async_handler()
        .call_handle_request(&mut store, &request)
        .await
        .unwrap();

    assert_eq!(response.status, 200);
    assert!(response.body.is_empty(), "empty request body should produce empty response body");
}

// ── US-502: Multi-chunk and concurrent request tests ─────────────

#[tokio::test(flavor = "multi_thread")]
async fn async_handler_echoes_multi_chunk_body_with_x_async_header() {
    init_tracing();

    let wasm_bytes = build_async_echo_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let linker = engine.async_handler_linker().unwrap();
    let host_state = minimal_host_state();
    let mut store = Store::new(engine.engine(), host_state);

    let handler = WarpgridAsyncHandler::instantiate_async(
        &mut store,
        &component,
        &linker,
    )
    .await
    .unwrap();

    // Compose a multi-chunk body: 10 x 1KB chunks with identifiable patterns.
    // Each chunk contains a repeating byte pattern based on its index so we
    // can verify ordering and completeness on the echo response.
    let mut body = Vec::with_capacity(10 * 1024);
    for chunk_idx in 0u8..10 {
        let chunk: Vec<u8> = (0u8..=255)
            .cycle()
            .skip(chunk_idx as usize)
            .take(1024)
            .collect();
        body.extend_from_slice(&chunk);
    }
    assert_eq!(body.len(), 10 * 1024, "multi-chunk body should be 10KB");

    let request = HttpRequest {
        method: "POST".into(),
        uri: "/echo-large".into(),
        headers: vec![
            HttpHeader {
                name: "content-type".into(),
                value: "application/octet-stream".into(),
            },
            HttpHeader {
                name: "x-chunk-count".into(),
                value: "10".into(),
            },
        ],
        body: body.clone(),
    };

    let response = handler
        .warpgrid_shim_async_handler()
        .call_handle_request(&mut store, &request)
        .await
        .unwrap();

    // Assert status and body echo
    assert_eq!(response.status, 200, "multi-chunk echo should return 200");
    assert_eq!(
        response.body.len(),
        body.len(),
        "echoed body length should match input ({} bytes)",
        body.len()
    );
    assert_eq!(
        response.body, body,
        "echoed body should be byte-identical to multi-chunk input"
    );

    // Assert x-async header is present
    let has_async_header = response
        .headers
        .iter()
        .any(|h| h.name == "x-async" && h.value == "true");
    assert!(
        has_async_header,
        "multi-chunk response should have x-async: true header, got: {:?}",
        response.headers
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn async_handler_10_concurrent_requests_no_deadlock() {
    init_tracing();

    let wasm_bytes = build_async_echo_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Arc::new(
        Component::new(engine.engine(), wasm_bytes).unwrap(),
    );
    let engine = Arc::new(engine);

    // Spawn 10 concurrent tasks, each instantiating its own handler
    // from the shared Component. The component is Send+Sync, while
    // each Store lives on its own task.
    let mut handles = Vec::with_capacity(10);
    for i in 0u32..10 {
        let engine = Arc::clone(&engine);
        let component = Arc::clone(&component);

        handles.push(tokio::spawn(async move {
            let linker = engine.async_handler_linker().unwrap();
            let host_state = minimal_host_state();
            let mut store = Store::new(engine.engine(), host_state);

            let handler = WarpgridAsyncHandler::instantiate_async(
                &mut store,
                &component,
                &linker,
            )
            .await
            .unwrap();

            let body = format!("concurrent-request-{i}").into_bytes();
            let request = HttpRequest {
                method: "POST".into(),
                uri: format!("/concurrent/{i}"),
                headers: vec![],
                body: body.clone(),
            };

            let response = handler
                .warpgrid_shim_async_handler()
                .call_handle_request(&mut store, &request)
                .await
                .unwrap();

            assert_eq!(
                response.status, 200,
                "concurrent request {i} should return 200"
            );
            assert_eq!(
                response.body, body,
                "concurrent request {i} body should echo correctly"
            );

            let has_async_header = response
                .headers
                .iter()
                .any(|h| h.name == "x-async" && h.value == "true");
            assert!(
                has_async_header,
                "concurrent request {i} should have x-async header"
            );

            i
        }));
    }

    // All 10 must complete within 5 seconds or we consider it a deadlock.
    let deadline = std::time::Duration::from_secs(5);
    let mut completed_ids: Vec<u32> = Vec::with_capacity(10);

    for handle in handles {
        let result = tokio::time::timeout(deadline, handle)
            .await
            .expect("concurrent request must complete within 5 seconds (deadlock detected)")
            .expect("task should not panic");
        completed_ids.push(result);
    }

    completed_ids.sort();
    assert_eq!(
        completed_ids,
        (0..10).collect::<Vec<u32>>(),
        "all 10 concurrent requests should complete successfully"
    );
}
