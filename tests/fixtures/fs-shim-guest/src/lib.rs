#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
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
    world: "fs-shim-test",
    generate_all,
});

struct Component;

impl Guest for Component {
    fn test_resolv_conf() -> Result<String, String> {
        let handle = warpgrid::shim::filesystem::open_virtual("/etc/resolv.conf")?;
        let data = warpgrid::shim::filesystem::read_virtual(handle, 4096)?;
        warpgrid::shim::filesystem::close_virtual(handle)?;
        String::from_utf8(data).map_err(|e| format!("invalid utf8: {e}"))
    }

    fn test_dev_urandom() -> Result<Vec<u8>, String> {
        let handle = warpgrid::shim::filesystem::open_virtual("/dev/urandom")?;
        let data = warpgrid::shim::filesystem::read_virtual(handle, 32)?;
        warpgrid::shim::filesystem::close_virtual(handle)?;
        Ok(data)
    }

    fn test_dev_null() -> Result<bool, String> {
        let handle = warpgrid::shim::filesystem::open_virtual("/dev/null")?;
        let data = warpgrid::shim::filesystem::read_virtual(handle, 4096)?;
        warpgrid::shim::filesystem::close_virtual(handle)?;
        Ok(data.is_empty())
    }

    fn test_etc_hosts() -> Result<String, String> {
        let handle = warpgrid::shim::filesystem::open_virtual("/etc/hosts")?;
        let data = warpgrid::shim::filesystem::read_virtual(handle, 4096)?;
        warpgrid::shim::filesystem::close_virtual(handle)?;
        String::from_utf8(data).map_err(|e| format!("invalid utf8: {e}"))
    }

    fn test_nonvirtual() -> Result<String, String> {
        match warpgrid::shim::filesystem::open_virtual("/tmp/nonexistent-file.txt") {
            Ok(_handle) => Err("expected error for non-virtual path".into()),
            Err(e) => Ok(e),
        }
    }
}

export!(Component);
