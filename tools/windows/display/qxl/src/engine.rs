// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Kernel GDI allocation and miniport mode enumeration.

use core::ffi::c_void;
use core::ptr::{NonNull, null_mut, read_unaligned};

use crate::display_modes::{self, DEVMODE_BYTES};
use crate::gdi::{Point, Rect, Size, SurfaceObject};
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
    pub(crate) fn EngCreateSemaphore() -> *mut c_void;
    pub(crate) fn EngAcquireSemaphore(semaphore: *mut c_void);
    pub(crate) fn EngReleaseSemaphore(semaphore: *mut c_void);
    pub(crate) fn EngDeleteSemaphore(semaphore: *mut c_void);
    pub(crate) fn EngClearEvent(event: *mut c_void);
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
    pub(crate) fn EngTextOut(
        destination: *mut SurfaceObject,
        text: *mut c_void,
        font: *mut c_void,
        clip: *mut c_void,
        extra: *mut Rect,
        opaque_rect: *mut Rect,
        foreground: *mut c_void,
        background: *mut c_void,
        brush_origin: *mut Point,
        mix: u32,
    ) -> i32;
    pub(crate) fn EngStrokePath(
        destination: *mut SurfaceObject,
        path: *mut c_void,
        clip: *mut c_void,
        transform: *mut c_void,
        brush: *mut c_void,
        brush_origin: *mut Point,
        attributes: *mut c_void,
        mix: u32,
    ) -> i32;
    pub(crate) fn EngStretchBlt(
        destination: *mut SurfaceObject,
        source: *mut SurfaceObject,
        mask: *mut SurfaceObject,
        clip: *mut c_void,
        translation: *mut c_void,
        color_adjustment: *mut c_void,
        halftone_origin: *mut Point,
        destination_rect: *mut Rect,
        source_rect: *mut Rect,
        mask_origin: *mut Point,
        mode: u32,
    ) -> i32;
    pub(crate) fn EngStretchBltROP(
        destination: *mut SurfaceObject,
        source: *mut SurfaceObject,
        mask: *mut SurfaceObject,
        clip: *mut c_void,
        translation: *mut c_void,
        color_adjustment: *mut c_void,
        brush_origin: *mut Point,
        destination_rect: *mut Rect,
        source_rect: *mut Rect,
        mask_origin: *mut Point,
        mode: u32,
        brush: *mut c_void,
        rop: u32,
    ) -> i32;
    pub(crate) fn EngAlphaBlend(
        destination: *mut SurfaceObject,
        source: *mut SurfaceObject,
        clip: *mut c_void,
        translation: *mut c_void,
        destination_rect: *mut Rect,
        source_rect: *mut Rect,
        blend: *mut c_void,
    ) -> i32;
    pub(crate) fn EngCopyBits(
        destination: *mut SurfaceObject,
        source: *mut SurfaceObject,
        clip: *mut c_void,
        translation: *mut c_void,
        destination_rect: *mut crate::gdi::Rect,
        source_origin: *mut crate::gdi::Point,
    ) -> i32;
    pub(crate) fn EngBitBlt(
        destination: *mut SurfaceObject,
        source: *mut SurfaceObject,
        mask: *mut SurfaceObject,
        clip: *mut c_void,
        translation: *mut c_void,
        destination_rect: *mut crate::gdi::Rect,
        source_origin: *mut crate::gdi::Point,
        mask_origin: *mut crate::gdi::Point,
        brush: *mut c_void,
        brush_origin: *mut crate::gdi::Point,
        rop: u32,
    ) -> i32;
    pub(crate) fn EngModifySurface(
        surface: *mut c_void,
        device: *mut c_void,
        hooks: u32,
        flags: u32,
        context: *mut c_void,
        first_scan: *mut c_void,
        stride: i32,
        reserved: *mut c_void,
    ) -> i32;
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
pub(crate) struct Memory(NonNull<u8>);

impl Memory {
    pub(crate) unsafe fn zeroed(bytes: u32) -> Result<Self, u32> {
        // SAFETY: the caller is a passive-level kernel display callback.
        let memory = unsafe { EngAllocMem(1, bytes, ALLOCATION_TAG) };
        Ok(Self(NonNull::new(memory.cast()).ok_or(NOT_ENOUGH_MEMORY)?))
    }

    pub(crate) fn as_ptr(&self) -> NonNull<u8> {
        self.0
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        // SAFETY: this owner retains the unique EngAllocMem allocation and all
        // mode-table reads end before its destruction at PASSIVE_LEVEL.
        unsafe { EngFreeMem(self.0.as_ptr().cast()) };
    }
}

/// # Safety
/// Construct and destroy the object at PASSIVE_LEVEL in GDI callback context.
pub(crate) unsafe fn allocate_object<T>(value: T) -> Result<crate::kernel_object::Object<T>, u32> {
    let bytes = crate::kernel_object::Object::<T>::allocation_bytes()
        .and_then(|bytes| u32::try_from(bytes).ok())
        .ok_or(INVALID_DATA)?;
    // SAFETY: the caller supplies the passive-level GDI heap context.
    let memory = unsafe { Memory::zeroed(bytes) }?;
    // SAFETY: this guard owns the full fresh extent; EngFreeMem reclaims its
    // original pointer, after T's destructor, in the retained GDI context.
    let object = unsafe {
        crate::kernel_object::Object::new_in(memory.0, bytes as usize, EngFreeMem, value)
    }
    .map_err(|_| INVALID_DATA)?;
    // Ownership transfers only after successful initialisation; nothing can
    // fail between that transfer and disarming the original allocation guard.
    core::mem::forget(memory);
    Ok(object)
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
