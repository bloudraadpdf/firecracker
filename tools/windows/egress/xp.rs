// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#![no_std]
#![no_main]

use core::{ffi::c_void, ptr};
mod http;
mod xp_pattern;
use http::{WriteFile, check, number, output};
use xp_pattern::storage_pattern;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetTickCount() -> u32;
    fn CreateFileA(
        path: *const u8,
        access: u32,
        share: u32,
        security: *mut c_void,
        creation: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn ReadFile(
        handle: usize,
        buffer: *mut u8,
        length: u32,
        read: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn SetFilePointer(handle: usize, distance: i32, high: *mut i32, method: u32) -> u32;
    fn FlushFileBuffers(handle: usize) -> i32;
    fn CloseHandle(handle: usize) -> i32;
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // SAFETY: ExitProcess has no pointer arguments and does not return.
    unsafe { http::ExitProcess(99) }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    storage();
    http::get(b"example.com\0", false);
    tcp_echo();
    // SAFETY: ExitProcess has no pointer arguments and does not return.
    unsafe { http::ExitProcess(0) }
}

#[link(name = "ws2_32")]
unsafe extern "system" {
    fn inet_addr(address: *const u8) -> u32;
    fn WSAStartup(version: u16, data: *mut u8) -> i32;
    fn WSACleanup() -> i32;
    fn socket(family: i32, kind: i32, protocol: i32) -> usize;
    fn connect(socket: usize, address: *const u8, length: i32) -> i32;
    fn send(socket: usize, data: *const u8, length: i32, flags: i32) -> i32;
    fn recv(socket: usize, data: *mut u8, length: i32, flags: i32) -> i32;
    fn setsockopt(socket: usize, level: i32, option: i32, value: *const u8, length: i32) -> i32;
    fn closesocket(socket: usize) -> i32;
}

fn tcp_echo() {
    let mut wsa = [0u64; 64];
    let mut sent = [0u8; 1024];
    let mut received = [0u8; 1024];
    // SAFETY: wsa is aligned and larger than WSADATA; addresses are terminated or fixed-size.
    // The checked socket and live buffers have the declared lengths and close after use.
    unsafe {
        check(
            WSAStartup(0x202, wsa.as_mut_ptr().cast()) == 0,
            b"WSAStartup",
        );
        let fd = socket(2, 1, 6);
        check(fd != usize::MAX, b"socket");
        let timeout = 30000u32;
        for option in [0x1005, 0x1006] {
            check(
                setsockopt(fd, 0xffff, option, (&timeout as *const u32).cast(), 4) == 0,
                b"setsockopt",
            );
        }
        let port = 40569u16.to_be_bytes();
        let host = inet_addr(concat!(env!("WINDOWS_EGRESS_HOST"), "\0").as_ptr());
        check(host != u32::MAX, b"host IPv4 address");
        let mut address = [2, 0, port[0], port[1], 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        address[4..8].copy_from_slice(&host.to_ne_bytes());
        check(connect(fd, address.as_ptr(), 16) == 0, b"connect");
        for chunk in 0..2048usize {
            for (i, byte) in sent.iter_mut().enumerate() {
                *byte = (i.wrapping_mul(37) ^ chunk ^ (chunk >> 8)) as u8;
            }
            let mut offset = 0;
            while offset < sent.len() {
                let count = send(
                    fd,
                    sent.as_ptr().add(offset),
                    (sent.len() - offset) as i32,
                    0,
                );
                check(count > 0, b"send");
                offset += count as usize;
            }
            offset = 0;
            while offset < received.len() {
                let count = recv(
                    fd,
                    received.as_mut_ptr().add(offset),
                    (received.len() - offset) as i32,
                    0,
                );
                check(count > 0, b"recv");
                offset += count as usize;
            }
            check(sent == received, b"payload mismatch");
        }
        closesocket(fd);
        WSACleanup();
    }
    output(b"PASS XP_TCP_EQUAL bytes=2097152\r\n");
}

fn storage() {
    let mut data = [0u8; 1024];
    let mut received = [0u8; 1024];
    // SAFETY: the path is terminated, the handle is checked and all synchronous buffers are
    // live for their declared sizes. The file closes after its last read.
    unsafe {
        let seed = GetTickCount();
        let file = CreateFileA(
            c"C:\\fc-restore-data.bin".as_ptr().cast(),
            0xc0000000,
            0,
            ptr::null_mut(),
            2,
            0x80000080,
            0,
        );
        check(file != usize::MAX, b"CreateFile");
        for chunk in 0..64 {
            for (i, byte) in data.iter_mut().enumerate() {
                *byte = storage_pattern(seed, chunk * 1024 + i);
            }
            let mut count = 0;
            check(
                WriteFile(file, data.as_ptr(), 1024, &mut count, ptr::null_mut()) != 0
                    && count == 1024,
                b"storage write",
            );
        }
        check(FlushFileBuffers(file) != 0, b"storage flush");
        check(
            SetFilePointer(file, 0, ptr::null_mut(), 0) == 0,
            b"storage seek",
        );
        for chunk in 0..64 {
            let mut count = 0;
            check(
                ReadFile(
                    file,
                    received.as_mut_ptr(),
                    1024,
                    &mut count,
                    ptr::null_mut(),
                ) != 0
                    && count == 1024,
                b"storage read",
            );
            for (i, byte) in received.iter().enumerate() {
                check(
                    *byte == storage_pattern(seed, chunk * 1024 + i),
                    b"storage compare",
                );
            }
        }
        check(CloseHandle(file) != 0, b"storage close");
        output(b"PASS XP_STORAGE bytes=65536 seed=");
        number(seed);
        output(b"\r\n");
    }
}
