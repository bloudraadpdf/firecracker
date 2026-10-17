// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#![no_std]
#![no_main]
use core::ptr;
#[path = "../egress/device.rs"]
mod device;
use device::{CloseHandle, CreateFileW, DeviceIoControl};
#[path = "../egress/win32.rs"]
mod win32;
use win32::{ExitProcess, GetLastError, GetStdHandle, WriteFile};
#[cfg(xp_matrix)]
mod xp_matrix;

#[link(name = "ws2_32")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut u8) -> i32;
    fn WSACleanup() -> i32;
    fn WSAGetLastError() -> i32;
    fn socket(af: i32, kind: i32, protocol: i32) -> usize;
    fn connect(fd: usize, address: *const u8, len: i32) -> i32;
    fn send(fd: usize, data: *const u8, len: i32, flags: i32) -> i32;
    fn recv(fd: usize, data: *mut u8, len: i32, flags: i32) -> i32;
    fn closesocket(fd: usize) -> i32;
    fn setsockopt(fd: usize, level: i32, name: i32, value: *const u8, len: i32) -> i32;
}
fn output(data: &[u8]) {
    let mut count = 0;
    // SAFETY: data is live for its bounded size; count is writable and I/O is synchronous.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            data.as_ptr(),
            data.len() as u32,
            &mut count,
            ptr::null_mut(),
        );
    }
}
fn number(mut value: u32) {
    let mut bytes = [b'0'; 10];
    let mut offset = bytes.len();
    loop {
        offset -= 1;
        bytes[offset] += (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    output(&bytes[offset..]);
}
fn require(ok: bool, stage: &[u8]) {
    if !ok {
        // SAFETY: both error getters take no arguments.
        let win = unsafe { GetLastError() };
        // SAFETY: WSAGetLastError takes no arguments.
        let wsa = unsafe { WSAGetLastError() };
        output(b"FAIL ");
        output(stage);
        output(b" win32=");
        number(win);
        output(b" wsa=");
        number(wsa as u32);
        output(b"\r\n");
        // SAFETY: ExitProcess has no pointer arguments and does not return.
        unsafe { ExitProcess(1) }
    }
}
struct Socket(usize);
impl Socket {
    fn open(af: u32) -> Self {
        // SAFETY: Winsock is initialised and socket takes no pointer arguments.
        let fd = unsafe { socket(af as i32, 1, 0) };
        require(fd != usize::MAX, b"socket");
        let result = Self(fd);
        let timeout = 30000u32;
        for option in [0x1005, 0x1006] {
            require(
                // SAFETY: fd is live; timeout is readable for the declared four bytes.
                unsafe { setsockopt(fd, 0xffff, option, (&timeout as *const u32).cast(), 4) } == 0,
                b"timeout",
            );
        }
        let mut address = [0u8; 16];
        address[..2].copy_from_slice(&(af as u16).to_le_bytes());
        address[4..8].copy_from_slice(&4750u32.to_le_bytes());
        address[8..12].copy_from_slice(&2u32.to_le_bytes());
        require(
            // SAFETY: fd is live and address contains the complete 16-byte Viosock address.
            unsafe { connect(fd, address.as_ptr(), 16) } == 0,
            b"connect",
        );
        result
    }
    fn write(&self, mut data: &[u8]) {
        while !data.is_empty() {
            // SAFETY: the socket is live and callers supply bounded readable message slices.
            let count = unsafe { send(self.0, data.as_ptr(), data.len() as i32, 0) };
            require(count > 0, b"send");
            data = &data[count as usize..];
        }
    }
    fn read(&self, mut data: &mut [u8]) {
        while !data.is_empty() {
            // SAFETY: the socket is live and callers supply bounded writable message slices.
            let count = unsafe { recv(self.0, data.as_mut_ptr(), data.len() as i32, 0) };
            require(count > 0, b"recv");
            data = &mut data[count as usize..];
        }
    }
    fn response(&self, expected: &[u8]) {
        let mut head = [0u8; 1024];
        let mut len = 0;
        loop {
            require(len < head.len(), b"header length");
            self.read(&mut head[len..len + 1]);
            output(&head[len..len + 1]);
            len += 1;
            if head[..len].ends_with(b"\r\n\r\n") {
                break;
            }
        }
        require(head[..len].starts_with(expected), b"HTTP status");
        output(&head[..len]);
    }
}
impl Drop for Socket {
    fn drop(&mut self) {
        // SAFETY: this object owns the live socket and closes it once.
        unsafe {
            closesocket(self.0);
        }
    }
}
fn run() {
    let mut wsa = [0u64; 64];
    require(
        // SAFETY: wsa is aligned and larger than WSADATA; WSAStartup initialises it in place.
        unsafe { WSAStartup(0x202, wsa.as_mut_ptr().cast()) } == 0,
        b"WSAStartup",
    );
    let name = (*b"\\\\.\\Viosock\0").map(u16::from);
    // SAFETY: name is terminated UTF-16 and stays live for the synchronous open.
    let handle = unsafe { CreateFileW(name.as_ptr(), 0x80000000, 1, ptr::null(), 3, 0x80, 0) };
    require(handle != usize::MAX, b"open Viosock");
    let mut af = 0u32;
    let mut returned = 0;
    // SAFETY: handle is live; af and returned are writable for their declared sizes, and
    // the synchronous IOCTL has no input or overlapped buffer.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            0x0801300c,
            ptr::null(),
            0,
            (&raw mut af).cast(),
            4,
            &mut returned,
            ptr::null_mut(),
        )
    };
    require(
        ok != 0 && returned == 4 && af > 0 && af <= u16::MAX as u32,
        b"address family",
    );
    // SAFETY: handle is owned here and has no subsequent use.
    unsafe {
        CloseHandle(handle);
    }
    {
        let allowed = Socket::open(af);
        allowed.write(b"CONNECT 127.0.0.1:40567 HTTP/1.1\r\nHost: 127.0.0.1:40567\r\n\r\n");
        allowed.response(b"HTTP/1.1 200 ");
        let mut sent = [0u8; 1024];
        let mut received = [0u8; 1024];
        for chunk in 0..2048usize {
            for (i, byte) in sent.iter_mut().enumerate() {
                *byte = (i.wrapping_mul(37) ^ chunk ^ (chunk >> 8)) as u8;
            }
            allowed.write(&sent);
            allowed.read(&mut received);
            require(sent == received, b"payload equality");
        }
        output(b"PASS LEGACY_CONNECT bytes=2097152\r\n");
    }
    {
        let denied = Socket::open(af);
        denied.write(b"CONNECT 127.0.0.1:40568 HTTP/1.1\r\nHost: 127.0.0.1:40568\r\n\r\n");
        denied.response(b"HTTP/1.1 403 ");
        output(b"PASS LEGACY_DENIED payload=0\r\n");
    }
    // SAFETY: all sockets have closed and the successful WSAStartup needs one cleanup.
    unsafe {
        WSACleanup();
    }
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // SAFETY: ExitProcess has no pointer arguments and does not return.
    unsafe { ExitProcess(99) }
}
#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    #[cfg(not(xp_matrix))]
    run();
    #[cfg(xp_matrix)]
    xp_matrix::run();
    // SAFETY: ExitProcess has no pointer arguments and does not return.
    unsafe { ExitProcess(0) }
}
