// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#![no_std]
#![no_main]

mod http;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // SAFETY: ExitProcess has no pointer arguments and does not return.
    unsafe { http::ExitProcess(99) }
}

#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup() -> ! {
    http::get(b"www.microsoft.com\0", true);
    // SAFETY: ExitProcess has no pointer arguments and does not return.
    unsafe { http::ExitProcess(0) }
}
