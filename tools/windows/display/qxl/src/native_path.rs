// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Native cosmetic path enumeration from display/res.c.

use crate::cosmetic::{CosmeticStyle, LineStyle};
use crate::engine::{Memory, PATHOBJ_bEnum, PATHOBJ_vEnumStart, PATHOBJ_vGetBounds};
use crate::gdi::{Point, Rect};
use core::ffi::c_void;
use core::ptr::addr_of;

#[repr(C)]
#[derive(Debug)]
pub(crate) struct PathData {
    flags: u32,
    count: u32,
    points: *const Point,
}

#[repr(C)]
#[derive(Debug)]
struct Attributes {
    flags: u32,
    join: u32,
    cap: u32,
    width: u32,
    miter: u32,
    count: u32,
    style: *const i32,
    state: u32,
}

#[derive(Debug)]
pub(crate) struct Captured {
    memory: Memory,
    length: usize,
}

impl Captured {
    pub(crate) fn segments(&self) -> Option<crate::path::Segments<'_>> {
        // SAFETY: the owner retains all initialised, immutable path bytes.
        let bytes =
            unsafe { core::slice::from_raw_parts(self.memory.as_ptr().as_ptr(), self.length) };
        crate::path::Segments::new(bytes)
    }

    /// # Safety
    /// Retain the complete GDI path and all enumerated points at PASSIVE_LEVEL.
    pub(crate) unsafe fn capture(path: *mut c_void) -> Option<Self> {
        if path.is_null() {
            return None;
        }
        let mut length = 0usize;
        // SAFETY: the caller supplies the retained path enumeration.
        unsafe {
            walk(path, |_, points| {
                length = length.checked_add(8 + points.len() * 8)?;
                (length <= 1 << 20).then_some(())
            })
        }?;
        // SAFETY: the caller supplies the passive-level GDI allocation context.
        let memory = unsafe { Memory::zeroed(u32::try_from(length.max(1)).ok()?) }.ok()?;
        // SAFETY: this guard owns the complete fresh zeroed allocation.
        let output = unsafe { core::slice::from_raw_parts_mut(memory.as_ptr().as_ptr(), length) };
        let mut offset = 0usize;
        // SAFETY: GDI retains the same path while enumeration is restarted.
        unsafe {
            walk(path, |flags, points| {
                let end = offset.checked_add(8 + points.len() * 8)?;
                let bytes = output.get_mut(offset..end)?;
                bytes[..4].copy_from_slice(&flags.to_le_bytes());
                bytes[4..8].copy_from_slice(&(points.len() as u32).to_le_bytes());
                for (bytes, point) in bytes[8..].as_chunks_mut::<8>().0.iter_mut().zip(points) {
                    bytes[..4].copy_from_slice(&point.x.to_le_bytes());
                    bytes[4..].copy_from_slice(&point.y.to_le_bytes());
                }
                offset = end;
                Some(())
            })
        }?;
        (offset == length).then_some(Self { memory, length })
    }
}

/// # Safety
/// The caller retains the native PATHOBJ and writable output rectangle.
pub(crate) unsafe fn bounds(path: *mut c_void) -> Option<Rect> {
    if path.is_null() {
        return None;
    }
    let mut bounds = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: the output owns its complete fixed-point RECTFX extent.
    unsafe { PATHOBJ_vGetBounds(path, &mut bounds) };
    Some(Rect {
        left: bounds.left >> 4,
        top: bounds.top >> 4,
        right: i32::try_from((i64::from(bounds.right) + 15) >> 4).ok()?,
        bottom: i32::try_from((i64::from(bounds.bottom) + 15) >> 4).ok()?,
    })
}

/// # Safety
/// Retain the LINEATTRS and its cosmetic style array through this call.
pub(crate) unsafe fn style(attributes: *mut c_void) -> Option<LineStyle> {
    if attributes.is_null() {
        return None;
    }
    let attributes = attributes.cast::<Attributes>();
    // SAFETY: the complete initialised native LINEATTRS remains live.
    let flags = unsafe { addr_of!((*attributes).flags).read() };
    if flags & 1 != 0 {
        return None;
    }
    let start_gap = flags & 4 != 0;
    let style = if flags & 2 != 0 {
        CosmeticStyle::Alternate { start_gap }
    } else if flags & 8 != 0 {
        // SAFETY: GDI retains the complete descriptor and count style entries.
        let (count, pointer) = unsafe {
            (
                addr_of!((*attributes).count).read(),
                addr_of!((*attributes).style).read(),
            )
        };
        if count == 0 || count > 100 || pointer.is_null() {
            return None;
        }
        // SAFETY: this cosmetic style array holds count live LONG values.
        let lengths = unsafe { core::slice::from_raw_parts(pointer, count as usize) };
        CosmeticStyle::Custom { lengths, start_gap }
    } else {
        CosmeticStyle::Solid
    };
    LineStyle::new(style, 3).ok()
}

unsafe fn walk(path: *mut c_void, mut emit: impl FnMut(u32, &[Point]) -> Option<()>) -> Option<()> {
    let mut total = 0u32;
    // SAFETY: the caller retains the native PATHOBJ throughout enumeration.
    unsafe { PATHOBJ_vEnumStart(path) };
    loop {
        let mut data = PathData {
            flags: 0,
            count: 0,
            points: core::ptr::null(),
        };
        // SAFETY: the complete PATHDATA output is writable and owned locally.
        let more = unsafe { PATHOBJ_bEnum(path, &mut data) };
        if more == -1 || data.flags & !15 != 0 {
            return None;
        }
        if data.count == 0 {
            return (more == 0).then_some(());
        }
        total = total.checked_add(data.count)?;
        if total > 65536 || data.points.is_null() {
            return None;
        }
        // SAFETY: the enumeration supplies count live POINTFIX records.
        let points = unsafe { core::slice::from_raw_parts(data.points, data.count as usize) };
        emit(data.flags, points)?;
        if more == 0 {
            return Some(());
        }
    }
}

const _: () = {
    assert!(core::mem::offset_of!(Attributes, style) == 24);
    assert!(core::mem::offset_of!(Attributes, state) == 24 + core::mem::size_of::<usize>());
    assert!(core::mem::offset_of!(PathData, points) == 8);
};
