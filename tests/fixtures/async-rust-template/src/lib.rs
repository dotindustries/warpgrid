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
