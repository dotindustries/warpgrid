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
    world: "signal-shim-test",
    generate_all,
});

struct Component;

impl Guest for Component {
    fn register_terminate() -> Result<(), String> {
        warpgrid::shim::signals::on_signal(warpgrid::shim::signals::SignalType::Terminate)
    }

    fn register_hangup() -> Result<(), String> {
        warpgrid::shim::signals::on_signal(warpgrid::shim::signals::SignalType::Hangup)
    }

    fn register_interrupt() -> Result<(), String> {
        warpgrid::shim::signals::on_signal(warpgrid::shim::signals::SignalType::Interrupt)
    }

    fn poll() -> Result<String, String> {
        match warpgrid::shim::signals::poll_signal() {
            Some(signal) => match signal {
                warpgrid::shim::signals::SignalType::Terminate => Ok(String::from("terminate")),
                warpgrid::shim::signals::SignalType::Hangup => Ok(String::from("hangup")),
                warpgrid::shim::signals::SignalType::Interrupt => Ok(String::from("interrupt")),
            },
            None => Ok(String::from("none")),
        }
    }
}

export!(Component);
