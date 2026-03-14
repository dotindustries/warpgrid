use super::TemplateFile;

pub fn files() -> Vec<TemplateFile> {
    vec![
        TemplateFile {
            path: "Cargo.toml",
            content: CARGO_TOML,
        },
        TemplateFile {
            path: "warp.toml",
            content: WARP_TOML,
        },
        TemplateFile {
            path: "README.md",
            content: README,
        },
        TemplateFile {
            path: "src/lib.rs",
            content: LIB_RS,
        },
        TemplateFile {
            path: "wit/world.wit",
            content: include_str!("../../../../crates/warpgrid-host/wit/world.wit"),
        },
        TemplateFile {
            path: "wit/async-handler.wit",
            content: include_str!("../../../../crates/warpgrid-host/wit/async-handler.wit"),
        },
        TemplateFile {
            path: "wit/http-types.wit",
            content: include_str!("../../../../crates/warpgrid-host/wit/http-types.wit"),
        },
        TemplateFile {
            path: "wit/dns.wit",
            content: include_str!("../../../../crates/warpgrid-host/wit/dns.wit"),
        },
        TemplateFile {
            path: "wit/database-proxy.wit",
            content: include_str!("../../../../crates/warpgrid-host/wit/database-proxy.wit"),
        },
        TemplateFile {
            path: "wit/filesystem.wit",
            content: include_str!("../../../../crates/warpgrid-host/wit/filesystem.wit"),
        },
        TemplateFile {
            path: "wit/signals.wit",
            content: include_str!("../../../../crates/warpgrid-host/wit/signals.wit"),
        },
        TemplateFile {
            path: "wit/threading.wit",
            content: include_str!("../../../../crates/warpgrid-host/wit/threading.wit"),
        },
    ]
}

const CARGO_TOML: &str = r#"[package]
name = "my-async-handler"
version = "0.1.0"
edition = "2021"

[workspace]

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = { version = "0.42", default-features = false, features = ["macros"] }
wit-bindgen-rt = { version = "0.42", features = ["bitflags"] }
dlmalloc = { version = "0.2", features = ["global"] }
serde = { version = "1", default-features = false, features = ["derive", "alloc"] }
serde_json = { version = "1", default-features = false, features = ["alloc"] }

[profile.release]
opt-level = "s"
lto = true
"#;

const WARP_TOML: &str = r#"[package]
name = "my-async-handler"
version = "0.1.0"

[build]
lang = "rust"
"#;

const README: &str = r#"# Async Rust Handler

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
"#;

const LIB_RS: &str = r##"#![no_std]
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
                // Echo the request info back as JSON
                let response_body = format!(
                    "{{\"method\":\"{}\",\"uri\":\"{}\",\"body_length\":{}}}",
                    request.method,
                    uri,
                    body.len()
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
"##;
