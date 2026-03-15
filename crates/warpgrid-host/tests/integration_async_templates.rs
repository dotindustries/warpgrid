//! Integration tests for US-509: Verify async handler template projects
//! build and produce valid WASI components.
//!
//! These tests validate that each template fixture (async-rust, async-go,
//! async-ts) compiles with its respective toolchain and exports the expected
//! WarpGrid interface.
//!
//! - Rust: `cargo build --target wasm32-unknown-unknown` + `wasm-tools component new`
//! - Go: `tinygo build -target=wasi` (requires TinyGo, skipped if unavailable)
//! - TS: `jco componentize` (requires ComponentizeJS, skipped if unavailable)

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use wasmtime::component::Component;

use warpgrid_host::config::ShimConfig;
use warpgrid_host::engine::WarpGridEngine;

// ── Helpers ──────────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn has_command(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ── Rust template tests ──────────────────────────────────────────

static RUST_TEMPLATE_COMPONENT: OnceLock<Vec<u8>> = OnceLock::new();

fn build_rust_template_component() -> &'static [u8] {
    RUST_TEMPLATE_COMPONENT.get_or_init(|| {
        let root = workspace_root();
        let fixture_dir = root.join("tests/fixtures/async-rust-template");

        // Build the fixture crate to a core Wasm module
        let status = Command::new("cargo")
            .args([
                "build",
                "--manifest-path",
                fixture_dir.join("Cargo.toml").to_str().unwrap(),
                "--target",
                "wasm32-unknown-unknown",
                "--release",
            ])
            .status()
            .expect("failed to run cargo build for async-rust-template");
        assert!(
            status.success(),
            "async-rust-template build failed with exit code {:?}",
            status.code()
        );

        let core_wasm = fixture_dir
            .join("target/wasm32-unknown-unknown/release/async_rust_template.wasm");

        // Convert to component
        let component_path = fixture_dir.join("target/async-rust-template.component.wasm");

        // Find wasm-tools
        let wasm_tools = find_wasm_tools();
        let status = Command::new(&wasm_tools)
            .args([
                "component",
                "new",
                core_wasm.to_str().unwrap(),
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
            .expect("failed to read compiled async-rust-template component")
    })
}

fn find_wasm_tools() -> PathBuf {
    // Check common locations
    for path in &[
        "wasm-tools",
        &format!(
            "{}/.cargo/bin/wasm-tools",
            std::env::var("HOME").unwrap_or_default()
        ),
        "/.sprite/languages/rust/cargo/bin/wasm-tools",
    ] {
        if Command::new(path).arg("--version").output().is_ok() {
            return PathBuf::from(path);
        }
    }
    panic!("wasm-tools not found — install with `cargo install wasm-tools`");
}

/// Verify the Rust template builds and exports `warpgrid:shim/async-handler`.
#[tokio::test(flavor = "multi_thread")]
async fn rust_template_builds_and_exports_async_handler() {
    let wasm_bytes = build_rust_template_component();

    let engine = WarpGridEngine::new(ShimConfig::default()).unwrap();
    let component = Component::new(engine.engine(), wasm_bytes);
    assert!(
        component.is_ok(),
        "async-rust-template component should load into Wasmtime engine"
    );

    // Validate with wasm-tools component wit
    let root = workspace_root();
    let component_path = root.join(
        "tests/fixtures/async-rust-template/target/async-rust-template.component.wasm",
    );

    let wasm_tools = find_wasm_tools();
    let output = Command::new(&wasm_tools)
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
        "component should export async-handler interface, got:\n{wit_output}"
    );
}

/// Load the Rust template component into WarpGridEngine and invoke handle-request.
#[tokio::test(flavor = "multi_thread")]
async fn rust_template_handles_health_request() {
    use warpgrid_host::bindings::async_handler_bindings::warpgrid::shim::http_types::HttpRequest;
    use warpgrid_host::bindings::async_handler_bindings::WarpgridAsyncHandler;
    use warpgrid_host::engine::HostState;
    use warpgrid_host::signals::host::SignalsHost;

    let wasm_bytes = build_rust_template_component();
    let engine = WarpGridEngine::new(ShimConfig::default()).unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let linker = engine.async_handler_linker().unwrap();
    let host_state = HostState {
        filesystem: None,
        dns: None,
        db_proxy: None,
        signals: SignalsHost::new(),
        threading_model: None,
        limiter: None,
    };
    let mut store = wasmtime::Store::new(engine.engine(), host_state);

    let handler =
        WarpgridAsyncHandler::instantiate_async(&mut store, &component, &linker)
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

    assert_eq!(response.status, 200, "health endpoint should return 200");
    let body_str = String::from_utf8(response.body).unwrap();
    assert!(
        body_str.contains("ok"),
        "health response should contain 'ok', got: {body_str}"
    );
}

/// Invoke handle-request with a non-health URI and verify echo response.
#[tokio::test(flavor = "multi_thread")]
async fn rust_template_handles_echo_request() {
    use warpgrid_host::bindings::async_handler_bindings::warpgrid::shim::http_types::HttpRequest;
    use warpgrid_host::bindings::async_handler_bindings::WarpgridAsyncHandler;
    use warpgrid_host::engine::HostState;
    use warpgrid_host::signals::host::SignalsHost;

    let wasm_bytes = build_rust_template_component();
    let engine = WarpGridEngine::new(ShimConfig::default()).unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let linker = engine.async_handler_linker().unwrap();
    let host_state = HostState {
        filesystem: None,
        dns: None,
        db_proxy: None,
        signals: SignalsHost::new(),
        threading_model: None,
        limiter: None,
    };
    let mut store = wasmtime::Store::new(engine.engine(), host_state);

    let handler =
        WarpgridAsyncHandler::instantiate_async(&mut store, &component, &linker)
            .await
            .unwrap();

    let request = HttpRequest {
        method: "POST".into(),
        uri: "/api/test".into(),
        headers: vec![],
        body: b"hello".to_vec(),
    };

    let response = handler
        .warpgrid_shim_async_handler()
        .call_handle_request(&mut store, &request)
        .await
        .unwrap();

    assert_eq!(response.status, 200);
    let body_str = String::from_utf8(response.body).unwrap();
    assert!(
        body_str.contains("POST"),
        "echo response should contain method, got: {body_str}"
    );
    assert!(
        body_str.contains("/api/test"),
        "echo response should contain URI, got: {body_str}"
    );
}

// ── Go template tests (require TinyGo) ──────────────────────────

/// Verify the Go template fixture exists and has the expected structure.
#[test]
fn go_template_fixture_structure() {
    let root = workspace_root();
    let fixture = root.join("tests/fixtures/async-go-template");
    assert!(fixture.join("main.go").exists(), "main.go should exist");
    assert!(fixture.join("go.mod").exists(), "go.mod should exist");
    assert!(fixture.join("warp.toml").exists(), "warp.toml should exist");
    assert!(
        fixture.join("main_test.go").exists(),
        "main_test.go should exist"
    );
}

/// Build async-go with TinyGo and validate it produces a Wasm module.
/// Skipped if TinyGo is not installed.
#[test]
#[ignore = "requires TinyGo toolchain"]
fn go_template_builds_with_tinygo() {
    if !has_command("tinygo") {
        eprintln!("TinyGo not found, skipping");
        return;
    }

    let root = workspace_root();
    let fixture = root.join("tests/fixtures/async-go-template");

    let status = Command::new("tinygo")
        .args([
            "build",
            "-target=wasi",
            "-buildmode=c-shared",
            "-o",
            fixture
                .join("target/async-go-template.wasm")
                .to_str()
                .unwrap(),
            ".",
        ])
        .current_dir(&fixture)
        .status()
        .expect("failed to run tinygo build");

    assert!(
        status.success(),
        "tinygo build failed with exit code {:?}",
        status.code()
    );

    assert!(
        fixture.join("target/async-go-template.wasm").exists(),
        "Wasm output should exist"
    );
}

// ── TS template tests (require ComponentizeJS) ───────────────────

/// Verify the TS template fixture exists and has the expected structure.
#[test]
fn ts_template_fixture_structure() {
    let root = workspace_root();
    let fixture = root.join("tests/fixtures/async-ts-template");
    assert!(
        fixture.join("src/handler.ts").exists(),
        "src/handler.ts should exist"
    );
    assert!(
        fixture.join("package.json").exists(),
        "package.json should exist"
    );
    assert!(
        fixture.join("warp.toml").exists(),
        "warp.toml should exist"
    );
    assert!(
        fixture.join("wit/handler.wit").exists(),
        "wit/handler.wit should exist"
    );
    assert!(
        fixture.join("wit/deps/http/types.wit").exists(),
        "WASI HTTP types WIT should exist"
    );
    assert!(
        fixture.join("wit/deps/shim/dns.wit").exists(),
        "WarpGrid DNS shim WIT should exist"
    );
}

/// Build async-ts with ComponentizeJS and validate component output.
/// Skipped if jco is not installed.
#[test]
#[ignore = "requires ComponentizeJS (jco) toolchain"]
fn ts_template_builds_with_componentize_js() {
    if !has_command("jco") {
        eprintln!("jco not found, skipping");
        return;
    }

    let root = workspace_root();
    let fixture = root.join("tests/fixtures/async-ts-template");

    let output_path = fixture.join("dist/handler.wasm");
    std::fs::create_dir_all(fixture.join("dist")).unwrap();

    let status = Command::new("jco")
        .args([
            "componentize",
            fixture.join("src/handler.ts").to_str().unwrap(),
            "--wit",
            fixture.join("wit").to_str().unwrap(),
            "--world-name",
            "handler",
            "--enable",
            "http",
            "--enable",
            "fetch-event",
            "-o",
            output_path.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run jco componentize");

    assert!(
        status.success(),
        "jco componentize failed with exit code {:?}",
        status.code()
    );
    assert!(output_path.exists(), "Wasm output should exist");
}
