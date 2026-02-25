#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

wit_bindgen::generate!({
    path: "../wit",
    world: "gateway-service",
    generate_all,
});

struct Component;

impl Guest for Component {
    /// Routes requests to downstream services by resolving DNS names.
    ///
    /// Returns (status, body, error_source):
    /// - On success: (200, "resolved:<service>:<ip>", "")
    /// - On DNS failure: (503, error_message, failing_service_hostname)
    fn handle_request(
        method: String,
        path: String,
        body: String,
    ) -> Result<(u16, String, String), String> {
        // Determine target service from request path.
        let service_hostname = if path.starts_with("/users") {
            "user-svc.test.warp.local"
        } else if path.starts_with("/notify") {
            "notification-svc.test.warp.local"
        } else if path.starts_with("/analytics") {
            "analytics-svc.test.warp.local"
        } else {
            return Ok((404, format!("unknown path: {path}"), String::new()));
        };

        // Resolve the service hostname via the DNS shim.
        match warpgrid::shim::dns::resolve_address(service_hostname) {
            Ok(addrs) => {
                if addrs.is_empty() {
                    return Ok((
                        503,
                        format!("no addresses for {service_hostname}"),
                        String::from(service_hostname),
                    ));
                }
                let first_addr = &addrs[0].address;
                // Return routing information: method, resolved address, and forwarded body.
                Ok((
                    200,
                    format!("resolved:{service_hostname}:{first_addr}:{method}:{body}"),
                    String::new(),
                ))
            }
            Err(e) => {
                // Downstream DNS failure — include X-WarpGrid-Source-Service.
                Ok((503, format!("dns error: {e}"), String::from(service_hostname)))
            }
        }
    }
}

export!(Component);
