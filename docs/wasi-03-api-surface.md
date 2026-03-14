# WASI 0.3 API Surface in WarpGrid

This document describes the WASI Preview 3 (WASI 0.3) async interfaces available to WarpGrid through its source-built Wasmtime dependency, and how those interfaces are used by `warpgrid-host` and `warpgrid-trigger`.

## Overview

WASI Preview 3 is the in-progress successor to WASI Preview 2. Its primary addition is the **component-model-async** extension, which introduces native async execution into the WebAssembly Component Model. The key features are:

- **Concurrent component invocation**: a host can call multiple exports on the same or different component instances concurrently, without those instances blocking each other.
- **Async-lifted/lowered functions**: WIT functions can be designated as async at the host–guest boundary. The Wasmtime runtime manages task scheduling; guests do not need OS threads.
- **Futures and streams in WIT**: the component model gains first-class `future<T>` and `stream<T>` types for passing async values across the host–guest boundary.
- **Error contexts**: structured error propagation for async failures.

As of the pinned commit (see below), WASI 0.3 is available in Wasmtime v41+ (the `wasip3-prototyping` branch was merged into `main`). WarpGrid pins a specific commit SHA from the `release-41.0.0` branch and builds Wasmtime from source to access these features.

## The Pinned SHA

The commit SHA is stored in `scripts/WASMTIME_ASYNC_SHA`:

```
d938a9df47c8e62014c1a12571547411ede6ff5e
```

This commit is from the `release-41.0.0` branch of `bytecodealliance/wasmtime` (Wasmtime 41.0.4, 2026-02-24). It requires rustc >= 1.90.0.

CI cache keys are derived from this file — changing the SHA triggers a full rebuild of the vendored Wasmtime.

To update the SHA, replace the commit hash in `scripts/WASMTIME_ASYNC_SHA` and run the build script (see Build Instructions below).

## Async Interfaces Available from the Source Build

The following interfaces are unlocked by enabling the `component-model-async` feature in the source build.

### Wasmtime Cargo Features

The workspace `Cargo.toml` requests three features from the source-built crates:

```toml
wasmtime = { version = "41", features = ["component-model", "async", "component-model-async"] }
wasmtime-wasi = { version = "41", features = ["p3"] }
```

| Feature | Effect |
|---|---|
| `component-model` | Enables WebAssembly Component Model support (required baseline) |
| `async` | Enables Tokio-backed async host calls and `Store::call_async` |
| `component-model-async` | Unlocks WASI 0.3 concurrent invocation, async lift/lower, futures and streams in WIT |
| `wasmtime-wasi` `p3` | Enables WASI Preview 3 standard interfaces (io, clocks, filesystem using async primitives) |

### Runtime Configuration

`WarpGridEngine::new()` in `crates/warpgrid-host/src/engine.rs` enables all three flags on the Wasmtime `Config`:

```rust
wasm_config.async_support(true);
wasm_config.wasm_component_model(true);
wasm_config.wasm_component_model_async(true);
```

This makes every `Store` created by the engine capable of running concurrent async component invocations.

### Key Async Primitives (component-model-async)

These types and mechanisms become available in WIT definitions once the feature is enabled:

**Futures** (`future<T>`): A single-value async return type. A WIT function can return a `future<T>` that the host resolves asynchronously. Not yet used directly in WarpGrid WIT files, but available for future interfaces.

**Streams** (`stream<T>`): A sequence of values passed across the component boundary asynchronously. Enables incremental processing of large payloads. The `warpgrid-async` crate provides a Rust-level streaming abstraction (see Mapping section below) that mirrors this model at the WIT boundary.

**Error contexts**: Structured async error propagation, accessible when a future or stream resolves to an error.

**Concurrent export invocation**: The host can call the same exported function (`handle-request`) on a single component instance multiple times concurrently. The Wasmtime scheduler interleaves the tasks cooperatively. This is the primary mechanism WarpGrid uses for high-throughput request handling.

### bindgen! `exports: { default: async }`

The async handler bindings in `crates/warpgrid-host/src/bindings.rs` use:

```rust
wasmtime::component::bindgen!({
    path: "wit",
    world: "warpgrid-async-handler",
    with: { /* shared import types */ },
    exports: { default: async },
});
```

The `exports: { default: async }` directive tells the `bindgen!` macro to generate async Rust call sites for all exported functions in the world. Without the `component-model-async` feature, this directive is not available and compilation fails.

## WarpGrid's WIT Interfaces

All WarpGrid WIT files live in `crates/warpgrid-host/wit/` under `package warpgrid:shim@0.1.0`.

### Worlds

**`warpgrid-shims`** (in `world.wit`): The base shim world. Guest components that import WarpGrid host services use this world. It imports all five shim interfaces but exports nothing — suitable for daemon-style workloads.

**`warpgrid-async-handler`** (in `world.wit`): Extends the base shim world with an exported `async-handler` interface. This is the world used for HTTP request-driven workloads. The host calls the exported `handle-request` function for each inbound request.

```wit
world warpgrid-async-handler {
    import filesystem;
    import dns;
    import signals;
    import database-proxy;
    import threading;

    export async-handler;
}
```

### Shim Interfaces

| Interface | WIT file | Purpose |
|---|---|---|
| `filesystem` | `filesystem.wit` | Virtual filesystem: intercepts open/read/stat for well-known paths (`/etc/resolv.conf`, `/dev/urandom`, timezone data, extra custom paths) |
| `dns` | `dns.wit` | DNS resolution via service registry → `/etc/hosts` → system DNS chain |
| `signals` | `signals.wit` | Lifecycle signals (SIGTERM, SIGHUP, SIGINT) via register-and-poll model |
| `database-proxy` | `database-proxy.wit` | Wire-protocol connection pooling for Postgres, MySQL, and Redis |
| `threading` | `threading.wit` | Guest declares cooperative or parallel-required threading model |
| `http-types` | `http-types.wit` | Shared HTTP request/response types (types only, no functions) |
| `async-handler` | `async-handler.wit` | Exported `handle-request` function for HTTP trigger invocation |

### async-handler Interface

```wit
interface async-handler {
    use http-types.{http-request, http-response};

    handle-request: func(request: http-request) -> http-response;
}
```

The function signature is synchronous at the WIT level. Async behavior is entirely managed by the Wasmtime component-model-async runtime: the host can invoke this function many times concurrently across multiple requests. The guest does not observe the concurrency — from its perspective each call is a normal synchronous call, but the host interleaves multiple in-flight calls without blocking.

## Mapping to WarpGrid Crates

### `warpgrid-host`

The central crate for WASI 0.3 integration.

**`src/bindings.rs`**: Uses `wasmtime::component::bindgen!` twice:
1. For the `warpgrid-shims` world (base import-only bindings).
2. For the `warpgrid-async-handler` world inside the `async_handler_bindings` module, with `exports: { default: async }` to generate async call sites for `handle-request`.

The `with` parameter in the second bindgen call reuses the same type definitions from the first, so `HostState` only needs to implement each `Host` trait once.

**`src/engine.rs`**: The `WarpGridEngine` struct is the top-level orchestrator. Key points:
- Creates a `wasmtime::Engine` with `async_support`, `wasm_component_model`, and `wasm_component_model_async` all enabled.
- Holds an `Arc<Linker<HostState>>` for the base shim world.
- `async_handler_linker()` creates a separate `Linker` configured for the async handler world, additionally registering the `http-types` interface.
- `instantiate()` is an async method that compiles, links, and instantiates a component in one call, using `linker.instantiate_async()`.
- Enforces a 64 MiB memory limit and 10,000 table element limit per instance via `StoreLimitsBuilder`.

**`src/config.rs`**: `ShimConfig` controls which shim interfaces are registered with the linker. Each shim can be toggled independently via TOML config or the `ShimsConfig` from `warp-core`. Shims not enabled are simply not registered — if a guest imports a disabled interface, instantiation fails at link time.

**`HostState`**: Per-instance state struct that implements all five WIT `Host` traits (`filesystem`, `dns`, `signals`, `database_proxy`, `threading`) by delegating to the individual shim implementations. The `http_types::Host` trait is empty (types-only interface).

### `warpgrid-trigger`

Bridges inbound HTTP to Wasm components.

`HttpTrigger` (`src/handler.rs`) manages a hyper HTTP/1.1 server. For each accepted connection, it spawns a Tokio task. The request handler callback (`RequestHandler`) is an `Arc<dyn Fn(...) -> BoxFuture + Send + Sync>`, which routes requests to the appropriate Wasm component.

The trigger uses `wasmtime-wasi-http` (version 41, matching the source-built crates) for wasi-http proxy world bindings and type conversions. `src/convert.rs` handles translation between hyper/http types and wasi-http internal types.

### `warpgrid-async`

Provides Rust-level `Request` and `Response` types with streaming body support, designed to sit between the WIT boundary and handler code.

- **`Request::body_stream()`**: Yields the buffered request body in 64 KB chunks via zero-copy `Bytes::slice()`. Pull-based stream (via `futures_core::Stream`) guarantees at most 2× the chunk size (one input chunk + one output chunk) in memory at any time during a transform.
- **`Response`**: Can be constructed from a `Stream<Item = Bytes>` for incremental output without pre-buffering.

This crate does not directly use `wasmtime` or WIT; it is a pure Rust abstraction layer for handlers that want streaming semantics over the buffered `list<u8>` bodies that cross the WIT boundary today.

## Build Instructions

The build script clones Wasmtime at the pinned SHA and builds with `component-model-async` enabled.

**Prerequisites:**
- Rust toolchain (stable or nightly via `rustup`)
- `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`)
- `wasm-tools` CLI (`cargo install wasm-tools`)
- `git`

**Full pipeline (clone + build + verify):**

```bash
scripts/build-wasmtime-async.sh
```

**Step-by-step:**

```bash
scripts/build-wasmtime-async.sh --clone    # Clone at pinned SHA into vendor/wasmtime-async/
scripts/build-wasmtime-async.sh --build    # Build with --features component-model-async
scripts/build-wasmtime-async.sh --verify   # Verify the built binary
scripts/build-wasmtime-async.sh --clean    # Remove vendor/ and build/ directories
```

**Output locations:**

| Path | Contents |
|---|---|
| `vendor/wasmtime-async/` | Cloned source tree (git checkout at pinned SHA) |
| `build/wasmtime-async/wasmtime` | Built binary |
| `build/wasmtime-async/.build-stamp` | Stamp recording the SHA of the last successful build |

The stamp file prevents redundant rebuilds: if the binary already exists and the stamp matches the current SHA, the build step is skipped.

**Updating the SHA:**

1. Find the desired commit on `https://github.com/bytecodealliance/wasmtime` (`main` branch).
2. Replace the hash in `scripts/WASMTIME_ASYNC_SHA` (keep the comment lines).
3. Run `scripts/build-wasmtime-async.sh --clean && scripts/build-wasmtime-async.sh` to rebuild from scratch.
4. Update the `# Date:` comment in the SHA file to match the commit date.
5. CI caches are keyed on the SHA file — the cache will be invalidated automatically on the next CI run.

## Known Limitations and Differences from crates.io

**Not on crates.io**: The `component-model-async` feature and the WASI Preview 3 standard interfaces (`wasmtime-wasi` `p3` feature) are not available in any published Wasmtime release as of the pinned date (2026-03-13). The workspace depends on version `"41"` in `Cargo.toml`, but `Cargo.lock` must point at the locally patched source build or a git dependency. Production builds require the source build.

**API stability**: WASI 0.3 is still being finalized. WIT-level types for `future<T>` and `stream<T>` are experimental. WarpGrid's current WIT interfaces avoid using these types directly (the `async-handler` WIT signature is synchronous; concurrency is managed by the runtime, not the WIT type system). This isolation means WarpGrid's WIT files should remain stable even as the upstream API evolves.

**Threading model**: True parallel threading (multiple OS threads sharing a Wasm instance's linear memory) is not supported by the component model or the cooperative Tokio scheduler. Guests that declare `threading-model: parallel-required` receive a warning from the host and fall back to cooperative execution. This is logged at `WARN` level and does not cause instantiation failure.

**Body buffering at WIT boundary**: The `http-request` and `http-response` types in WIT use `list<u8>` (a complete byte buffer) rather than WIT `stream<u8>`. This means the full request body must be materialized before crossing the WIT boundary. The `warpgrid-async` crate provides a streaming abstraction over this buffer for handlers that want incremental processing, but the network→Wasm copy is not zero-copy. Streaming WIT bodies are planned (referenced in the `http-types.wit` comment as US-505).

**`wasm_component_model_async` flag scope**: Enabling `wasm_component_model_async` on the Wasmtime `Config` applies to all components loaded by that engine. There is currently no per-component opt-out. All WarpGrid components run under an async-enabled engine.

**wasmtime-wasi-http version**: `warpgrid-trigger` depends on `wasmtime-wasi-http = "41"` from crates.io. This version may lag behind the source-built wasmtime in API surface. If the source build advances far ahead of the crates.io release, type mismatches between the two may require a path dependency or git dependency for `wasmtime-wasi-http` as well.
