//! Rust async handler — minimal WarpGrid WASI component example.
//!
//! Routes:
//!   GET  /          — JSON greeting
//!   GET  /health    — health check
//!   POST /uppercase — converts request body to uppercase, returns as JSON

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
