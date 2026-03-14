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
    ]
}

const CARGO_TOML: &str = r#"[package]
name = "my-async-handler"
version = "0.1.0"
edition = "2021"

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
- `wit/` — WIT interface definitions (added during build)

## How It Works

The handler implements the `warpgrid:shim/async-handler` interface. It receives
HTTP requests and returns HTTP responses. The `handle-request` function is the
entry point invoked by the WarpGrid runtime.
"#;

const LIB_RS: &str = r##"#![no_std]
#![no_main]

extern crate alloc;

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

wit_bindgen::generate!({
    path: "wit",
    world: "rust-async-handler",
    generate_all,
});

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use exports::warpgrid::shim::async_handler::Guest;

struct Component;

impl Guest for Component {
    fn handle_request(request: warpgrid::shim::http_types::HttpRequest) -> warpgrid::shim::http_types::HttpResponse {
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
                // Echo the request body back as JSON
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
