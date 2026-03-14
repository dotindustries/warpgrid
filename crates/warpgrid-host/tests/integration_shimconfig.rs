//! Integration tests for ShimConfig-driven shim toggling in WarpGridEngine.
//!
//! These tests verify that `WarpGridEngine` correctly enables and disables
//! shim interfaces based on `ShimConfig`. When a shim is disabled in the
//! config, its WIT interface is **not** registered with the linker, so
//! instantiating a guest component that imports that interface fails at
//! link time. Conversely, when a shim is enabled, the guest can call its
//! functions successfully.
//!
//! ## Why these tests matter
//!
//! Unit tests in `engine.rs` and `config.rs` verify individual shim
//! registration and config parsing. These integration tests verify the
//! full end-to-end path: ShimConfig → WarpGridEngine → linker registration
//! → guest instantiation → WIT function calls across the Wasm boundary.
//!
//! ## Test matrix
//!
//! | Config                  | Guest fixture      | Expected behavior            |
//! |-------------------------|--------------------|------------------------------|
//! | All shims enabled       | multi-shim-guest   | All 4 exports succeed        |
//! | Only filesystem enabled | fs-shim-guest      | fs calls work                |
//! | Only filesystem enabled | dns-shim-guest     | LinkError at instantiation   |
//! | Only DNS enabled        | dns-shim-guest     | dns calls work               |
//! | Only DNS enabled        | fs-shim-guest      | LinkError at instantiation   |
//! | No shims enabled        | multi-shim-guest   | LinkError at instantiation   |

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use wasmtime::component::Component;
use wasmtime::Store;

use warpgrid_host::config::ShimConfig;
use warpgrid_host::engine::WarpGridEngine;

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

/// Generic build helper: compile a guest fixture to a Wasm component.
///
/// Builds the guest crate to a core Wasm module, then converts it to a
/// component with `wasm-tools component new`. Returns the component bytes.
fn build_guest(fixture_name: &str) -> Vec<u8> {
    let root = workspace_root();
    let guest_dir = root.join(format!("tests/fixtures/{fixture_name}"));

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
        .unwrap_or_else(|e| panic!("failed to run cargo build for {fixture_name}: {e}"));
    assert!(
        status.success(),
        "{fixture_name} build failed with exit code {:?}",
        status.code()
    );

    // Derive the crate name from fixture name (hyphens → underscores)
    let crate_name = fixture_name.replace('-', "_");
    let core_wasm_path = guest_dir
        .join(format!("target/wasm32-unknown-unknown/release/{crate_name}.wasm"));

    // Step 2: Convert core module to component with wasm-tools
    let component_path = guest_dir.join(format!("target/{fixture_name}.component.wasm"));
    let status = Command::new("wasm-tools")
        .args([
            "component",
            "new",
            core_wasm_path.to_str().unwrap(),
            "-o",
            component_path.to_str().unwrap(),
        ])
        .status()
        .unwrap_or_else(|e| panic!("failed to run wasm-tools component new for {fixture_name}: {e}"));
    assert!(
        status.success(),
        "wasm-tools component new failed for {fixture_name} with exit code {:?}",
        status.code()
    );

    std::fs::read(&component_path)
        .unwrap_or_else(|e| panic!("failed to read compiled {fixture_name} component: {e}"))
}

// ── Cached component bytes per fixture ──────────────────────────

static MULTI_SHIM_BYTES: OnceLock<Vec<u8>> = OnceLock::new();
static FS_SHIM_BYTES: OnceLock<Vec<u8>> = OnceLock::new();
static DNS_SHIM_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

fn multi_shim_component() -> &'static [u8] {
    MULTI_SHIM_BYTES.get_or_init(|| build_guest("multi-shim-guest"))
}

fn fs_shim_component() -> &'static [u8] {
    FS_SHIM_BYTES.get_or_init(|| build_guest("fs-shim-guest"))
}

fn dns_shim_component() -> &'static [u8] {
    DNS_SHIM_BYTES.get_or_init(|| build_guest("dns-shim-guest"))
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

// ── Integration tests ───────────────────────────────────────────

/// All shims enabled: multi-shim guest instantiates and all 4 exports succeed.
///
/// Verifies that `ShimConfig::default()` (all shims enabled) registers all
/// four WIT interfaces, allowing a guest that imports all of them to
/// instantiate and call each exported function.
#[tokio::test(flavor = "multi_thread")]
async fn test_all_shims_enabled_multi_shim_guest_succeeds() {
    let wasm_bytes = multi_shim_component();
    let config = ShimConfig::default();
    let engine = WarpGridEngine::new(config).unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = engine.build_host_state(None);
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .expect("multi-shim guest should instantiate with all shims enabled");

    // Test filesystem: read /etc/resolv.conf
    let fs_func = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-fs-read")
        .unwrap();
    let (fs_result,) = fs_func.call_async(&mut store, ()).await.unwrap();
    let content = fs_result.expect("test-fs-read should succeed");
    assert!(
        content.contains("nameserver"),
        "resolv.conf should contain 'nameserver', got: {content}"
    );
    fs_func.post_return_async(&mut store).await.unwrap();

    // Test DNS: resolve localhost
    let dns_func = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-dns-resolve")
        .unwrap();
    let (dns_result,) = dns_func.call_async(&mut store, ()).await.unwrap();
    let addr = dns_result.expect("test-dns-resolve should succeed");
    assert!(
        !addr.is_empty(),
        "localhost should resolve to a non-empty address, got empty"
    );
    dns_func.post_return_async(&mut store).await.unwrap();

    // Test signals: register terminate
    let sig_func = instance
        .get_typed_func::<(), (Result<(), String>,)>(&mut store, "test-signal-register")
        .unwrap();
    let (sig_result,) = sig_func.call_async(&mut store, ()).await.unwrap();
    sig_result.expect("test-signal-register should succeed");
    sig_func.post_return_async(&mut store).await.unwrap();

    // Test threading: declare cooperative
    let thread_func = instance
        .get_typed_func::<(), (Result<(), String>,)>(&mut store, "test-threading-declare")
        .unwrap();
    let (thread_result,) = thread_func.call_async(&mut store, ()).await.unwrap();
    thread_result.expect("test-threading-declare should succeed");
    thread_func.post_return_async(&mut store).await.unwrap();
}

/// Only filesystem enabled: fs-shim-guest instantiates and reads /etc/resolv.conf.
///
/// Verifies that a guest importing only the filesystem interface works when
/// only the filesystem shim is enabled in the config.
#[tokio::test(flavor = "multi_thread")]
async fn test_only_filesystem_enabled_fs_guest_succeeds() {
    let wasm_bytes = fs_shim_component();
    let config = ShimConfig {
        filesystem: true,
        dns: false,
        signals: false,
        database_proxy: false,
        threading: false,
        ..ShimConfig::default()
    };
    let engine = WarpGridEngine::new(config).unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = engine.build_host_state(None);
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .expect("fs-shim-guest should instantiate with only filesystem enabled");

    let func = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-resolv-conf")
        .unwrap();
    let (result,) = func.call_async(&mut store, ()).await.unwrap();
    let content = result.expect("test-resolv-conf should succeed");
    assert!(
        content.contains("nameserver"),
        "resolv.conf should contain 'nameserver', got: {content}"
    );
}

/// Only filesystem enabled: dns-shim-guest fails at link time.
///
/// When DNS is disabled, a guest that imports `warpgrid:shim/dns@0.1.0`
/// cannot be instantiated because the linker has no registration for that
/// interface. This is the expected config-driven isolation behavior.
#[tokio::test(flavor = "multi_thread")]
async fn test_only_filesystem_enabled_dns_guest_fails() {
    let wasm_bytes = dns_shim_component();
    let config = ShimConfig {
        filesystem: true,
        dns: false,
        signals: false,
        database_proxy: false,
        threading: false,
        ..ShimConfig::default()
    };
    let engine = WarpGridEngine::new(config).unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = engine.build_host_state(None);
    let mut store = Store::new(engine.engine(), host_state);

    let result = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await;

    assert!(
        result.is_err(),
        "dns-shim-guest should fail to instantiate when DNS shim is disabled"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("dns") || err_msg.contains("import"),
        "error should mention the missing DNS interface, got: {err_msg}"
    );
}

/// Only DNS enabled: dns-shim-guest instantiates and resolves a hostname.
///
/// Verifies that a guest importing only the DNS interface works when
/// only the DNS shim is enabled in the config.
#[tokio::test(flavor = "multi_thread")]
async fn test_only_dns_enabled_dns_guest_succeeds() {
    let wasm_bytes = dns_shim_component();
    let config = ShimConfig {
        filesystem: false,
        dns: true,
        signals: false,
        database_proxy: false,
        threading: false,
        ..ShimConfig::default()
    };
    let engine = WarpGridEngine::new(config).unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = engine.build_host_state(None);
    let mut store = Store::new(engine.engine(), host_state);

    let instance = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await
        .expect("dns-shim-guest should instantiate with only DNS enabled");

    let func = instance
        .get_typed_func::<(), (Result<String, String>,)>(&mut store, "test-resolve-system-dns")
        .unwrap();
    let (result,) = func.call_async(&mut store, ()).await.unwrap();
    let addr = result.expect("test-resolve-system-dns should succeed");
    assert!(
        !addr.is_empty(),
        "localhost should resolve to a non-empty address, got empty"
    );
}

/// Only DNS enabled: fs-shim-guest fails at link time.
///
/// When filesystem is disabled, a guest that imports
/// `warpgrid:shim/filesystem@0.1.0` cannot be instantiated.
#[tokio::test(flavor = "multi_thread")]
async fn test_only_dns_enabled_fs_guest_fails() {
    let wasm_bytes = fs_shim_component();
    let config = ShimConfig {
        filesystem: false,
        dns: true,
        signals: false,
        database_proxy: false,
        threading: false,
        ..ShimConfig::default()
    };
    let engine = WarpGridEngine::new(config).unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = engine.build_host_state(None);
    let mut store = Store::new(engine.engine(), host_state);

    let result = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await;

    assert!(
        result.is_err(),
        "fs-shim-guest should fail to instantiate when filesystem shim is disabled"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("filesystem") || err_msg.contains("import"),
        "error should mention the missing filesystem interface, got: {err_msg}"
    );
}

/// No shims enabled: multi-shim guest fails at link time.
///
/// When all shims are disabled, a guest that imports any shim interface
/// cannot be instantiated. This verifies that the engine correctly
/// omits all interface registrations.
#[tokio::test(flavor = "multi_thread")]
async fn test_no_shims_enabled_multi_shim_guest_fails() {
    let wasm_bytes = multi_shim_component();
    let config = ShimConfig {
        filesystem: false,
        dns: false,
        signals: false,
        database_proxy: false,
        threading: false,
        ..ShimConfig::default()
    };
    let engine = WarpGridEngine::new(config).unwrap();
    let component = Component::new(engine.engine(), wasm_bytes).unwrap();

    let host_state = engine.build_host_state(None);
    let mut store = Store::new(engine.engine(), host_state);

    let result = engine
        .linker()
        .instantiate_async(&mut store, &component)
        .await;

    assert!(
        result.is_err(),
        "multi-shim-guest should fail to instantiate when all shims are disabled"
    );
}

/// TOML config parsing produces expected ShimConfig values.
///
/// Verifies the integration path from TOML string → `ShimConfig::from_toml()`
/// → `WarpGridEngine::new()` → `engine.config()` round-trips correctly.
/// Complements the unit tests in `config.rs` by exercising the engine's
/// config storage.
#[tokio::test(flavor = "multi_thread")]
async fn test_toml_config_round_trip_through_engine() {
    // Test 1: All shims enabled via booleans
    let toml_str = r#"
        filesystem = true
        dns = true
        signals = true
        database_proxy = true
        threading = true
    "#;
    let value: toml::Value = toml::from_str(toml_str).unwrap();
    let config = ShimConfig::from_toml(Some(&value)).unwrap();
    let engine = WarpGridEngine::new(config).unwrap();

    assert!(engine.config().filesystem);
    assert!(engine.config().dns);
    assert!(engine.config().signals);
    assert!(engine.config().database_proxy);
    assert!(engine.config().threading);

    // Test 2: Selective enable/disable
    let toml_str = r#"
        filesystem = true
        dns = false
        signals = true
        database_proxy = false
        threading = false
    "#;
    let value: toml::Value = toml::from_str(toml_str).unwrap();
    let config = ShimConfig::from_toml(Some(&value)).unwrap();
    let engine = WarpGridEngine::new(config).unwrap();

    assert!(engine.config().filesystem);
    assert!(!engine.config().dns);
    assert!(engine.config().signals);
    assert!(!engine.config().database_proxy);
    assert!(!engine.config().threading);

    // Test 3: Table-form with sub-config
    let toml_str = r#"
        signals = false
        threading = true
        [dns]
        enabled = true
        ttl_seconds = 60
        cache_size = 2048
        [filesystem]
        enabled = true
        timezone_name = "America/New_York"
    "#;
    let value: toml::Value = toml::from_str(toml_str).unwrap();
    let config = ShimConfig::from_toml(Some(&value)).unwrap();
    let engine = WarpGridEngine::new(config).unwrap();

    assert!(engine.config().filesystem);
    assert_eq!(engine.config().filesystem_config.timezone_name, "America/New_York");
    assert!(engine.config().dns);
    assert_eq!(engine.config().dns_config.ttl_seconds, 60);
    assert_eq!(engine.config().dns_config.cache_size, 2048);
    assert!(!engine.config().signals);
    assert!(engine.config().threading);
}

/// Startup logs correctly list enabled/disabled shims.
///
/// Uses tracing-subscriber capture to verify that `WarpGridEngine::new()`
/// emits an INFO log containing each shim's enabled/disabled status.
#[tokio::test(flavor = "multi_thread")]
async fn test_startup_logs_list_shim_status() {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let writer = BufWriter(Arc::clone(&buffer));

    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(writer)
        .with_ansi(false)
        .finish();

    let dispatch = tracing::Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);

    let config = ShimConfig {
        filesystem: true,
        dns: false,
        signals: true,
        database_proxy: false,
        threading: true,
        ..ShimConfig::default()
    };
    let _engine = WarpGridEngine::new(config).unwrap();

    let output = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();

    // The INFO log should contain the engine initialization message
    assert!(
        output.contains("WarpGrid engine initialized"),
        "startup log should contain 'WarpGrid engine initialized', got: {output}"
    );

    // Verify individual shim statuses are logged
    assert!(
        output.contains("filesystem=true"),
        "startup log should contain filesystem=true, got: {output}"
    );
    assert!(
        output.contains("dns=false"),
        "startup log should contain dns=false, got: {output}"
    );
    assert!(
        output.contains("signals=true"),
        "startup log should contain signals=true, got: {output}"
    );
    assert!(
        output.contains("database_proxy=false"),
        "startup log should contain database_proxy=false, got: {output}"
    );
    assert!(
        output.contains("threading=true"),
        "startup log should contain threading=true, got: {output}"
    );
}
