# Deploy a Rust App

This guide walks through deploying a Rust async handler to WarpGrid.

## Prerequisites

- The `warp` CLI
- [Rust](https://rustup.rs) toolchain with the `wasm32-wasip2` target:
  ```bash
  rustup target add wasm32-wasip2
  ```
- A WarpGrid Cloud account (`warp login`)

## Create the Project

Scaffold from the async-rust template:

```bash
warp init --template async-rust my-rust-api
cd my-rust-api
```

Generated structure:

```
my-rust-api/
  warp.toml
  Cargo.toml
  src/
    lib.rs
  wit/
    ...          # WIT interface definitions
```

## Understand the Manifest

```toml
[package]
name = "my-rust-api"
version = "0.1.0"
description = "Minimal Rust async handler example for WarpGrid"

[build]
lang = "rust"
entry = "src/lib.rs"
target = "wasip2"

[runtime]
trigger = "http"
```

## Write the Handler

Rust handlers implement the `warpgrid-handler` WIT world. The template uses
`wit_bindgen` to generate bindings and `no_std` for a minimal binary. Here is
the handler from `examples/rust-handler`:

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

wit_bindgen::generate!({
    path: "wit",
    world: "warpgrid-handler",
    generate_all,
});

use alloc::string::String;
use alloc::vec;
use exports::warpgrid::shim::async_handler::Guest;
use warpgrid::shim::http_types::{HttpHeader, HttpRequest, HttpResponse};

struct Component;

fn json_response(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![HttpHeader {
            name: "content-type".into(),
            value: "application/json".into(),
        }],
        body: body.as_bytes().to_vec(),
    }
}

impl Guest for Component {
    fn handle_request(request: HttpRequest) -> HttpResponse {
        match (request.method.as_str(), request.uri.as_str()) {
            ("GET", "/") => json_response(
                200,
                "{\"message\":\"Hello from WarpGrid!\",\"runtime\":\"rust\"}",
            ),
            ("GET", "/health") => json_response(200, "{\"status\":\"ok\"}"),
            ("POST", "/uppercase") => {
                let body_str =
                    String::from_utf8(request.body).unwrap_or_default();
                let uppercased = body_str.to_uppercase();
                let escaped = uppercased
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"");
                let response_body =
                    alloc::format!("{{\"result\":\"{}\"}}", escaped);
                json_response(200, &response_body)
            }
            _ => json_response(404, "{\"error\":\"Not Found\"}"),
        }
    }
}

export!(Component);
```

The handler receives an `HttpRequest` and returns an `HttpResponse`. Routing is
done with pattern matching on the method and URI.

## Why `no_std`?

Using `no_std` keeps the compiled Wasm binary small (often under 500 KB). The
`dlmalloc` allocator provides heap allocation without pulling in the full Rust
standard library. If you need `std` features like `HashMap` or formatting, you
can use `alloc` crate equivalents.

## Build Locally

Compile the component without deploying:

```bash
warp pack --path .
```

This produces a `.wasm` file in the `target/` directory. You can inspect its
size and hash in the output.

## Deploy

```bash
warp deploy --region iad
```

```
Compiling project...
  Compiled: target/my-rust-api.wasm (480 KB, sha256: c3d4e5f6a7b8)
Deploying 'my-rust-api' to iad (491520 bytes)...
Deployed successfully!
  Name:      my-rust-api
  URL:       https://my-rust-api.you.edge.warpgrid.dev
  Wasm hash: c3d4e5f6a7b8
```

Rust handlers produce the smallest binaries of any supported language.

## Add Resource Limits

Configure memory and CPU weight for your deployment:

```toml
[runtime]
trigger = "http"
min_instances = 1
max_instances = 10

[runtime.resources]
memory_limit = "64MB"
cpu_weight = 100
```

## Configure Autoscaling

```toml
[runtime.scaling]
metric = "rps"
target_value = 200
scale_up_window = "30s"
scale_down_window = "120s"
```

This scales up when requests per second exceed 200 and scales down after two
minutes of low traffic. See the [Scaling guide](scaling.md) for more details.

## Full Example

See [`examples/rust-handler/`](https://github.com/dotindustries/warpgrid/tree/main/examples/rust-handler)
in the repository.
