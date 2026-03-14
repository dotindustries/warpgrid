#![no_std]
#![no_main]

extern crate alloc;

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
    world: "threading-shim-test",
    generate_all,
});

struct Component;

impl Guest for Component {
    fn declare_cooperative() -> Result<(), String> {
        warpgrid::shim::threading::declare_threading_model(
            warpgrid::shim::threading::ThreadingModel::Cooperative,
        )
    }

    fn declare_parallel_required() -> Result<(), String> {
        warpgrid::shim::threading::declare_threading_model(
            warpgrid::shim::threading::ThreadingModel::ParallelRequired,
        )
    }
}

export!(Component);
