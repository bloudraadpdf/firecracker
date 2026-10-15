// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#![no_std]
#![no_main]
use core::{
    ffi::c_void,
    fmt::{self, Write},
};
#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> *mut c_void;
    fn WriteFile(
        handle: *mut c_void,
        data: *const u8,
        size: u32,
        written: *mut u32,
        overlap: *mut c_void,
    ) -> i32;
    fn ExitProcess(code: u32) -> !;
}
#[link(name = "tbs", kind = "raw-dylib")]
unsafe extern "system" {
    fn Tbsi_Context_Create(params: *const u32, context: *mut *mut c_void) -> u32;
    fn Tbsip_Submit_Command(
        context: *mut c_void,
        locality: u32,
        priority: u32,
        command: *const u8,
        command_size: u32,
        response: *mut u8,
        response_size: *mut u32,
    ) -> u32;
    fn Tbsip_Context_Close(context: *mut c_void) -> u32;
}
struct Console;
impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut count = 0;
        let result = unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                s.as_ptr(),
                s.len() as u32,
                &mut count,
                core::ptr::null_mut(),
            )
        };
        if result == 0 || count as usize != s.len() {
            Err(fmt::Error)
        } else {
            Ok(())
        }
    }
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    unsafe { ExitProcess(99) }
}
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup() -> ! {
    let mut context = core::ptr::null_mut();
    let status = unsafe { Tbsi_Context_Create([2u32, 4].as_ptr(), &mut context) };
    let _ = writeln!(Console, "TBS context status={status:#010x}");
    if status != 0 {
        unsafe { ExitProcess(1) }
    }
    let command = [0x80u8, 1, 0, 0, 0, 12, 0, 0, 1, 0x7b, 0, 16];
    let mut response = [0u8; 512];
    let mut size = response.len() as u32;
    let status = unsafe {
        Tbsip_Submit_Command(
            context,
            0,
            200,
            command.as_ptr(),
            command.len() as u32,
            response.as_mut_ptr(),
            &mut size,
        )
    };
    let close = unsafe { Tbsip_Context_Close(context) };
    let _ = writeln!(
        Console,
        "TBS submit status={status:#010x} bytes={size} close={close:#010x}"
    );
    if status != 0 || close != 0 || !(12..=28).contains(&size) {
        unsafe { ExitProcess(2) }
    }
    let code = u32::from_be_bytes(response[6..10].try_into().unwrap());
    let declared = u32::from_be_bytes(response[2..6].try_into().unwrap());
    let random = u16::from_be_bytes(response[10..12].try_into().unwrap());
    let _ = writeln!(
        Console,
        "TPM GetRandom status={code:#010x} random_bytes={random}"
    );
    if code != 0
        || declared != size
        || u32::from(random) + 12 != size
        || random == 0
        || response[..2] != [0x80, 1]
    {
        unsafe { ExitProcess(3) }
    }
    let _ = writeln!(Console, "PASS Windows TBS TPM2_GetRandom");
    unsafe { ExitProcess(0) }
}
