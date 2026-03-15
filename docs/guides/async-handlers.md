# Async Handler Authoring Guide

Write HTTP request handlers as WebAssembly components that run on WarpGrid with WASI 0.3 concurrent execution. This guide covers Rust, TypeScript, and Go.

## Table of Contents

- [WASI 0.3 Async Model](#wasi-03-async-model)
  - [How It Works](#how-it-works)
  - [The warpgrid-async-handler World](#the-warpgrid-async-handler-world)
- [WIT Interface Definitions](#wit-interface-definitions)
  - [async-handler](#async-handler)
  - [http-types](#http-types)
  - [World Definition](#world-definition)
- [Host Shim Services](#host-shim-services)
- [Writing a Rust Async Handler](#writing-a-rust-async-handler)
  - [Prerequisites](#prerequisites)
  - [Project Scaffold](#project-scaffold)
  - [Configuration](#configuration)
  - [Handler Implementation](#handler-implementation)
  - [Advanced: JSON Parsing and DNS Shim](#advanced-json-parsing-and-dns-shim)
  - [Build and Test](#build-and-test)
- [Writing a TypeScript Async Handler](#writing-a-typescript-async-handler)
  - [Prerequisites](#prerequisites-1)
  - [Project Scaffold](#project-scaffold-1)
  - [Configuration](#configuration-1)
  - [Handler Implementation](#handler-implementation-1)
  - [Build and Test](#build-and-test-1)
- [Writing a Go Async Handler](#writing-a-go-async-handler)
  - [Prerequisites](#prerequisites-2)
  - [Project Scaffold](#project-scaffold-2)
  - [Configuration](#configuration-2)
  - [Handler Implementation](#handler-implementation-2)
  - [Build and Test](#build-and-test-2)
- [Streaming Bodies (Rust)](#streaming-bodies-rust)
  - [Request Body Streams](#request-body-streams)
  - [Streaming Responses](#streaming-responses)
  - [Memory Guarantees](#memory-guarantees)
- [Migrating from Sync Handlers](#migrating-from-sync-handlers)
  - [Rust Migration](#rust-migration)
  - [TypeScript Migration](#typescript-migration)
  - [Go Migration](#go-migration)
  - [Key Behavioral Differences](#key-behavioral-differences)
- [Further Reading](#further-reading)

---

## WASI 0.3 Async Model

WarpGrid runs handler code inside WebAssembly components using the WASI Preview 3 (WASI 0.3) async execution model. Understanding this model is key to writing correct handlers — though the programming model itself is simple.

### How It Works

The async model has one central idea: **your handler code is synchronous, but the host runs many instances of it concurrently.**

When an HTTP request arrives, WarpGrid calls your exported `handle-request` function. From your handler's perspective, this is a normal synchronous function call — you receive a request, do some work, and return a response. There are no async/await keywords, no callbacks, no event loops inside the Wasm component.

The concurrency happens at the host level. Wasmtime's `component-model-async` runtime can invoke `handle-request` multiple times on the same component instance without waiting for previous calls to complete. The runtime interleaves these calls cooperatively, so multiple requests are in flight simultaneously without blocking each other.

This means:

1. **You write synchronous code.** No async runtime needed inside your component.
2. **The host handles concurrency.** Wasmtime's task scheduler manages multiple in-flight requests.
3. **No shared mutable state between requests.** Each invocation gets its own call stack. Do not rely on global mutable state being consistent across concurrent calls.

The Wasmtime engine is configured with three flags that enable this:

```rust
config.async_support(true);              // Tokio-backed async host calls
config.wasm_component_model(true);       // WebAssembly Component Model
config.wasm_component_model_async(true); // WASI 0.3 concurrent invocation
```

### The warpgrid-async-handler World

Every async handler targets the `warpgrid-async-handler` WIT world. This world defines what your component exports (the `handle-request` function) and what it can import (host shim services).

```wit
world warpgrid-async-handler {
    // Host services available to your handler
    import filesystem;
    import dns;
    import signals;
    import database-proxy;
    import threading;

    // Your handler must export this interface
    export async-handler;
}
```

Your component **exports** the `async-handler` interface (a single `handle-request` function) and **imports** whichever host shim services it needs. The host only makes available the shims that are enabled in the deployment configuration — if your code imports a disabled shim, instantiation fails at link time.

## WIT Interface Definitions

All WIT definitions live in `crates/warpgrid-host/wit/` under `package warpgrid:shim@0.1.0`. These are the canonical source — the code examples below are copied from those files.

### async-handler

The core interface your component exports. It has a single function:

```wit
interface async-handler {
    use http-types.{http-request, http-response};

    /// Handle an incoming HTTP request and return a response.
    ///
    /// The host invokes this function for each inbound HTTP request routed
    /// to this component. The function should process the request and return
    /// a complete response.
    handle-request: func(request: http-request) -> http-response;
}
```

The signature is synchronous at the WIT level. Async execution is managed entirely by the Wasmtime runtime — from your handler's perspective, each call is a normal function call.

### http-types

Shared types used by `async-handler` for request and response:

```wit
interface http-types {
    /// An HTTP header as a name-value pair.
    record http-header {
        name: string,
        value: string,
    }

    /// An incoming HTTP request.
    record http-request {
        method: string,
        uri: string,
        headers: list<http-header>,
        body: list<u8>,
    }

    /// An outgoing HTTP response.
    record http-response {
        status: u16,
        headers: list<http-header>,
        body: list<u8>,
    }
}
```

Request and response bodies are `list<u8>` — complete byte buffers, not streams. The full body is materialized before crossing the WIT boundary. For large-payload handling, see [Streaming Bodies (Rust)](#streaming-bodies-rust) which layers a streaming abstraction on top.

### World Definition

The complete world definition in `world.wit`:

```wit
package warpgrid:shim@0.1.0;

/// Async handler world for WASI 0.3 request-driven workloads.
world warpgrid-async-handler {
    import filesystem;
    import dns;
    import signals;
    import database-proxy;
    import threading;

    export async-handler;
}
```

There is also a `warpgrid-shims` world that imports the same five interfaces but exports nothing — it is used for daemon-style workloads, not request handlers.

## Host Shim Services

Your handler can import any of these host-provided services. Each shim is independently toggleable in the deployment configuration.

| Interface | WIT file | Purpose |
|-----------|----------|---------|
| **filesystem** | `filesystem.wit` | Virtual filesystem for well-known paths (`/etc/resolv.conf`, `/dev/urandom`, timezone data). Operations: `open-virtual`, `read-virtual`, `stat-virtual`, `close-virtual`. |
| **dns** | `dns.wit` | Hostname resolution through the WarpGrid service registry, then virtual `/etc/hosts`, then system DNS. Returns `list<ip-address-record>`. |
| **signals** | `signals.wit` | Lifecycle signal handling (SIGTERM, SIGHUP, SIGINT) via a register-and-poll model. Use `on-signal` to subscribe and `poll-signal` to check. |
| **database-proxy** | `database-proxy.wit` | Wire-protocol connection pooling for Postgres, MySQL, and Redis. The guest sends/receives raw protocol bytes; the host manages TLS, pooling, and health checking. |
| **threading** | `threading.wit` | Declare cooperative or parallel-required threading model. Note: true parallel threads are not supported — `parallel-required` falls back to cooperative execution with a warning. |

For full details on each interface, see the [WASI 0.3 API Surface](../wasi-03-api-surface.md) document.

## Writing a Rust Async Handler

### Prerequisites

- Rust toolchain (stable, via `rustup`)
- `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- `wasm-tools` CLI: `cargo install wasm-tools`
- `warp` CLI (WarpGrid packaging tool)

### Project Scaffold

```bash
warp init --template async-rust my-handler
cd my-handler
```

This creates a project with:

```
my-handler/
├── Cargo.toml
├── warp.toml
├── wit/           # WIT definitions (copied from warpgrid-host)
│   ├── world.wit
│   ├── async-handler.wit
│   ├── http-types.wit
│   ├── dns.wit
│   ├── filesystem.wit
│   ├── signals.wit
│   ├── database-proxy.wit
│   └── threading.wit
└── src/
    └── lib.rs
```

### Configuration

**`warp.toml`** — tells the `warp` CLI how to build and package:

```toml
[package]
name = "my-handler"
version = "0.1.0"

[build]
lang = "rust"
```

**`Cargo.toml`** — the crate must produce a `cdylib` for Wasm:

```toml
[package]
name = "my-handler"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = { version = "0.42", default-features = false, features = ["macros"] }
wit-bindgen-rt = { version = "0.42", features = ["bitflags"] }
dlmalloc = { version = "0.2", features = ["global"] }

[profile.release]
opt-level = "s"
lto = true
```

### Handler Implementation

A minimal async handler in Rust. The `#![no_std]` pattern is required because the `wasm32-unknown-unknown` target does not have a standard library — `dlmalloc` provides the global allocator.

```rust
#![no_std]
#![no_main]

extern crate alloc;

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

// Generate bindings from the WIT definitions in ./wit/
wit_bindgen::generate!({
    path: "wit",
    world: "warpgrid-async-handler",
    generate_all,
});

use alloc::format;
use alloc::string::String;
use alloc::vec;
use exports::warpgrid::shim::async_handler::Guest;

struct Component;

impl Guest for Component {
    fn handle_request(
        request: warpgrid::shim::http_types::HttpRequest,
    ) -> warpgrid::shim::http_types::HttpResponse {
        let body = String::from_utf8(request.body).unwrap_or_default();
        let uri = &request.uri;

        match uri.as_str() {
            "/health" => warpgrid::shim::http_types::HttpResponse {
                status: 200,
                headers: vec![warpgrid::shim::http_types::HttpHeader {
                    name: "content-type".into(),
                    value: "application/json".into(),
                }],
                body: b"{\"status\":\"ok\"}".to_vec(),
            },
            _ => {
                let response_body = format!(
                    "{{\"method\":\"{}\",\"uri\":\"{}\",\"body_length\":{}}}",
                    request.method, uri, body.len()
                );
                warpgrid::shim::http_types::HttpResponse {
                    status: 200,
                    headers: vec![warpgrid::shim::http_types::HttpHeader {
                        name: "content-type".into(),
                        value: "application/json".into(),
                    }],
                    body: response_body.into_bytes(),
                }
            }
        }
    }
}

export!(Component);
```

Key patterns:

- **`wit_bindgen::generate!()`** reads the WIT files in `./wit/` and generates Rust types and traits for the `warpgrid-async-handler` world.
- **`exports::warpgrid::shim::async_handler::Guest`** is the trait you implement. It has one method: `handle_request`.
- **`export!(Component)`** registers your struct as the component implementation.
- **`#![no_std]` + `dlmalloc`** is required because `wasm32-unknown-unknown` has no standard library. Use `alloc::` types (`String`, `Vec`, `format!`) instead of `std::`.

### Advanced: JSON Parsing and DNS Shim

For handlers that need JSON parsing and host service access, add `serde` and `serde_json`:

```toml
# Additional Cargo.toml dependencies
serde = { version = "1", default-features = false, features = ["derive", "alloc"] }
serde_json = { version = "1", default-features = false, features = ["alloc"] }
```

This example parses a JSON body, queries the DNS shim, and returns a transformed response:

```rust
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct ServiceQuery {
    hostname: String,
    #[serde(default)]
    action: String,
}

#[derive(Serialize)]
struct ServiceResponse {
    hostname: String,
    addresses: Vec<String>,
    transformed: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

impl exports::warpgrid::shim::async_handler::Guest for Component {
    fn handle_request(
        request: warpgrid::shim::http_types::HttpRequest,
    ) -> warpgrid::shim::http_types::HttpResponse {
        use warpgrid::shim::http_types::{HttpHeader, HttpResponse};

        // Parse JSON request body
        let query: ServiceQuery = match serde_json::from_slice(&request.body) {
            Ok(q) => q,
            Err(e) => {
                return json_error_response(400, format!("invalid JSON: {e}"));
            }
        };

        // Query the DNS shim for hostname resolution
        let addresses = match warpgrid::shim::dns::resolve_address(&query.hostname) {
            Ok(records) => records.iter().map(|r| r.address.clone()).collect::<Vec<_>>(),
            Err(e) => {
                return json_error_response(502, format!("DNS resolution failed: {e}"));
            }
        };

        let transformed = format!(
            "resolved {} to {} address(es)",
            query.hostname, addresses.len()
        );

        let response = ServiceResponse {
            hostname: query.hostname,
            addresses,
            transformed,
        };

        let body = serde_json::to_vec(&response).unwrap_or_default();

        HttpResponse {
            status: 200,
            headers: alloc::vec![
                HttpHeader { name: "content-type".into(), value: "application/json".into() },
                HttpHeader { name: "x-handler".into(), value: "rust-async".into() },
            ],
            body,
        }
    }
}
```

The DNS shim (`warpgrid::shim::dns::resolve_address`) resolves hostnames through WarpGrid's service registry first, then falls back to `/etc/hosts` and system DNS. This is how your handler discovers other services in the cluster.

### Build and Test

```bash
# Build the Wasm component
warp pack

# Or build manually with cargo
cargo build --target wasm32-unknown-unknown --release

# The output .wasm file can be deployed to WarpGrid
```

## Writing a TypeScript Async Handler

### Prerequisites

- Node.js (v18+) or Bun
- `warp` CLI (WarpGrid packaging tool)

### Project Scaffold

```bash
warp init --template async-ts my-handler
cd my-handler
```

This creates:

```
my-handler/
├── warp.toml
└── src/
    └── handler.ts
```

### Configuration

**`warp.toml`**:

```toml
[package]
name = "my-handler"
version = "0.1.0"

[build]
lang = "typescript"
entry = "src/handler.ts"
```

The `entry` field points to your handler source file.

### Handler Implementation

TypeScript handlers use the **service worker `fetch` event** pattern. Register an event listener for `"fetch"` events and respond with a `Response` object — the same Web API you would use in a Cloudflare Worker or Deno Deploy handler.

```typescript
addEventListener("fetch", (event: any) => {
  event.respondWith(handleRequest(event.request));
});

async function handleRequest(request: Request): Promise<Response> {
  const url = new URL(request.url, "http://localhost");

  if (url.pathname === "/health") {
    return new Response(JSON.stringify({ status: "ok" }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }

  // Echo request info as JSON
  const body = {
    method: request.method,
    uri: url.pathname,
  };

  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}
```

Key patterns:

- **`addEventListener("fetch", ...)`** registers your handler. The WarpGrid runtime dispatches incoming HTTP requests as `fetch` events.
- **`event.respondWith()`** accepts a `Promise<Response>` — your handler can be `async`.
- **`Request` and `Response`** are standard Web API types. Parse URLs with `new URL()`, read bodies with `request.json()` or `request.text()`, set headers on the response.
- **`new URL(request.url, "http://localhost")`** — the base URL is needed because the request URL may be a relative path.

### Build and Test

```bash
# Build the Wasm component
warp pack

# The output .wasm file can be deployed to WarpGrid
```

## Writing a Go Async Handler

### Prerequisites

- Go 1.22+
- TinyGo (for Wasm compilation)
- `warp` CLI (WarpGrid packaging tool)

### Project Scaffold

```bash
warp init --template async-go my-handler
cd my-handler
```

This creates:

```
my-handler/
├── warp.toml
├── go.mod
└── main.go
```

### Configuration

**`warp.toml`**:

```toml
[package]
name = "my-handler"
version = "0.1.0"

[build]
lang = "go"
entry = "main.go"
```

**`go.mod`**:

```go
module my-handler

go 1.22.0

require github.com/anthropics/warpgrid/packages/warpgrid-go v0.0.0
```

The `warpgrid-go` package provides HTTP handler utilities that map to the WIT interfaces.

### Handler Implementation

Go handlers use the standard `net/http` handler signature via the `warpgrid-go/http` package. Routes are registered in `init()` — **not** `main()` — because the module runs in **reactor mode** (`-buildmode=c-shared`), where `_initialize` calls `init()` functions but not `main()`.

```go
package main

import (
	"encoding/json"
	"fmt"
	"net/http"

	wghttp "github.com/anthropics/warpgrid/packages/warpgrid-go/http"
)

func init() {
	wghttp.HandleFunc("/health", handleHealth)
	wghttp.HandleFunc("/", handleEcho)
	wghttp.ListenAndServe(":0", nil)
}

func handleHealth(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	fmt.Fprint(w, `{"status":"ok"}`)
}

func handleEcho(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	resp := map[string]interface{}{
		"method": r.Method,
		"uri":    r.URL.String(),
	}
	json.NewEncoder(w).Encode(resp)
}

func main() {
	// main is intentionally empty. Handler registration happens in init()
	// so that the module works correctly in reactor mode (-buildmode=c-shared)
	// where _initialize runs init() functions but not main().
}
```

Key patterns:

- **`wghttp.HandleFunc()`** mirrors `http.HandleFunc()` from the standard library. Your handlers use the familiar `http.ResponseWriter` and `*http.Request` signatures.
- **`wghttp.ListenAndServe(":0", nil)`** binds the handler to the WarpGrid trigger system. The `":0"` address is a convention — the actual port is managed by the host.
- **`init()` vs `main()`**: Register routes in `init()` because reactor mode (`-buildmode=c-shared`) only runs `init()` functions during `_initialize`. `main()` is never called by the host, so it must be empty.
- **Reactor mode**: TinyGo builds with `-buildmode=c-shared` produce a reactor module where exported functions (like `handle-request`) can be called after initialization.

### Build and Test

```bash
# Build with TinyGo for Wasm
tinygo build -target=wasi -buildmode=c-shared -o my-handler.wasm .

# Or use the warp CLI
warp pack

# The output .wasm file can be deployed to WarpGrid
```

## Streaming Bodies (Rust)

For handlers that process large payloads, the `warpgrid-async` crate provides streaming abstractions over the buffered `list<u8>` bodies that cross the WIT boundary. This avoids holding the entire body in memory during processing.

> **Note:** This crate is a Rust-only library. TypeScript and Go handlers work with the full body buffer directly.

### Request Body Streams

`Request::body_stream()` yields the request body in fixed-size chunks (default 64 KB) using zero-copy `Bytes::slice()`:

```rust
use warpgrid_async::{Request, HeaderMap, Header};

// Build a HeaderMap from WIT headers
let mut headers = HeaderMap::new();
for h in wit_request.headers {
    headers.insert(h.name, h.value);
}

// Wrap the WIT request in the streaming-capable Request type
let request = Request::new(
    wit_request.method,
    wit_request.uri,
    headers,
    wit_request.body, // Vec<u8> implements Into<Bytes>
);

// Stream the body in 64 KB chunks (default)
let stream = request.body_stream();

// Or specify a custom chunk size
let stream = request.body_stream_chunked(32 * 1024); // 32 KB chunks
```

Each chunk from the request body stream is a `Result<Bytes, Error>`. The stream is pull-based (`futures_core::Stream`), so chunks are yielded on demand.

### Streaming Responses

Responses can be constructed from a `Stream<Item = Bytes>` for incremental output without pre-buffering. Note: response streams yield `Bytes` directly (infallible), unlike request body streams which yield `Result<Bytes, Error>`.

```rust
use warpgrid_async::{Response, HeaderMap};
use bytes::Bytes;
use futures_util::stream; // add `futures-util = "0.3"` to Cargo.toml

// Buffered response (small payloads)
let response = Response::new(200, headers, body_bytes);

// Streaming response (large or generated payloads)
let chunk_stream = stream::iter(vec![
    Bytes::from("chunk 1"),
    Bytes::from("chunk 2"),
]);
let response = Response::streaming(200, headers, chunk_stream);

// Collect a streaming response back to bytes if needed
let all_bytes = response.into_bytes().await;
```

### Memory Guarantees

The streaming API provides bounded memory overhead regardless of total body size:

- The stream is **pull-based**: at most one chunk is yielded by the producer at any time.
- During a transform (map) operation, both the input chunk and output chunk may exist simultaneously, giving a peak overhead of **2x the chunk size** (default: 2 x 64 KB = 128 KB).
- The `Bytes::slice()` implementation avoids copying — chunks reference the original buffer.

This matters because WIT `list<u8>` bodies are fully materialized at the boundary. The streaming API does not reduce the initial copy from the host, but it does ensure that *processing* the body (parsing, transforming, forwarding) uses bounded memory.

## Migrating from Sync Handlers

If you have an existing WarpGrid handler targeting the `warpgrid-shims` world (import-only, daemon-style), here is how to migrate to the async handler model.

### Rust Migration

**WIT world change** — switch from `warpgrid-shims` to `warpgrid-async-handler`:

```diff
 wit_bindgen::generate!({
     path: "wit",
-    world: "warpgrid-shims",
+    world: "warpgrid-async-handler",
     generate_all,
 });
```

**Add the handler trait implementation:**

```diff
+use exports::warpgrid::shim::async_handler::Guest;
+
+struct Component;
+
+impl Guest for Component {
+    fn handle_request(
+        request: warpgrid::shim::http_types::HttpRequest,
+    ) -> warpgrid::shim::http_types::HttpResponse {
+        // Your request handling logic here
+    }
+}
+
+export!(Component);
```

### TypeScript Migration

If migrating from a custom event loop or module-based handler:

```diff
-export function run() {
-    // polling loop or daemon logic
-}
+addEventListener("fetch", (event: any) => {
+    event.respondWith(handleRequest(event.request));
+});
+
+async function handleRequest(request: Request): Promise<Response> {
+    // Your request handling logic here
+}
```

Update `warp.toml` to point to the new entry file if it changed.

### Go Migration

If migrating from a `main()`-based daemon:

```diff
-func main() {
-    // daemon loop
-    for {
-        // process work
-    }
-}
+func init() {
+    wghttp.HandleFunc("/", handler)
+    wghttp.ListenAndServe(":0", nil)
+}
+
+func handler(w http.ResponseWriter, r *http.Request) {
+    // Your request handling logic here
+}
+
+func main() {
+    // Empty — init() registers handlers for reactor mode
+}
```

### Key Behavioral Differences

| Aspect | Sync (daemon) | Async (handler) |
|--------|---------------|-----------------|
| **Invocation** | Long-running process | Per-request function call |
| **Concurrency** | Single-threaded event loop | Host-managed concurrent invocations |
| **Global state** | Mutable globals persist | Do not assume globals are consistent across concurrent calls |
| **Lifecycle** | Started once, runs continuously | Instantiated per deployment, invoked per request |
| **WIT world** | `warpgrid-shims` (import-only) | `warpgrid-async-handler` (imports + export) |

**Important:** The async handler model means your `handle-request` function may be called concurrently by the host. Do not rely on global mutable state being consistent between calls — each invocation should be self-contained.

## Further Reading

- [WASI 0.3 API Surface](../wasi-03-api-surface.md) — detailed coverage of Wasmtime's async features, build configuration, and known limitations
- [WIT Definitions](../../crates/warpgrid-host/wit/) — canonical WIT source files
- [`warpgrid-async` crate](../../crates/warpgrid-async/) — streaming body support for Rust handlers
