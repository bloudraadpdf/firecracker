// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use core::{ffi::c_void, mem::MaybeUninit, ptr};

#[link(name = "winhttp")]
unsafe extern "system" {
    fn WinHttpOpen(
        agent: *const u16,
        access: u32,
        proxy: *const u16,
        bypass: *const u16,
        flags: u32,
    ) -> usize;
    fn WinHttpSetOption(handle: usize, option: u32, value: *const c_void, length: u32) -> i32;
    fn WinHttpSetTimeouts(
        handle: usize,
        resolve: i32,
        connect: i32,
        send: i32,
        receive: i32,
    ) -> i32;
    fn WinHttpConnect(session: usize, server: *const u16, port: u16, reserved: u32) -> usize;
    fn WinHttpOpenRequest(
        connect: usize,
        verb: *const u16,
        path: *const u16,
        version: *const u16,
        referer: *const u16,
        accepts: *const *const u16,
        flags: u32,
    ) -> usize;
    fn WinHttpSendRequest(
        request: usize,
        headers: *const u16,
        header_length: u32,
        body: *const c_void,
        body_length: u32,
        total_length: u32,
        context: usize,
    ) -> i32;
    fn WinHttpReceiveResponse(request: usize, reserved: *mut c_void) -> i32;
    fn WinHttpQueryHeaders(
        request: usize,
        info: u32,
        name: *const u16,
        buffer: *mut c_void,
        length: *mut u32,
        index: *mut u32,
    ) -> i32;
    fn WinHttpReadData(request: usize, buffer: *mut c_void, length: u32, read: *mut u32) -> i32;
    fn WinHttpCloseHandle(handle: usize) -> i32;
}

#[path = "win32.rs"]
mod win32;
pub use win32::{ExitProcess, WriteFile};
use win32::{GetLastError, GetStdHandle};

pub fn output(bytes: &[u8]) {
    let mut written = 0;
    // SAFETY: bytes is live for its bounded length; written is writable and I/O is synchronous.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            ptr::null_mut(),
        );
    }
}

pub fn number(mut value: u32) {
    let mut bytes = [b'0'; 10];
    let mut index = bytes.len();
    loop {
        index -= 1;
        bytes[index] += (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    output(&bytes[index..]);
}

pub fn check(success: bool, stage: &[u8]) {
    if !success {
        // SAFETY: GetLastError takes no arguments.
        let error = unsafe { GetLastError() };
        output(stage);
        output(b" error=");
        number(error);
        output(b"\r\n");
        // SAFETY: ExitProcess has no pointer arguments and does not return.
        unsafe { ExitProcess(1) }
    }
}

fn wide<const N: usize>(bytes: &[u8; N]) -> [u16; N] {
    core::array::from_fn(|i| u16::from(bytes[i]))
}

pub fn get<const N: usize>(server: &[u8; N], tls: bool) {
    // SAFETY: callers supply terminated names; handles are checked and buffers stay live for
    // their declared sizes. WinHTTP calls are synchronous and handles close after their last use.
    unsafe {
        let session = WinHttpOpen(
            wide(b"FirecrackerEgressTest\0").as_ptr(),
            1,
            ptr::null(),
            ptr::null(),
            0,
        );
        check(session != 0, b"WinHttpOpen");
        if tls {
            let protocols = 0x800u32;
            check(
                WinHttpSetOption(session, 84, (&protocols as *const u32).cast(), 4) != 0,
                b"WinHttpSetOption TLS1.2",
            );
        }
        check(
            WinHttpSetTimeouts(session, 30000, 30000, 30000, 60000) != 0,
            b"WinHttpSetTimeouts",
        );
        let connection = WinHttpConnect(
            session,
            wide(server).as_ptr(),
            if tls { 443 } else { 80 },
            0,
        );
        check(connection != 0, b"WinHttpConnect");
        let request = WinHttpOpenRequest(
            connection,
            wide(b"GET\0").as_ptr(),
            wide(b"/\0").as_ptr(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            if tls { 0x800000 } else { 0 },
        );
        check(request != 0, b"WinHttpOpenRequest");
        check(
            WinHttpSendRequest(request, ptr::null(), 0, ptr::null(), 0, 0, 0) != 0,
            b"WinHttpSendRequest",
        );
        check(
            WinHttpReceiveResponse(request, ptr::null_mut()) != 0,
            b"WinHttpReceiveResponse",
        );
        let mut status = 0u32;
        let mut length = 4;
        check(
            WinHttpQueryHeaders(
                request,
                19 | 0x20000000,
                ptr::null(),
                (&mut status as *mut u32).cast(),
                &mut length,
                ptr::null_mut(),
            ) != 0,
            b"WinHttpQueryHeaders",
        );
        output(if tls {
            b"HTTPS status="
        } else {
            b"HTTP status="
        });
        number(status);
        output(b"\r\n");
        if status != 200 {
            ExitProcess(2);
        }
        let mut buffer = MaybeUninit::<[u8; 1024]>::uninit();
        let mut total = 0u32;
        loop {
            let mut count = 0;
            check(
                WinHttpReadData(request, buffer.as_mut_ptr().cast(), 1024, &mut count) != 0,
                b"WinHttpReadData",
            );
            if count == 0 {
                break;
            }
            total += count;
        }
        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connection);
        WinHttpCloseHandle(session);
        output(if tls {
            b"HTTPS TLS1.2 CERTIFICATE_VALIDATED DIRECT bytes="
        } else {
            b"HTTP DIRECT bytes="
        });
        number(total);
        output(b"\r\n");
    }
}
