// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! GDI raster-glyph enumeration from display/res.c.

use crate::engine::{Memory, STROBJ_bEnum, STROBJ_vEnumStart};
use crate::gdi::{Point, Rect, Size};
use core::ffi::c_void;
use core::ptr::{addr_of, null_mut};

#[repr(C)]
#[derive(Debug)]
pub(crate) struct GlyphPosition {
    handle: u32,
    definition: *const *const GlyphBits,
    position: Point,
}

#[repr(C)]
#[derive(Debug)]
struct GlyphBits {
    origin: Point,
    size: Size,
}

#[repr(C)]
#[derive(Debug)]
pub(crate) struct StringObject {
    count: u32,
    flags: u32,
    increment: u32,
    pub(crate) background: Rect,
    positions: *mut GlyphPosition,
    original: *const u16,
}

#[derive(Debug)]
pub(crate) struct Captured {
    memory: Memory,
    length: usize,
    count: u16,
    bits: u8,
}

impl Captured {
    pub(crate) fn glyphs(&self) -> Option<crate::text::Glyphs<'_>> {
        // SAFETY: the owner retains all initialised captured bytes immutably.
        let bytes =
            unsafe { core::slice::from_raw_parts(self.memory.as_ptr().as_ptr(), self.length) };
        crate::text::Glyphs::new(bytes, self.count, self.bits)
    }

    /// # Safety
    /// GDI must retain this STROBJ, FONTOBJ, enumerated glyph definitions and
    /// bitmap bytes through the synchronous passive-level capture.
    pub(crate) unsafe fn capture(text: *mut c_void, font: *mut c_void) -> Option<Self> {
        if text.is_null() || font.is_null() {
            return None;
        }
        // SAFETY: flFontType is the fourth ULONG of the retained FONTOBJ.
        let flags = unsafe { font.cast::<u32>().add(3).read() };
        if flags & 1 == 0 {
            return None;
        }
        let bits = if flags & 0x10000 != 0 { 4 } else { 1 };
        let mut length = 0usize;
        // SAFETY: the caller supplies the retained native glyph enumeration.
        let count = unsafe {
            walk(text.cast(), bits, |_, _, _, _, pixels| {
                length = length.checked_add(20 + pixels.len())?;
                (length <= 32 << 20).then_some(())
            })
        }?;
        // SAFETY: capture runs in the GDI passive-level allocation context.
        let memory = unsafe { Memory::zeroed(u32::try_from(length.max(1)).ok()?) }.ok()?;
        let mut offset = 0usize;
        // SAFETY: this guard owns the entire fresh, zeroed output allocation.
        let output = unsafe { core::slice::from_raw_parts_mut(memory.as_ptr().as_ptr(), length) };
        // SAFETY: enumeration is restarted while all GDI glyph records remain live.
        let captured = unsafe {
            walk(
                text.cast(),
                bits,
                |position, origin, width, height, pixels| {
                    let end = offset.checked_add(20 + pixels.len())?;
                    let bytes = output.get_mut(offset..end)?;
                    for (index, value) in [position.x, position.y, origin.x, origin.y]
                        .into_iter()
                        .enumerate()
                    {
                        bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
                    }
                    bytes[16..18].copy_from_slice(&width.to_le_bytes());
                    bytes[18..20].copy_from_slice(&height.to_le_bytes());
                    bytes[20..].copy_from_slice(pixels);
                    offset = end;
                    Some(())
                },
            )
        }?;
        if captured != count || offset != length {
            return None;
        }
        Some(Self {
            memory,
            length,
            count,
            bits,
        })
    }
}

unsafe fn walk(
    text: *mut StringObject,
    bits: u8,
    mut emit: impl FnMut(Point, Point, u16, u16, &[u8]) -> Option<()>,
) -> Option<u16> {
    // SAFETY: the caller retains the complete STROBJ and all its glyph records.
    let (expected, flags, increment, direct) = unsafe {
        (
            addr_of!((*text).count).read(),
            addr_of!((*text).flags).read(),
            addr_of!((*text).increment).read(),
            addr_of!((*text).positions).read(),
        )
    };
    if expected > 4096 {
        return None;
    }
    let step = i32::try_from(increment).ok()?;
    let step = if flags & 8 != 0 { -step } else { step };
    let delta = if flags & 4 != 0 {
        Point { x: 0, y: step }
    } else {
        Point { x: step, y: 0 }
    };
    let mut previous: Option<Point> = None;
    let mut total = 0u32;
    // SAFETY: the live STROBJ permits repeated enumeration in this callback.
    unsafe { STROBJ_vEnumStart(text.cast()) };
    loop {
        let (count, positions, more) = if !direct.is_null() {
            (expected, direct, false)
        } else {
            let mut count = 0;
            let mut positions = null_mut();
            // SAFETY: local outputs are writable and GDI owns the enumerated array.
            let more = unsafe { STROBJ_bEnum(text.cast(), &mut count, &mut positions) };
            if more == -1 || (more != 0 && count == 0) {
                return None;
            }
            (count, positions, more != 0)
        };
        total = total.checked_add(count)?;
        if total > expected || (count != 0 && positions.is_null()) {
            return None;
        }
        for index in 0..count {
            // SAFETY: GDI retains count complete GLYPHPOS entries in this batch.
            let (definition, supplied) = unsafe {
                let position = positions.add(index as usize);
                (
                    addr_of!((*position).definition).read(),
                    addr_of!((*position).position).read(),
                )
            };
            if definition.is_null() {
                return None;
            }
            // SAFETY: the glyph's live GLYPHDEF contains its GLYPHBITS pointer.
            let bitmap = unsafe { definition.read() };
            if bitmap.is_null() {
                return None;
            }
            // SAFETY: GDI retains the complete raster glyph header and bit array.
            let (origin, size) = unsafe {
                (
                    addr_of!((*bitmap).origin).read(),
                    addr_of!((*bitmap).size).read(),
                )
            };
            let width = u16::try_from(size.width).ok().filter(|v| *v <= 4096)?;
            let height = u16::try_from(size.height).ok().filter(|v| *v <= 4096)?;
            let length = (usize::from(width) * usize::from(bits)).div_ceil(8) * usize::from(height);
            // SAFETY: GLYPHBITS.aj follows its 16-byte header and has tightly
            // packed top-down rows for the FONTOBJ-selected raster depth.
            let pixels =
                unsafe { core::slice::from_raw_parts(bitmap.cast::<u8>().add(16), length) };
            let position = match (increment, previous) {
                (0, _) | (_, None) => supplied,
                (_, Some(previous)) => Point {
                    x: previous.x.checked_add(delta.x)?,
                    y: previous.y.checked_add(delta.y)?,
                },
            };
            emit(position, origin, width, height, pixels)?;
            previous = Some(position);
        }
        if !more {
            break;
        }
    }
    (total == expected).then_some(total as u16)
}

const _: () = {
    assert!(core::mem::size_of::<GlyphBits>() == 16);
    assert!(core::mem::offset_of!(StringObject, background) == 12);
    assert!(
        core::mem::offset_of!(StringObject, positions)
            == if cfg!(target_pointer_width = "32") {
                28
            } else {
                32
            }
    );
    assert!(
        core::mem::size_of::<GlyphPosition>()
            == if cfg!(target_pointer_width = "32") {
                16
            } else {
                24
            }
    );
};
