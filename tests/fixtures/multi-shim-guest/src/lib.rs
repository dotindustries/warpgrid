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
    path: "wit",
    world: "multi-shim-test",
    generate_all,
});

struct Component;

impl Guest for Component {
    fn test_fs_read() -> Result<String, String> {
        let handle = warpgrid::shim::filesystem::open_virtual("/etc/resolv.conf")?;
        let data = warpgrid::shim::filesystem::read_virtual(handle, 4096)?;
        warpgrid::shim::filesystem::close_virtual(handle)?;
        String::from_utf8(data).map_err(|e| format!("invalid utf8: {e}"))
    }

    fn test_dns_resolve() -> Result<String, String> {
        let records = warpgrid::shim::dns::resolve_address("localhost")?;
        if records.is_empty() {
            return Err("no records returned".into());
        }
        Ok(records[0].address.clone())
    }

    fn test_signal_register() -> Result<(), String> {
        warpgrid::shim::signals::on_signal(warpgrid::shim::signals::SignalType::Terminate)
    }

    fn test_threading_declare() -> Result<(), String> {
        warpgrid::shim::threading::declare_threading_model(
            warpgrid::shim::threading::ThreadingModel::Cooperative,
        )
    }
}

export!(Component);
