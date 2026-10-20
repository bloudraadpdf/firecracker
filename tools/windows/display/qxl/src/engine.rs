// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Kernel GDI allocation and miniport mode enumeration.

use core::ffi::c_void;
use core::ptr::{NonNull, null_mut, read_unaligned};

use crate::display_modes::{self, DEVMODE_BYTES};
use crate::gdi::{Size, SurfaceObject};
use crate::ioctl::{QUERY_AVAILABLE_MODES, QUERY_MODE_COUNT};
use crate::modes::VideoModeInformation;

const INVALID_DATA: u32 = 13;
const NOT_ENOUGH_MEMORY: u32 = 8;
const ALLOCATION_TAG: u32 = u32::from_le_bytes(*b"qxld");

#[cfg_attr(
    target_arch = "x86",
    link(
        name = "win32k.sys",
        kind = "raw-dylib",
        modifiers = "+verbatim",
        import_name_type = "undecorated"
    )
)]
#[cfg_attr(
    target_arch = "x86_64",
    link(name = "win32k.sys", kind = "raw-dylib", modifiers = "+verbatim")
)]
unsafe extern "system" {
    pub(crate) fn EngCreatePalette(
        mode: u32,
        count: u32,
        colors: *mut u32,
        red_mask: u32,
        green_mask: u32,
        blue_mask: u32,
    ) -> *mut c_void;
    pub(crate) fn EngDeletePalette(palette: *mut c_void) -> i32;
    pub(crate) fn EngCreateBitmap(
        size: Size,
        stride: i32,
        format: u32,
        flags: u32,
        pixels: *mut c_void,
    ) -> *mut c_void;
    pub(crate) fn EngCreateDeviceBitmap(
        context: *mut c_void,
        size: Size,
        format: u32,
    ) -> *mut c_void;
    pub(crate) fn EngAssociateSurface(surface: *mut c_void, device: *mut c_void, hooks: u32)
    -> i32;
    pub(crate) fn EngLockSurface(surface: *mut c_void) -> *mut SurfaceObject;
    pub(crate) fn EngUnlockSurface(object: *mut SurfaceObject);
    pub(crate) fn EngDeleteSurface(surface: *mut c_void) -> i32;
    fn EngAllocMem(flags: u32, bytes: u32, tag: u32) -> *mut c_void;
    fn EngFreeMem(memory: *mut c_void);
    pub(crate) fn EngDeviceIoControl(
        device: *mut c_void,
        code: u32,
        input: *mut c_void,
        input_bytes: u32,
        output: *mut c_void,
        output_bytes: u32,
        returned: *mut u32,
    ) -> u32;
}

#[derive(Debug)]
struct Memory(NonNull<u8>);

impl Memory {
    unsafe fn zeroed(bytes: u32) -> Result<Self, u32> {
        // SAFETY: the caller is a passive-level kernel display callback.
        let memory = unsafe { EngAllocMem(1, bytes, ALLOCATION_TAG) };
        Ok(Self(NonNull::new(memory.cast()).ok_or(NOT_ENOUGH_MEMORY)?))
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        // SAFETY: this owner retains the unique EngAllocMem allocation and all
        // mode-table reads end before its destruction at PASSIVE_LEVEL.
        unsafe { EngFreeMem(self.0.as_ptr().cast()) };
    }
}

#[derive(Debug)]
pub struct Modes {
    memory: Memory,
    count: u32,
    stride: u32,
}

impl Modes {
    /// # Safety
    /// Call in a serialised GDI callback at PASSIVE_LEVEL with its live miniport
    /// handle. Keep all access and destruction at PASSIVE_LEVEL.
    pub unsafe fn query(driver: NonNull<c_void>) -> Result<Self, u32> {
        let mut count = [0u32; 2];
        let mut returned = 0;
        // SAFETY: the handle is live and the aligned output is eight bytes.
        let status = unsafe {
            EngDeviceIoControl(
                driver.as_ptr(),
                QUERY_MODE_COUNT,
                null_mut(),
                0,
                count.as_mut_ptr().cast(),
                8,
                &mut returned,
            )
        };
        if status != 0 {
            return Err(status);
        }
        if returned != 8 || count[0] == 0 || count[1] < 80 {
            return Err(INVALID_DATA);
        }
        let bytes = count[0]
            .checked_mul(count[1])
            .filter(|bytes| (*bytes as usize) <= isize::MAX as usize)
            .ok_or(INVALID_DATA)?;
        // SAFETY: this callback runs at PASSIVE_LEVEL; the size is nonzero.
        let memory = unsafe { Memory::zeroed(bytes)? };
        // SAFETY: memory owns the full output extent; no references into it
        // exist while the synchronous miniport operation writes the table.
        let status = unsafe {
            EngDeviceIoControl(
                driver.as_ptr(),
                QUERY_AVAILABLE_MODES,
                null_mut(),
                0,
                memory.0.as_ptr().cast(),
                bytes,
                &mut returned,
            )
        };
        if status != 0 {
            return Err(status);
        }
        if returned != bytes {
            return Err(INVALID_DATA);
        }
        Ok(Self {
            memory,
            count: count[0],
            stride: count[1],
        })
    }

    pub fn entries(&self) -> impl Iterator<Item = VideoModeInformation> + '_ {
        (0..self.count).filter_map(|index| {
            // SAFETY: query validated count*stride against the allocation and
            // stride>=80. The immutable synchronous result has integer fields
            // only. Extended or oddly aligned records need no aligned borrow.
            let mode: VideoModeInformation = unsafe {
                read_unaligned(
                    self.memory
                        .0
                        .as_ptr()
                        .add((index * self.stride) as usize)
                        .cast(),
                )
            };
            (mode.length <= self.stride && display_modes::supported(&mode)).then_some(mode)
        })
    }
}

/// The complete DrvGetModes callback; registration belongs to the display DLL.
/// # Safety
/// GDI must supply a live miniport handle at PASSIVE_LEVEL. A non-null output
/// must be writable for output_bytes and must not alias callback state.
pub unsafe extern "system" fn get_modes(
    driver: *mut c_void,
    output_bytes: u32,
    output: *mut u8,
) -> u32 {
    let Some(driver) = NonNull::new(driver) else {
        return 0;
    };
    // SAFETY: this callback inherits GDI's live handle and passive-level context.
    let Ok(modes) = (unsafe { Modes::query(driver) }) else {
        return 0;
    };
    let Some(bytes) = modes
        .entries()
        .count()
        .checked_mul(DEVMODE_BYTES)
        .filter(|bytes| *bytes <= isize::MAX as usize)
        .and_then(|bytes| u32::try_from(bytes).ok())
    else {
        return 0;
    };
    if output.is_null() {
        return bytes;
    }
    if output_bytes < bytes {
        return 0;
    }
    // SAFETY: the caller supplies the complete writable output; no reference
    // exists to this region during the preceding synchronous miniport calls.
    let output = unsafe { core::slice::from_raw_parts_mut(output, bytes as usize) };
    for (mode, record) in modes
        .entries()
        .zip(output.as_chunks_mut::<DEVMODE_BYTES>().0)
    {
        // entries filters unsupported modes and the record has the exact size.
        if display_modes::encode(&mode, record).is_none() {
            return 0;
        }
    }
    bytes
}
