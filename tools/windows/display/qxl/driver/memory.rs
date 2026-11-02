// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Compiler memory-operation ABI without a C runtime.

use core::ffi::c_void;
use core::ptr::write_volatile;

use firecracker_qxl_driver::videoport::{VideoPortMoveMemory, VideoPortZeroMemory};

/// # Safety
/// The ranges must be valid for length bytes and must not overlap. Their
/// residency must satisfy the current IRQL's VideoPort memory requirements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(
    destination: *mut c_void,
    source: *const c_void,
    length: usize,
) -> *mut c_void {
    let mut offset = 0;
    while offset < length {
        let bytes = (length - offset).min(u32::MAX as usize) as u32;
        // SAFETY: both complete ranges are valid; each chunk stays inside them.
        unsafe {
            VideoPortMoveMemory(
                destination.cast::<u8>().add(offset).cast(),
                source.cast::<u8>().add(offset).cast(),
                bytes,
            )
        };
        offset += bytes as usize;
    }
    destination
}

/// # Safety
/// Destination must be writable for length bytes and resident at elevated IRQL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memset(
    destination: *mut c_void,
    value: i32,
    length: usize,
) -> *mut c_void {
    let byte = value as u8;
    let mut offset = 0;
    if byte == 0 {
        while offset < length {
            let bytes = (length - offset).min(u32::MAX as usize) as u32;
            // SAFETY: each chunk is within the complete writable destination.
            unsafe { VideoPortZeroMemory(destination.cast::<u8>().add(offset).cast(), bytes) };
            offset += bytes as usize;
        }
    } else {
        while offset < length {
            // SAFETY: each byte is within the writable range. Volatile stores
            // prevent LLVM from replacing this implementation with itself.
            unsafe { write_volatile(destination.cast::<u8>().add(offset), byte) };
            offset += 1;
        }
    }
    destination
}
