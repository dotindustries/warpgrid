//! Integration tests for Go HTTP overlay request/response round-trip (US-307).
//!
//! These tests prove acceptance criterion #5: "Test harness sends request to
//! Wasm module and validates response."
//!
//! The test compiles the Go HTTP fixture (`tests/fixtures/go-http-roundtrip-guest/`)
//! with TinyGo targeting WASI Preview 1 in reactor mode (`-buildmode=c-shared`),
//! instantiates it with wasmtime + WASI P1 imports, and calls the
//! `warpgrid-handle-request` export directly using the Go overlay's custom ABI.
//!
//! Reactor mode is essential: TinyGo's `//go:wasmexport` functions require the
//! module to stay alive after initialization. In command mode (`_start`), the Go
//! runtime sets `mainExited=true` after `main()` returns, which causes
//! `wasmExportCheckRun` to panic on any subsequent export call. Reactor mode
//! uses `_initialize` instead, which runs `init()` functions without setting
//! `mainExited`, allowing exports to be called afterward.
//!
//! ## Architecture
//!
//! The Go overlay uses a custom serialization format (not the WIT canonical ABI):
//! - Request: flattened (ptr, len) pairs for method, uri, headers, body + retPtr
//! - Headers: null-separated `name\0value\0` byte buffer
//! - Response: 20-byte struct at retPtr with status, header/body pointers
//!
//! Full canonical ABI alignment (enabling WarpgridAsyncHandler instantiation)
//! is deferred to US-310 (warp pack --lang go).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use wasmtime::{Config, Engine, Linker, Memory, Module, Store, TypedFunc};
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

/// Compile the Go HTTP roundtrip fixture with TinyGo targeting WASI P1
/// in reactor mode (`-buildmode=c-shared`).
///
/// The resulting core module exports `_initialize` (not `_start`) plus
/// `warpgrid-handle-request` with the Go overlay's custom ABI
/// (flattened ptr/len pairs, null-separated headers).
fn build_go_http_roundtrip_module() -> &'static [u8] {
    CORE_MODULE_BYTES.get_or_init(|| {
        let root = workspace_root();
        let fixture_dir = root.join("tests/fixtures/go-http-roundtrip-guest");
        let output_path = fixture_dir.join("go-http-roundtrip.wasm");

        // Locate TinyGo (installed by scripts/build-tinygo.sh)
        let tinygo_root = root.join("build/tinygo");
        let tinygo_bin = tinygo_root.join("bin/tinygo");

        if !tinygo_bin.exists() {
            panic!(
                "TinyGo not found at {}. Run: scripts/build-tinygo.sh --download",
                tinygo_bin.display()
            );
        }

        // wasm-opt and wasm-tools must be on PATH for TinyGo builds.
        // Extend PATH with common cargo bin directories.
        let path = std::env::var("PATH").unwrap_or_default();
        let mut extra_paths = Vec::new();
        if let Ok(home) = std::env::var("HOME") {
            extra_paths.push(format!("{home}/.cargo/bin"));
        }
        if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
            extra_paths.push(format!("{cargo_home}/bin"));
        }
        // Sprite VM cargo bin
        let sprite_bin = "/.sprite/languages/rust/cargo/bin";
        if Path::new(sprite_bin).exists() {
            extra_paths.push(sprite_bin.to_string());
        }
        extra_paths.push(path);
        let extended_path = extra_paths.join(":");

        eprintln!("Building Go HTTP roundtrip fixture with TinyGo wasip1 (reactor mode)...");
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
            "Go HTTP roundtrip fixture compiled: {} bytes",
            std::fs::metadata(&output_path).unwrap().len()
        );

        std::fs::read(&output_path).expect("failed to read compiled wasm module")
    })
}

// ── Guest ABI helpers ─────────────────────────────────────────────

/// A running Go HTTP handler instance ready to handle requests.
struct GoHttpInstance {
    store: Store<WasiP1Ctx>,
    memory: Memory,
    malloc_fn: TypedFunc<i32, i32>,
    #[allow(dead_code)]
    free_fn: TypedFunc<i32, ()>,
    handle_request_fn: TypedFunc<(i32, i32, i32, i32, i32, i32, i32, i32, i32), ()>,
}

/// Parsed response from the Go handler.
#[derive(Debug)]
struct GuestResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl GoHttpInstance {
    /// Create a new instance, initialize the Go runtime, and return the handle.
    fn new(wasm_bytes: &[u8]) -> Self {
        let engine = Engine::new(Config::new().wasm_memory64(false))
            .expect("failed to create engine");

        let mut linker: Linker<WasiP1Ctx> = Linker::new(&engine);
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |ctx| ctx)
            .expect("failed to add WASI P1 to linker");

        let wasi_ctx = WasiCtxBuilder::new().build_p1();
        let mut store = Store::new(&engine, wasi_ctx);

        let module =
            Module::new(&engine, wasm_bytes).expect("failed to compile wasm module");
        let instance = linker
            .instantiate(&mut store, &module)
            .expect("failed to instantiate module");

        // Initialize Go runtime via _initialize (reactor mode).
        // In reactor mode (-buildmode=c-shared), TinyGo exports _initialize
        // instead of _start. This runs init() functions (which register HTTP
        // handlers) without setting mainExited, allowing //go:wasmexport
        // functions to be called afterward.
        let initialize = instance
            .get_typed_func::<(), ()>(&mut store, "_initialize")
            .expect("_initialize export not found (module must be built with -buildmode=c-shared)");
        initialize
            .call(&mut store, ())
            .expect("_initialize failed — Go runtime initialization error");

        let memory = instance
            .get_memory(&mut store, "memory")
            .expect("memory export not found");

        let malloc_fn = instance
            .get_typed_func::<i32, i32>(&mut store, "malloc")
            .expect("malloc export not found");

        let free_fn = instance
            .get_typed_func::<i32, ()>(&mut store, "free")
            .expect("free export not found");

        let handle_request_fn = instance
            .get_typed_func::<(i32, i32, i32, i32, i32, i32, i32, i32, i32), ()>(
                &mut store,
                "warpgrid-handle-request",
            )
            .expect("warpgrid-handle-request export not found");

        Self {
            store,
            memory,
            malloc_fn,
            free_fn,
            handle_request_fn,
        }
    }

    /// Allocate memory in the guest and write data to it.
    fn guest_alloc(&mut self, data: &[u8]) -> i32 {
        if data.is_empty() {
            return 0;
        }
        let ptr = self
            .malloc_fn
            .call(&mut self.store, data.len() as i32)
            .expect("malloc failed");
        self.memory
            .write(&mut self.store, ptr as usize, data)
            .expect("memory write failed");
        ptr
    }

    /// Send an HTTP request and parse the response.
    fn send_request(
        &mut self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> GuestResponse {
        // Write request fields to guest memory
        let method_ptr = self.guest_alloc(method.as_bytes());
        let uri_ptr = self.guest_alloc(uri.as_bytes());

        // Serialize headers as null-separated name\0value\0 pairs
        let mut header_buf = Vec::new();
        for (name, value) in headers {
            header_buf.extend_from_slice(name.as_bytes());
            header_buf.push(0);
            header_buf.extend_from_slice(value.as_bytes());
            header_buf.push(0);
        }
        let headers_ptr = self.guest_alloc(&header_buf);
        let body_ptr = self.guest_alloc(body);

        // Allocate and zero the 20-byte return buffer
        let ret_ptr = self
            .malloc_fn
            .call(&mut self.store, 20)
            .expect("malloc retbuf failed");
        self.memory
            .write(&mut self.store, ret_ptr as usize, &[0u8; 20])
            .expect("zero retbuf failed");

        // Call warpgrid-handle-request
        self.handle_request_fn
            .call(
                &mut self.store,
                (
                    method_ptr,
                    method.len() as i32,
                    uri_ptr,
                    uri.len() as i32,
                    headers_ptr,
                    header_buf.len() as i32,
                    body_ptr,
                    body.len() as i32,
                    ret_ptr,
                ),
            )
            .expect("warpgrid-handle-request call failed");

        // Parse response from retPtr
        self.parse_response(ret_ptr)
    }

    /// Parse the 20-byte response struct from guest memory.
    ///
    /// Layout (little-endian):
    ///   [0:2]   u16 status code
    ///   [2:4]   padding
    ///   [4:8]   ptr to headers data (null-separated name\0value\0)
    ///   [8:12]  headers data length
    ///   [12:16] ptr to body data
    ///   [16:20] body data length
    fn parse_response(&self, ret_ptr: i32) -> GuestResponse {
        let mut ret_buf = [0u8; 20];
        self.memory
            .read(&self.store, ret_ptr as usize, &mut ret_buf)
            .expect("read response struct");

        let status = u16::from_le_bytes([ret_buf[0], ret_buf[1]]);
        let headers_ptr =
            u32::from_le_bytes([ret_buf[4], ret_buf[5], ret_buf[6], ret_buf[7]]) as usize;
        let headers_len =
            u32::from_le_bytes([ret_buf[8], ret_buf[9], ret_buf[10], ret_buf[11]]) as usize;
        let body_ptr =
            u32::from_le_bytes([ret_buf[12], ret_buf[13], ret_buf[14], ret_buf[15]]) as usize;
        let body_len =
            u32::from_le_bytes([ret_buf[16], ret_buf[17], ret_buf[18], ret_buf[19]]) as usize;

        // Parse headers from null-separated buffer
        let headers = if headers_len > 0 {
            let mut header_data = vec![0u8; headers_len];
            self.memory
                .read(&self.store, headers_ptr, &mut header_data)
                .expect("read headers");
            parse_null_separated_headers(&header_data)
        } else {
            Vec::new()
        };

        // Read body
        let body = if body_len > 0 {
            let mut body_data = vec![0u8; body_len];
            self.memory
                .read(&self.store, body_ptr, &mut body_data)
                .expect("read body");
            body_data
        } else {
            Vec::new()
        };

        GuestResponse {
            status,
            headers,
            body,
        }
    }
}

/// Parse null-separated header bytes (name\0value\0name\0value\0...) into pairs.
fn parse_null_separated_headers(data: &[u8]) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    let mut i = 0;

    while i < data.len() {
        // Find name end
        let name_end = data[i..].iter().position(|b| *b == 0).unwrap_or(data.len() - i) + i;
        if name_end >= data.len() {
            break;
        }
        let name = String::from_utf8_lossy(&data[i..name_end]).to_string();

        // Find value end
        let val_start = name_end + 1;
        let val_end =
            data[val_start..].iter().position(|b| *b == 0).unwrap_or(data.len() - val_start)
                + val_start;
        let value = String::from_utf8_lossy(&data[val_start..val_end]).to_string();

        headers.push((name, value));
        i = val_end + 1;
    }

    headers
}

// ── Integration tests ─────────────────────────────────────────────

#[test]
fn go_http_roundtrip_echo_body() {
    let wasm_bytes = build_go_http_roundtrip_module();
    let mut instance = GoHttpInstance::new(wasm_bytes);

    let body = br#"{"message":"hello from test harness"}"#;
    let response = instance.send_request(
        "POST",
        "/echo",
        &[("Content-Type", "application/json")],
        body,
    );

    assert_eq!(response.status, 200, "echo should return 200");
    assert_eq!(
        response.body, body,
        "response body should echo request body"
    );

    // Verify Content-Type is preserved
    let ct = response
        .headers
        .iter()
        .find(|(n, _)| n == "Content-Type")
        .map(|(_, v)| v.as_str());
    assert_eq!(
        ct,
        Some("application/json"),
        "Content-Type header should be preserved in echo response"
    );
}

#[test]
fn go_http_roundtrip_status_codes() {
    let wasm_bytes = build_go_http_roundtrip_module();
    let mut instance = GoHttpInstance::new(wasm_bytes);

    for code in [200, 201, 400, 404, 500] {
        let response = instance.send_request(
            "GET",
            &format!("/status?code={code}"),
            &[],
            &[],
        );

        assert_eq!(
            response.status, code,
            "status handler should return requested code {code}"
        );

        let body_str = String::from_utf8_lossy(&response.body);
        assert!(
            body_str.contains(&format!("status: {code}")),
            "body should contain 'status: {code}', got: {body_str}"
        );
    }
}

#[test]
fn go_http_roundtrip_headers_preserved() {
    let wasm_bytes = build_go_http_roundtrip_module();
    let mut instance = GoHttpInstance::new(wasm_bytes);

    let response = instance.send_request(
        "GET",
        "/headers",
        &[
            ("X-Custom-Header", "test-value"),
            ("Accept", "text/plain"),
        ],
        &[],
    );

    assert_eq!(response.status, 200);

    // The /headers endpoint returns headers as JSON
    let body_str = String::from_utf8_lossy(&response.body);
    assert!(
        body_str.contains("X-Custom-Header"),
        "response should contain X-Custom-Header, got: {body_str}"
    );
    assert!(
        body_str.contains("test-value"),
        "response should contain header value 'test-value', got: {body_str}"
    );
    assert!(
        body_str.contains("Accept"),
        "response should contain Accept header, got: {body_str}"
    );
}

#[test]
fn go_http_roundtrip_method_dispatch() {
    let wasm_bytes = build_go_http_roundtrip_module();
    let mut instance = GoHttpInstance::new(wasm_bytes);

    for method in ["GET", "POST", "PUT", "DELETE"] {
        let response = instance.send_request(method, "/method", &[], &[]);

        assert_eq!(response.status, 200, "{method} should return 200");
        let body_str = String::from_utf8_lossy(&response.body);
        assert_eq!(
            body_str, method,
            "/method should return the HTTP method, got: {body_str}"
        );
    }
}

#[test]
fn go_http_roundtrip_streaming_body() {
    let wasm_bytes = build_go_http_roundtrip_module();
    let mut instance = GoHttpInstance::new(wasm_bytes);

    // Send a 10KB+ body to test streaming via io.Reader
    let body: Vec<u8> = (0u8..=255).cycle().take(10 * 1024).collect();
    let response = instance.send_request("POST", "/stream", &[], &body);

    assert_eq!(response.status, 200, "stream should return 200");
    let body_str = String::from_utf8_lossy(&response.body);
    assert!(
        body_str.contains("bytes_read: 10240"),
        "stream handler should report 10240 bytes read, got: {body_str}"
    );
}

#[test]
fn go_http_roundtrip_empty_body() {
    let wasm_bytes = build_go_http_roundtrip_module();
    let mut instance = GoHttpInstance::new(wasm_bytes);

    let response = instance.send_request("GET", "/echo", &[], &[]);

    assert_eq!(response.status, 200, "GET /echo should return 200");
    assert!(
        response.body.is_empty(),
        "empty request body should produce empty response body"
    );
}

#[test]
fn go_http_roundtrip_404_for_unregistered_path() {
    let wasm_bytes = build_go_http_roundtrip_module();
    let mut instance = GoHttpInstance::new(wasm_bytes);

    let response = instance.send_request("GET", "/nonexistent", &[], &[]);

    assert_eq!(
        response.status, 404,
        "unregistered path should return 404, got: {}",
        response.status
    );
}
