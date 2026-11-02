// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#![no_std]
#![no_main]
use core::ffi::c_void;
#[link(name = "newdev")]
unsafe extern "system" {
    fn UpdateDriverForPlugAndPlayDevicesW(
        parent: *mut c_void,
        id: *const u16,
        inf: *const u16,
        flags: u32,
        reboot: *mut i32,
    ) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCommandLineW() -> *const u16;
    fn GetLastError() -> u32;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
    fn ExitProcess(code: u32) -> !;
}
#[link(name = "shell32")]
unsafe extern "system" {
    fn CommandLineToArgvW(command: *const u16, count: *mut i32) -> *mut *mut u16;
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ExitProcess has no pointer arguments and does not return.
    unsafe { ExitProcess(255) }
}
#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let mut reboot = 0;
    // SAFETY: Windows supplies terminated arguments. The synchronous driver call borrows
    // them before LocalFree releases the argument block; both out-parameters are writable.
    unsafe {
        let mut count = 0;
        let arguments = CommandLineToArgvW(GetCommandLineW(), &mut count);
        if arguments.is_null() {
            ExitProcess(GetLastError());
        }
        if count != 3 {
            LocalFree(arguments.cast());
            ExitProcess(87);
        }
        let result = UpdateDriverForPlugAndPlayDevicesW(
            core::ptr::null_mut(),
            *arguments.add(1),
            *arguments.add(2),
            1,
            &mut reboot,
        );
        let status = if result == 0 {
            GetLastError()
        } else if reboot != 0 {
            3010
        } else {
            0
        };
        LocalFree(arguments.cast());
        ExitProcess(status);
    }
}
