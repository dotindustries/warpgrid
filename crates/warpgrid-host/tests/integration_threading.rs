//! Integration tests for the threading model declaration shim.
//!
//! These tests compile a real Wasm guest component that calls the
//! `warpgrid:shim/threading@0.1.0` imported functions, then instantiate
//! it inside a real Wasmtime engine with the threading shim wired through
//! `HostState`.
//!
//! The guest component is built from `tests/fixtures/threading-shim-guest/`
//! and exports:
//! - `declare-cooperative` → calls `declare-threading-model(cooperative)`
//! - `declare-parallel-required` → calls `declare-threading-model(parallel-required)`
//!
//! ## Why these tests matter
//!
//! Unit tests in `threading/host.rs` and `engine.rs` verify the threading
//! model implementation in isolation. These integration tests verify the full
//! WIT boundary: guest Wasm code ↔ host functions ↔ HostState, running on a
//! real Wasmtime engine instance.
//!
//! ## Wasmtime component model note
//!
//! After each `call_async` on a component-model typed function, the caller
//! must invoke `post_return_async` before re-entering the component instance.
//! Omitting this causes a "cannot enter component instance" trap.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use wasmtime::component::Component;
use wasmtime::Store;

use warpgrid_host::bindings::warpgrid::shim::threading::ThreadingModel;
use warpgrid_host::engine::{HostState, WarpGridEngine};
use warpgrid_host::signals::host::SignalsHost;

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
static THREADING_COMPONENT_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

fn build_threading_guest_component() -> &'static [u8] {
    THREADING_COMPONENT_BYTES.get_or_init(|| {
        let root = workspace_root();
        let guest_dir = root.join("tests/fixtures/threading-shim-guest");

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
            .expect("failed to run cargo build for threading-shim-guest fixture");
        assert!(
            status.success(),
            "threading-shim-guest fixture build failed with exit code {:?}",
            status.code()
        );

        let core_wasm_path = guest_dir
            .join("target/wasm32-unknown-unknown/release/threading_shim_guest.wasm");

        // Step 2: Convert core module to component with wasm-tools
        let component_path = guest_dir.join("target/threading-shim-guest.component.wasm");
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
            .expect("failed to read compiled threading-shim-guest component")
    })
}

// ── Test host state builder ───────────────────────────────────────

/// Create a minimal HostState with only defaults (threading starts as None).
fn minimal_host_state() -> HostState {
    HostState {
        filesystem: None,
        dns: None,
        db_proxy: None,
        signals: SignalsHost::new(),
        threading_model: None,
        limiter: None,
    }
}

// ── Tracing capture helpers ─────────────────────────────────────

/// A writer that appends to a shared buffer for tracing output capture.
#[derive(Clone)]
struct BufWriter(Arc<Mutex<Vec<u8>>>);

impl Write for BufWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufWriter {
    type Writer = BufWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

// ── Integration tests ─────────────────────────────────────────────

/// AC #4: Guest declares cooperative threading model through WIT boundary.
///
/// Verifies that calling `declare-cooperative` from a Wasm guest succeeds
/// and stores `Cooperative` in the host state.
#[tokio::test(flavor = "multi_thread")]
async fn test_cooperative_declares_and_continues() {
    let wasm_bytes = build_threading_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = minimal_host_state();
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    // Guest declares cooperative threading model
    let declare_fn = instance
        .get_typed_func::<(), (Result<(), String>,)>(&mut store, "declare-cooperative")
        .unwrap();
    let (result,) = declare_fn.call_async(&mut store, ()).await.unwrap();
    result.expect("declare-cooperative should succeed");
    declare_fn.post_return_async(&mut store).await.unwrap();

    // Verify threading model is stored in host state
    assert!(
        matches!(
            store.data().threading_model,
            Some(ThreadingModel::Cooperative)
        ),
        "threading model should be Cooperative after declaration"
    );
}

/// AC #5: Guest declares parallel-required, succeeds with warning.
///
/// Verifies that calling `declare-parallel-required` from a Wasm guest
/// succeeds, stores `ParallelRequired` in the host state, and emits
/// a WARN-level tracing event about cooperative fallback.
#[tokio::test(flavor = "multi_thread")]
async fn test_parallel_required_runs_cooperative_with_warning() {
    let wasm_bytes = build_threading_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = minimal_host_state();
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    // Set up tracing capture scoped to this test via a dispatch guard.
    // Using `set_default` (not `with_default`) so the guard can be held
    // across await points — `with_default` is thread-scoped and breaks
    // under multi-threaded async runtimes.
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let writer = BufWriter(Arc::clone(&buffer));

    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(writer)
        .with_ansi(false)
        .finish();

    let dispatch = tracing::Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);

    // Guest declares parallel-required threading model
    let declare_fn = instance
        .get_typed_func::<(), (Result<(), String>,)>(
            &mut store,
            "declare-parallel-required",
        )
        .unwrap();
    let (result,) = declare_fn.call_async(&mut store, ()).await.unwrap();
    result.expect("declare-parallel-required should succeed");
    declare_fn.post_return_async(&mut store).await.unwrap();

    // Verify threading model is stored in host state
    assert!(
        matches!(
            store.data().threading_model,
            Some(ThreadingModel::ParallelRequired)
        ),
        "threading model should be ParallelRequired after declaration"
    );

    // Verify warning was emitted
    let output = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
    assert!(
        output.contains("parallel threading requested but not supported"),
        "expected warning message in tracing output, got: {output}"
    );
}

/// Double declaration through WIT boundary returns error.
///
/// Verifies that calling `declare-cooperative` then `declare-parallel-required`
/// through the Wasm guest returns an error on the second call containing
/// "already declared".
#[tokio::test(flavor = "multi_thread")]
async fn test_double_declaration_returns_error() {
    let wasm_bytes = build_threading_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = minimal_host_state();
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    // First declaration: cooperative — should succeed
    let declare_coop = instance
        .get_typed_func::<(), (Result<(), String>,)>(&mut store, "declare-cooperative")
        .unwrap();
    let (result,) = declare_coop.call_async(&mut store, ()).await.unwrap();
    result.expect("first declaration should succeed");
    declare_coop.post_return_async(&mut store).await.unwrap();

    // Second declaration: parallel-required — should fail
    let declare_par = instance
        .get_typed_func::<(), (Result<(), String>,)>(
            &mut store,
            "declare-parallel-required",
        )
        .unwrap();
    let (result,) = declare_par.call_async(&mut store, ()).await.unwrap();
    let err = result.expect_err("second declaration should fail");
    assert!(
        err.contains("already declared"),
        "error should mention 'already declared', got: {err}"
    );
    declare_par.post_return_async(&mut store).await.unwrap();

    // Original model should be preserved
    assert!(
        matches!(
            store.data().threading_model,
            Some(ThreadingModel::Cooperative)
        ),
        "original cooperative model should be preserved after rejected second declaration"
    );
}
