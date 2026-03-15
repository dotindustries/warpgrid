//! Integration tests for Go DNS+Dial overlay (US-304, issue #42).
//!
//! These tests compile a Go WASI guest fixture (`tests/fixtures/go-dns-dial-guest/`)
//! with TinyGo targeting WASI P1 in reactor mode, instantiate it with Wasmtime,
//! and exercise the DNS resolution chain through the `warpgrid_shim.dns_resolve`
//! host import.
//!
//! The guest uses `dns.DefaultResolver()` (which calls the WASI shim) to resolve
//! hostnames, proving the full ABI contract: Go code → shim_wasi.go → wasmimport →
//! host function → resolved IPs back to guest linear memory.
//!
//! ## Architecture
//!
//! The guest is a WASI P1 reactor module (-buildmode=c-shared) that exports test
//! functions returning packed uint64 (ptr << 32 | len) pointing to result strings
//! in linear memory. The host provides:
//! - Standard WASI P1 imports (via wasmtime_wasi::p1)
//! - `warpgrid_shim.dns_resolve` host function (manual registration)
//!
//! The DNS host function uses a simple HashMap<String, Vec<IpAddr>> as a service
//! registry, matching the production DnsResolver's first-tier resolution.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use wasmtime::{Caller, Config, Engine, Linker, Memory, Module, Store, TypedFunc};
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::WasiCtxBuilder;

// ── Build helpers ─────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

static CORE_MODULE_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

/// Compile the Go DNS dial fixture with TinyGo targeting WASI P1
/// in reactor mode (`-buildmode=c-shared`).
fn build_go_dns_dial_module() -> &'static [u8] {
    CORE_MODULE_BYTES.get_or_init(|| {
        let root = workspace_root();
        let fixture_dir = root.join("tests/fixtures/go-dns-dial-guest");
        let output_path = fixture_dir.join("go-dns-dial-guest.wasm");

        // Locate TinyGo
        let tinygo_root = root.join("build/tinygo");
        let tinygo_bin = tinygo_root.join("bin/tinygo");

        if !tinygo_bin.exists() {
            panic!(
                "TinyGo not found at {}. Run: scripts/build-tinygo.sh --download",
                tinygo_bin.display()
            );
        }

        // Extend PATH with cargo bin directories for wasm-opt and wasm-tools
        let path = std::env::var("PATH").unwrap_or_default();
        let mut extra_paths = Vec::new();
        if let Ok(home) = std::env::var("HOME") {
            extra_paths.push(format!("{home}/.cargo/bin"));
        }
        if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
            extra_paths.push(format!("{cargo_home}/bin"));
        }
        let sprite_bin = "/.sprite/languages/rust/cargo/bin";
        if Path::new(sprite_bin).exists() {
            extra_paths.push(sprite_bin.to_string());
        }
        extra_paths.push(path);
        let extended_path = extra_paths.join(":");

        eprintln!("Building Go DNS dial fixture with TinyGo wasip1 (reactor mode)...");
        let output = Command::new(&tinygo_bin)
            .env("TINYGOROOT", &tinygo_root)
            .env("PATH", &extended_path)
            .args(["build", "-target=wasi", "-buildmode=c-shared", "-o"])
            .arg(output_path.to_str().unwrap())
            .arg(".")
            .current_dir(&fixture_dir)
            .output()
            .expect("failed to run tinygo build");

        assert!(
            output.status.success(),
            "tinygo build failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        eprintln!(
            "Go DNS dial fixture compiled: {} bytes",
            std::fs::metadata(&output_path).unwrap().len()
        );

        std::fs::read(&output_path).expect("failed to read compiled wasm module")
    })
}

// ── DNS service registry ──────────────────────────────────────────

/// Simple hostname → IP address mapping for testing.
#[derive(Clone)]
struct DnsRegistry {
    entries: HashMap<String, Vec<IpAddr>>,
}

impl DnsRegistry {
    fn new() -> Self {
        let mut entries = HashMap::new();
        entries.insert(
            "echo-server.test.warp.local".to_string(),
            vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))],
        );
        entries.insert(
            "multi.test.warp.local".to_string(),
            vec![
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3)),
            ],
        );
        Self { entries }
    }

    fn resolve(&self, hostname: &str) -> Option<&[IpAddr]> {
        self.entries.get(hostname).map(|v| v.as_slice())
    }
}

// ── Host state for WASI P1 + DNS shim ────────────────────────────

struct TestHostState {
    wasi: WasiP1Ctx,
    dns_registry: Arc<DnsRegistry>,
}

// ── DNS shim host function ────────────────────────────────────────

/// Record size: 1 byte family + 16 bytes address = 17 bytes.
const RECORD_SIZE: usize = 17;

/// Register the `warpgrid_shim.dns_resolve` host function on a WASI P1 linker.
///
/// ABI (matching shim_wasi.go):
///   params: (hostname_ptr: i32, hostname_len: i32, family: i32, out_buf_ptr: i32, out_buf_cap: i32)
///   result: i32 (count of records written; 0 = not found)
fn register_dns_shim(linker: &mut Linker<TestHostState>) {
    linker
        .func_wrap(
            "warpgrid_shim",
            "dns_resolve",
            |mut caller: Caller<'_, TestHostState>,
             hostname_ptr: i32,
             hostname_len: i32,
             _family: i32,
             out_buf_ptr: i32,
             out_buf_cap: i32|
             -> i32 {
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .expect("guest must export memory");

                // Read hostname from guest memory
                let hostname = {
                    let data = memory.data(&caller);
                    let start = hostname_ptr as usize;
                    let end = start + hostname_len as usize;
                    if end > data.len() {
                        return 0;
                    }
                    String::from_utf8_lossy(&data[start..end]).to_string()
                };

                // Look up in registry
                let registry = caller.data().dns_registry.clone();
                let addrs = match registry.resolve(&hostname) {
                    Some(addrs) => addrs.to_vec(),
                    None => return 0, // not found
                };

                // Write records to guest memory
                let max_records = (out_buf_cap as usize) / RECORD_SIZE;
                let count = addrs.len().min(max_records);

                let data = memory.data_mut(&mut caller);
                let buf_start = out_buf_ptr as usize;

                for (i, addr) in addrs.iter().take(count).enumerate() {
                    let offset = buf_start + i * RECORD_SIZE;
                    if offset + RECORD_SIZE > data.len() {
                        return i as i32;
                    }

                    match addr {
                        IpAddr::V4(v4) => {
                            data[offset] = 4; // family marker
                            data[offset + 1..offset + 5].copy_from_slice(&v4.octets());
                            data[offset + 5..offset + RECORD_SIZE].fill(0); // pad
                        }
                        IpAddr::V6(v6) => {
                            data[offset] = 6; // family marker
                            data[offset + 1..offset + RECORD_SIZE]
                                .copy_from_slice(&v6.octets());
                        }
                    }
                }

                count as i32
            },
        )
        .expect("failed to register warpgrid_shim.dns_resolve");
}

// ── Test instance ─────────────────────────────────────────────────

/// A running Go DNS dial instance with callable test functions.
struct GoDnsDialInstance {
    store: Store<TestHostState>,
    memory: Memory,
    test_fns: HashMap<String, TypedFunc<(), i64>>,
}

impl GoDnsDialInstance {
    fn new(wasm_bytes: &[u8]) -> Self {
        let engine =
            Engine::new(Config::new().wasm_memory64(false)).expect("failed to create engine");

        let mut linker: Linker<TestHostState> = Linker::new(&engine);
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |state: &mut TestHostState| {
            &mut state.wasi
        })
        .expect("failed to add WASI P1 to linker");

        register_dns_shim(&mut linker);

        let wasi_ctx = WasiCtxBuilder::new().build_p1();
        let host_state = TestHostState {
            wasi: wasi_ctx,
            dns_registry: Arc::new(DnsRegistry::new()),
        };
        let mut store = Store::new(&engine, host_state);

        let module =
            Module::new(&engine, wasm_bytes).expect("failed to compile wasm module");
        let instance = linker
            .instantiate(&mut store, &module)
            .expect("failed to instantiate module");

        // Initialize Go runtime
        let initialize = instance
            .get_typed_func::<(), ()>(&mut store, "_initialize")
            .expect("_initialize export not found");
        initialize
            .call(&mut store, ())
            .expect("_initialize failed");

        let memory = instance
            .get_memory(&mut store, "memory")
            .expect("memory export not found");

        // Collect test function exports
        let test_names = [
            "test-resolve-registry",
            "test-resolve-multiple",
            "test-resolve-nonexistent",
            "test-resolve-ip-literal",
            "test-dialer-dns-error",
        ];

        let mut test_fns = HashMap::new();
        for name in test_names {
            if let Ok(func) = instance.get_typed_func::<(), i64>(&mut store, name) {
                test_fns.insert(name.to_string(), func);
            }
        }

        Self {
            store,
            memory,
            test_fns,
        }
    }

    /// Call an exported test function and return the result string.
    fn call_test(&mut self, name: &str) -> String {
        let func = self
            .test_fns
            .get(name)
            .unwrap_or_else(|| panic!("test function {name} not found"));
        let packed = func
            .call(&mut self.store, ())
            .unwrap_or_else(|e| panic!("calling {name} failed: {e}"));

        // Unpack: high 32 bits = ptr, low 32 bits = len
        let ptr = (packed >> 32) as u32 as usize;
        let len = (packed & 0xFFFF_FFFF) as u32 as usize;

        let mut buf = vec![0u8; len];
        self.memory
            .read(&self.store, ptr, &mut buf)
            .unwrap_or_else(|e| panic!("reading result for {name} failed: {e}"));

        String::from_utf8_lossy(&buf).to_string()
    }
}

// ── Integration tests ─────────────────────────────────────────────

/// AC1: Go guest resolves a service registry hostname via the DNS shim.
#[test]
fn go_dns_dial_resolves_registry_hostname() {
    let wasm_bytes = build_go_dns_dial_module();
    let mut instance = GoDnsDialInstance::new(wasm_bytes);

    let result = instance.call_test("test-resolve-registry");
    assert!(
        result.starts_with("OK:"),
        "expected OK result, got: {result}"
    );

    let ip = &result[3..];
    assert_eq!(
        ip, "127.0.0.1",
        "echo-server.test.warp.local should resolve to 127.0.0.1"
    );
}

/// AC4: Multiple A records are returned in order.
#[test]
fn go_dns_dial_resolves_multiple_addresses() {
    let wasm_bytes = build_go_dns_dial_module();
    let mut instance = GoDnsDialInstance::new(wasm_bytes);

    let result = instance.call_test("test-resolve-multiple");
    assert!(
        result.starts_with("OK:"),
        "expected OK result, got: {result}"
    );

    let addrs: Vec<&str> = result[3..].split(',').collect();
    assert_eq!(
        addrs.len(),
        3,
        "multi.test.warp.local should return 3 addresses, got: {result}"
    );
    assert_eq!(addrs[0], "10.0.0.1");
    assert_eq!(addrs[1], "10.0.0.2");
    assert_eq!(addrs[2], "10.0.0.3");
}

/// Negative case: nonexistent hostname returns DNS error.
#[test]
fn go_dns_dial_nonexistent_returns_error() {
    let wasm_bytes = build_go_dns_dial_module();
    let mut instance = GoDnsDialInstance::new(wasm_bytes);

    let result = instance.call_test("test-resolve-nonexistent");
    assert!(
        result.starts_with("OK:"),
        "expected OK (error was correctly detected by guest), got: {result}"
    );
    // The guest returns OK:<error_message> when the DNS error was properly caught
    assert!(
        result.contains("host not found") || result.contains("HostNotFound"),
        "error should indicate host not found, got: {result}"
    );
}

/// IP literal bypasses DNS resolution entirely.
#[test]
fn go_dns_dial_ip_literal_bypasses_dns() {
    let wasm_bytes = build_go_dns_dial_module();
    let mut instance = GoDnsDialInstance::new(wasm_bytes);

    let result = instance.call_test("test-resolve-ip-literal");
    assert!(
        result.starts_with("OK:"),
        "expected OK result, got: {result}"
    );
    assert_eq!(&result[3..], "192.168.1.1");
}

/// AC3: DNS failure wrapped as *net.OpError containing *DNSError.
#[test]
fn go_dns_dial_error_wrapping() {
    let wasm_bytes = build_go_dns_dial_module();
    let mut instance = GoDnsDialInstance::new(wasm_bytes);

    let result = instance.call_test("test-dialer-dns-error");
    assert!(
        result.starts_with("OK:"),
        "expected OK (error was correctly wrapped), got: {result}"
    );
    assert!(
        result.contains("OpError") && result.contains("DNSError"),
        "result should confirm OpError+DNSError wrapping, got: {result}"
    );
}
