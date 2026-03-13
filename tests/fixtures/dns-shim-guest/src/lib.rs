#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

wit_bindgen::generate!({
    path: "wit",
    world: "dns-shim-test",
    generate_all,
});

struct Component;

impl Guest for Component {
    fn test_resolve_registry() -> Result<String, String> {
        let records = warpgrid::shim::dns::resolve_address("db.test.warp.local")?;
        if records.is_empty() {
            return Err("no records returned".into());
        }
        Ok(records[0].address.clone())
    }

    fn test_resolve_etc_hosts() -> Result<String, String> {
        let records = warpgrid::shim::dns::resolve_address("cache.test.warp.local")?;
        if records.is_empty() {
            return Err("no records returned".into());
        }
        Ok(records[0].address.clone())
    }

    fn test_resolve_system_dns() -> Result<String, String> {
        let records = warpgrid::shim::dns::resolve_address("localhost")?;
        if records.is_empty() {
            return Err("no records returned".into());
        }
        Ok(records[0].address.clone())
    }

    fn test_resolve_round_robin() -> Result<String, String> {
        // Call resolve-address 3 times for a multi-address hostname
        // and collect all addresses from each call.
        let mut all_addresses: Vec<String> = Vec::new();

        for _ in 0..3 {
            let records = warpgrid::shim::dns::resolve_address("api.test.warp.local")?;
            for rec in &records {
                let addr = rec.address.clone();
                // Only add if not already seen
                let mut found = false;
                for existing in &all_addresses {
                    if *existing == addr {
                        found = true;
                        break;
                    }
                }
                if !found {
                    all_addresses.push(addr);
                }
            }
        }

        // Sort for deterministic output
        all_addresses.sort();
        let joined = all_addresses.join(",");
        Ok(joined)
    }

    fn test_resolve_nonexistent() -> Result<String, String> {
        match warpgrid::shim::dns::resolve_address("nonexistent.invalid") {
            Ok(_records) => Err("expected error for nonexistent hostname".into()),
            Err(e) => Ok(e),
        }
    }
}

export!(Component);
