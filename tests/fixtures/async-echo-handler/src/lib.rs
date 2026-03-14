//! Minimal async echo handler fixture for US-501/US-502 integration tests.
//!
//! Returns the request body as the response body with status 200
//! and an `x-async: true` header.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;

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

struct Component;

impl exports::warpgrid::shim::async_handler::Guest for Component {
    fn handle_request(
        request: warpgrid::shim::http_types::HttpRequest,
    ) -> warpgrid::shim::http_types::HttpResponse {
        use warpgrid::shim::http_types::{HttpHeader, HttpResponse};

        HttpResponse {
            status: 200,
            headers: vec![
                HttpHeader {
                    name: "x-async".into(),
                    value: "true".into(),
                },
            ],
            body: request.body,
        }
    }
}

export!(Component);
