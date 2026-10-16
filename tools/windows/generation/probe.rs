// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#![no_std]
#![no_main]

#[path = "../egress/win32.rs"]
mod win32;

use core::ffi::c_void;
use core::ptr;
use win32::{ExitProcess, GetLastError, GetStdHandle, WriteFile};

// CTL_CODE(FILE_DEVICE_ACPI, 1, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS).
const IOCTL_VMGENCOUNTER_READ: u32 = (0x32 << 16) | (3 << 14) | (1 << 2);

#[repr(C)]
struct VmGenCounter {
    generation_count: u64,
    generation_count_high: u64,
}

const _: () = assert!(size_of::<VmGenCounter>() == 16);
const _: () = assert!(core::mem::offset_of!(VmGenCounter, generation_count_high) == 8);

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        attributes: u32,
        template: usize,
    ) -> usize;
    fn DeviceIoControl(
        handle: usize,
        code: u32,
        input: *const c_void,
        input_len: u32,
        output: *mut c_void,
        output_len: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn CloseHandle(handle: usize) -> i32;
}

struct Handle(usize);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: this scope owns the valid handle and all synchronous I/O has finished.
        unsafe { CloseHandle(self.0) };
    }
}

enum Failure {
    Win32(&'static [u8], u32),
    OutputSize(u32),
}

fn query() -> Result<VmGenCounter, Failure> {
    let name = (*b"\\\\.\\VmGenerationCounter\0").map(u16::from);
    // SAFETY: name is terminated UTF-16 and remains live for the synchronous open.
    let raw = unsafe { CreateFileW(name.as_ptr(), 0xc0000000, 3, ptr::null(), 3, 0, 0) };
    if raw == usize::MAX {
        // SAFETY: GetLastError has no arguments; capture before any cleanup can alter it.
        return Err(Failure::Win32(b"CreateFileW", unsafe { GetLastError() }));
    }
    let handle = Handle(raw);
    let mut counter = VmGenCounter {
        generation_count: 0,
        generation_count_high: 0,
    };
    let mut returned = 0;
    // SAFETY: handle is live; both output buffers are writable for their declared sizes.
    // No input selects polling mode; this synchronous call has no OVERLAPPED object.
    let ok = unsafe {
        DeviceIoControl(
            handle.0,
            IOCTL_VMGENCOUNTER_READ,
            ptr::null(),
            0,
            (&raw mut counter).cast(),
            16,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        // SAFETY: GetLastError has no arguments; capture before Handle is dropped.
        return Err(Failure::Win32(b"DeviceIoControl", unsafe {
            GetLastError()
        }));
    }
    if returned != 16 {
        return Err(Failure::OutputSize(returned));
    }
    Ok(counter)
}

fn output(mut bytes: &[u8]) {
    // SAFETY: GetStdHandle takes a standard handle identifier and no pointers.
    let stdout = unsafe { GetStdHandle(-11i32 as u32) };
    while !bytes.is_empty() {
        let mut written = 0;
        // SAFETY: bytes remains readable and written is writable during synchronous I/O.
        let ok = unsafe {
            WriteFile(
                stdout,
                bytes.as_ptr(),
                u32::try_from(bytes.len()).unwrap(),
                &mut written,
                ptr::null_mut(),
            )
        };
        if ok == 0 || written == 0 || written as usize > bytes.len() {
            // SAFETY: ExitProcess takes no pointers and never returns. The query handle is closed.
            unsafe { ExitProcess(2) }
        }
        bytes = &bytes[written as usize..];
    }
}

fn hex(mut value: u64) {
    let mut digits = [b'0'; 16];
    for digit in digits.iter_mut().rev() {
        *digit = b"0123456789abcdef"[(value & 15) as usize];
        value >>= 4;
    }
    output(&digits);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // SAFETY: ExitProcess takes no pointers and never returns.
    unsafe { ExitProcess(99) }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let result = query();
    let code = match result {
        Ok(counter) => {
            output(b"VMGENID=");
            hex(counter.generation_count_high);
            hex(counter.generation_count);
            output(b"\r\n");
            0
        }
        Err(Failure::Win32(stage, error)) => {
            output(b"FAIL VMGENID ");
            output(stage);
            output(b" win32=0x");
            hex(u64::from(error));
            output(b"\r\n");
            if error == 5 {
                output(b"Run as Administrator or SYSTEM.\r\n");
            }
            1
        }
        Err(Failure::OutputSize(size)) => {
            output(b"FAIL VMGENID output_bytes=0x");
            hex(u64::from(size));
            output(b" expected=0x10\r\n");
            1
        }
    };
    // SAFETY: ExitProcess takes no pointers and never returns. query closed its handle.
    unsafe { ExitProcess(code) }
}
