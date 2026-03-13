//! Integration tests for the signal handling shim.
//!
//! These tests compile a real Wasm guest component that calls the
//! `warpgrid:shim/signals@0.1.0` imported functions, then instantiate
//! it inside a real Wasmtime engine with the signal shim wired through
//! `SignalsHost`.
//!
//! The guest component is built from `tests/fixtures/signal-shim-guest/` and
//! exports functions to register interest and poll signals:
//! - `register-terminate`, `register-hangup`, `register-interrupt`
//! - `poll` → returns signal name or `"none"`
//!
//! ## Why these tests matter
//!
//! Unit tests in `signals.rs` and `signals/host.rs` verify the queue
//! implementation in isolation. These integration tests verify the full
//! WIT boundary: guest Wasm code ↔ host functions ↔ SignalsHost, running
//! on a real Wasmtime engine instance.
//!
//! ## Wasmtime component model note
//!
//! After each `call_async` on a component-model typed function, the caller
//! must invoke `post_return_async` before re-entering the component instance.
//! Omitting this causes a "cannot enter component instance" trap.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use wasmtime::component::Component;
use wasmtime::Store;

use warpgrid_host::bindings::warpgrid::shim::signals::SignalType;
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
static SIGNAL_COMPONENT_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

fn build_signal_guest_component() -> &'static [u8] {
    SIGNAL_COMPONENT_BYTES.get_or_init(|| {
        let root = workspace_root();
        let guest_dir = root.join("tests/fixtures/signal-shim-guest");

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
            .expect("failed to run cargo build for signal-shim-guest fixture");
        assert!(
            status.success(),
            "signal-shim-guest fixture build failed with exit code {:?}",
            status.code()
        );

        let core_wasm_path = guest_dir
            .join("target/wasm32-unknown-unknown/release/signal_shim_guest.wasm");

        // Step 2: Convert core module to component with wasm-tools
        let component_path = guest_dir.join("target/signal-shim-guest.component.wasm");
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
            .expect("failed to read compiled signal-shim-guest component")
    })
}

// ── Test host state builder ───────────────────────────────────────

/// Create a minimal HostState with only the signals shim active.
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

// ── Integration tests ─────────────────────────────────────────────

/// AC #1: Guest registers interest in terminate, host delivers, guest polls.
///
/// Verifies the full round-trip through the WIT boundary:
/// guest calls on-signal(terminate) → host registers interest →
/// host delivers terminate → guest calls poll-signal → receives "terminate".
#[tokio::test(flavor = "multi_thread")]
async fn test_register_terminate_deliver_poll() {
    let wasm_bytes = build_signal_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = minimal_host_state();
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    // Guest registers interest in terminate signal
    let register_fn = instance
        .get_typed_func::<(), (Result<(), String>,)>(&mut store, "register-terminate")
        .unwrap();
    let (result,) = register_fn.call_async(&mut store, ()).await.unwrap();
    result.expect("register-terminate should succeed");
    register_fn.post_return_async(&mut store).await.unwrap();

    // Host delivers a terminate signal
    store.data_mut().signals.deliver_signal(SignalType::Terminate);

    // Guest polls — should receive "terminate"
    let poll_fn = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "poll")
        .unwrap();
    let (result,) = poll_fn.call_async(&mut store, ()).await.unwrap();
    let signal_name = result.expect("poll should succeed");
    assert_eq!(signal_name, "terminate", "first poll should return terminate");
    poll_fn.post_return_async(&mut store).await.unwrap();

    // Guest polls again — queue should be drained
    let (result,) = poll_fn.call_async(&mut store, ()).await.unwrap();
    let signal_name = result.expect("poll should succeed");
    assert_eq!(signal_name, "none", "second poll should return none (queue drained)");
    poll_fn.post_return_async(&mut store).await.unwrap();
}

/// AC #2: Polling an empty queue immediately returns "none".
///
/// No interest registered, no signals delivered — poll should return "none"
/// through the WIT boundary.
#[tokio::test(flavor = "multi_thread")]
async fn test_poll_empty_queue_returns_none() {
    let wasm_bytes = build_signal_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = minimal_host_state();
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    // Poll immediately with no registration and no delivery
    let poll_fn = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "poll")
        .unwrap();
    let (result,) = poll_fn.call_async(&mut store, ()).await.unwrap();
    let signal_name = result.expect("poll should succeed");
    assert_eq!(signal_name, "none", "empty queue should return none");
    poll_fn.post_return_async(&mut store).await.unwrap();
}

/// AC #3: Queue is bounded to 16 entries — delivering 20 signals yields only 16.
///
/// Verifies that the default queue capacity (16) is enforced through the
/// WIT boundary. Oldest signals are dropped when the queue is full.
#[tokio::test(flavor = "multi_thread")]
async fn test_20_signals_only_16_retrievable() {
    let wasm_bytes = build_signal_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = minimal_host_state();
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    // Guest registers interest in terminate
    let register_fn = instance
        .get_typed_func::<(), (Result<(), String>,)>(&mut store, "register-terminate")
        .unwrap();
    let (result,) = register_fn.call_async(&mut store, ()).await.unwrap();
    result.expect("register-terminate should succeed");
    register_fn.post_return_async(&mut store).await.unwrap();

    // Host delivers 20 terminate signals (queue capacity is 16)
    for _ in 0..20 {
        store.data_mut().signals.deliver_signal(SignalType::Terminate);
    }

    // Guest polls — should get exactly 16 signals
    let poll_fn = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "poll")
        .unwrap();

    let mut count = 0;
    loop {
        let (result,) = poll_fn.call_async(&mut store, ()).await.unwrap();
        let signal_name = result.expect("poll should succeed");
        poll_fn.post_return_async(&mut store).await.unwrap();
        if signal_name == "none" {
            break;
        }
        assert_eq!(signal_name, "terminate");
        count += 1;
    }

    assert_eq!(count, 16, "should retrieve exactly 16 signals (queue capacity)");
}

/// AC #4: Signal filtering — delivering an unregistered signal type is ignored.
///
/// Guest registers interest in hangup only. Host delivers terminate.
/// Guest polls and gets "none" because terminate was not registered.
#[tokio::test(flavor = "multi_thread")]
async fn test_register_hangup_deliver_terminate_poll_returns_none() {
    let wasm_bytes = build_signal_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = minimal_host_state();
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    // Guest registers interest in hangup only
    let register_fn = instance
        .get_typed_func::<(), (Result<(), String>,)>(&mut store, "register-hangup")
        .unwrap();
    let (result,) = register_fn.call_async(&mut store, ()).await.unwrap();
    result.expect("register-hangup should succeed");
    register_fn.post_return_async(&mut store).await.unwrap();

    // Host delivers a terminate signal (not hangup)
    store.data_mut().signals.deliver_signal(SignalType::Terminate);

    // Guest polls — should return "none" because terminate is not registered
    let poll_fn = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "poll")
        .unwrap();
    let (result,) = poll_fn.call_async(&mut store, ()).await.unwrap();
    let signal_name = result.expect("poll should succeed");
    assert_eq!(
        signal_name, "none",
        "terminate should be filtered out (only hangup registered)"
    );
    poll_fn.post_return_async(&mut store).await.unwrap();
}

/// Multi-type signal ordering: register all three types, deliver one each, poll in FIFO order.
///
/// Verifies that multiple signal types are correctly registered and
/// delivered in FIFO order through the WIT boundary.
#[tokio::test(flavor = "multi_thread")]
async fn test_multiple_signal_types_register_and_deliver() {
    let wasm_bytes = build_signal_guest_component();
    let engine = WarpGridEngine::new().unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = minimal_host_state();
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();

    // Register interest in all three signal types
    for name in ["register-terminate", "register-hangup", "register-interrupt"] {
        let register_fn = instance
            .get_typed_func::<(), (Result<(), String>,)>(&mut store, name)
            .unwrap();
        let (result,) = register_fn.call_async(&mut store, ()).await.unwrap();
        result.unwrap_or_else(|e| panic!("{name} should succeed: {e}"));
        register_fn.post_return_async(&mut store).await.unwrap();
    }

    // Deliver one of each in a specific order
    store.data_mut().signals.deliver_signal(SignalType::Hangup);
    store.data_mut().signals.deliver_signal(SignalType::Terminate);
    store.data_mut().signals.deliver_signal(SignalType::Interrupt);

    // Poll and verify FIFO order
    let poll_fn = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "poll")
        .unwrap();

    let expected = ["hangup", "terminate", "interrupt"];
    for expected_name in &expected {
        let (result,) = poll_fn.call_async(&mut store, ()).await.unwrap();
        let signal_name = result.expect("poll should succeed");
        assert_eq!(
            &signal_name, expected_name,
            "signals should be delivered in FIFO order"
        );
        poll_fn.post_return_async(&mut store).await.unwrap();
    }

    // Queue should be empty
    let (result,) = poll_fn.call_async(&mut store, ()).await.unwrap();
    let signal_name = result.expect("poll should succeed");
    assert_eq!(signal_name, "none", "queue should be empty after draining all signals");
    poll_fn.post_return_async(&mut store).await.unwrap();
}
