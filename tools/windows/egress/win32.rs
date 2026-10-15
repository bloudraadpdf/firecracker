// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use core::ffi::c_void;

#[cfg_attr(
    target_arch = "x86",
    link(
        name = "kernel32",
        kind = "raw-dylib",
        import_name_type = "undecorated"
    )
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    pub fn GetLastError() -> u32;
    pub fn GetStdHandle(which: u32) -> usize;
    pub fn WriteFile(
        handle: usize,
        buffer: *const u8,
        length: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    pub fn ExitProcess(code: u32) -> !;
}
