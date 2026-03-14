# Async Rust Handler

A WarpGrid async handler written in Rust.

## Prerequisites

- Rust toolchain with `wasm32-unknown-unknown` target
- `warp` CLI

## Getting Started

```bash
# Build the Wasm component
warp pack

# The output .wasm component will be in the project directory
```

## Project Structure

- `src/lib.rs` — Handler implementation
- `warp.toml` — WarpGrid build configuration
- `wit/` — WIT interface definitions

## How It Works

The handler implements the `warpgrid:shim/async-handler` interface. It receives
HTTP requests and returns HTTP responses. The `handle-request` function is the
entry point invoked by the WarpGrid runtime.

The handler demonstrates:
- Health check endpoint (`/health`)
- Request echo (returns method, URI, and body length as JSON)
- `#![no_std]` pattern with `dlmalloc` allocator for minimal Wasm size
