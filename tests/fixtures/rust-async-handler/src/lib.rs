//! Rust async handler fixture for US-508.
//!
//! Demonstrates a non-trivial Wasm component that:
//! 1. Parses a JSON request body
//! 2. Queries a mock service via the DNS shim
//! 3. Returns a transformed JSON response
//!
//! This fixture validates that `wit-bindgen` generates correct bindings
//! from WarpGrid's WIT package and produces a component compatible with
//! `WarpGridEngine`'s async handler world.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

wit_bindgen::generate!({
    path: "wit",
    world: "rust-async-handler",
    generate_all,
});

// ── JSON request/response types ─────────────────────────────────

#[derive(Deserialize)]
#[allow(dead_code)]
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

// ── Handler implementation ──────────────────────────────────────

struct Component;

impl exports::warpgrid::shim::async_handler::Guest for Component {
    fn handle_request(
        request: warpgrid::shim::http_types::HttpRequest,
    ) -> warpgrid::shim::http_types::HttpResponse {
        use warpgrid::shim::http_types::{HttpHeader, HttpResponse};

        // Parse the JSON request body
        let query: ServiceQuery = match serde_json::from_slice(&request.body) {
            Ok(q) => q,
            Err(e) => {
                return json_error_response(400, format!("invalid JSON: {e}"));
            }
        };

        // Query the mock service via the DNS shim
        let addresses = match warpgrid::shim::dns::resolve_address(&query.hostname) {
            Ok(records) => records
                .iter()
                .map(|r| r.address.clone())
                .collect::<Vec<_>>(),
            Err(e) => {
                return json_error_response(502, format!("DNS resolution failed: {e}"));
            }
        };

        // Build transformed response
        let transformed = format!(
            "resolved {} to {} address(es)",
            query.hostname,
            addresses.len()
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
                HttpHeader {
                    name: "content-type".into(),
                    value: "application/json".into(),
                },
                HttpHeader {
                    name: "x-handler".into(),
                    value: "rust-async".into(),
                },
            ],
            body,
        }
    }
}

fn json_error_response(
    status: u16,
    message: String,
) -> warpgrid::shim::http_types::HttpResponse {
    use warpgrid::shim::http_types::{HttpHeader, HttpResponse};

    let error = ErrorResponse { error: message };
    let body = serde_json::to_vec(&error).unwrap_or_default();

    HttpResponse {
        status,
        headers: alloc::vec![HttpHeader {
            name: "content-type".into(),
            value: "application/json".into(),
        }],
        body,
    }
}

export!(Component);
