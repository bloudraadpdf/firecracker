// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

#![no_std]
#![no_main]

#[cfg(not(all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
compile_error!("The QXL miniport targets x86 or x64 Windows.");

use core::ffi::c_void;
use core::panic::PanicInfo;

#[cfg_attr(
    target_arch = "x86",
    link(
        name = "ntoskrnl.exe",
        kind = "raw-dylib",
        modifiers = "+verbatim",
        import_name_type = "undecorated"
    )
)]
#[cfg_attr(
    target_arch = "x86_64",
    link(name = "ntoskrnl.exe", kind = "raw-dylib", modifiers = "+verbatim")
)]
unsafe extern "system" {
    fn KeBugCheckEx(
        code: u32,
        parameter1: usize,
        parameter2: usize,
        parameter3: usize,
        parameter4: usize,
    ) -> !;
}

mod memory;

/// # Safety
/// The Windows loader must supply its DriverEntry arguments at PASSIVE_LEVEL.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DriverEntry(driver: *mut c_void, registry: *mut c_void) -> u32 {
    // SAFETY: the loader supplies the registration context and resident driver code.
    unsafe { firecracker_qxl_driver::miniport::driver_entry(driver, registry) }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    let (file, length, line, column) = match info.location() {
        Some(location) => (
            location.file().as_ptr() as usize,
            location.file().len(),
            location.line() as usize,
            location.column() as usize,
        ),
        None => (0, 0, 0, 0),
    };
    // SAFETY: KeBugCheckEx is callable at any IRQL and does not return. The RPNC
    // code follows Microsoft's Rust panic convention and preserves source location.
    unsafe { KeBugCheckEx(u32::from_be_bytes(*b"RPNC"), file, length, line, column) }
}
