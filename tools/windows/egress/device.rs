// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use core::ffi::c_void;

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        attributes: u32,
        template: usize,
    ) -> usize;
    pub fn DeviceIoControl(
        handle: usize,
        code: u32,
        input: *const c_void,
        input_len: u32,
        output: *mut c_void,
        output_len: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    pub fn CloseHandle(handle: usize) -> i32;
}
