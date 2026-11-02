// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Defined pixel bytes from native GDI bitmaps, without row padding.

use crate::cursor::{Bitmap, Format};
use crate::engine::Memory;
use crate::gdi::{Rect, SurfaceObject};
use core::ffi::c_void;
use core::ptr::{addr_of, copy_nonoverlapping};

#[derive(Debug)]
pub(crate) struct Pixels {
    memory: Memory,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) stride: u32,
    pub(crate) format: Format,
    colors: Option<ColorTable>,
}

impl Pixels {
    pub(crate) unsafe fn capture(surface: *mut SurfaceObject, alpha: bool) -> Option<Self> {
        // SAFETY: the pointer callback retains the complete source bitmap.
        unsafe { Self::capture_inner(surface, alpha, None) }
    }

    pub(crate) unsafe fn capture_rect(
        surface: *mut SurfaceObject,
        alpha: bool,
        area: Rect,
    ) -> Option<Self> {
        // SAFETY: the drawing callback retains the bitmap and requested area.
        unsafe { Self::capture_inner(surface, alpha, Some(area)) }
    }

    unsafe fn capture_inner(
        surface: *mut SurfaceObject,
        alpha: bool,
        area: Option<Rect>,
    ) -> Option<Self> {
        if surface.is_null() {
            return None;
        }
        // SAFETY: GDI supplies a live readable standard bitmap for this callback.
        let (size, stride, first, format, kind) = unsafe {
            (
                addr_of!((*surface).size).read(),
                addr_of!((*surface).stride).read(),
                addr_of!((*surface).first_scan).read(),
                addr_of!((*surface).bitmap_format).read(),
                addr_of!((*surface).kind).read(),
            )
        };
        if kind != 0 || first.is_null() {
            return None;
        }
        let area = area.unwrap_or(Rect {
            left: 0,
            top: 0,
            right: size.width,
            bottom: size.height,
        });
        if area.left < 0
            || area.top < 0
            || area.left >= area.right
            || area.top >= area.bottom
            || area.right > size.width
            || area.bottom > size.height
        {
            return None;
        }
        let width = u16::try_from(area.right - area.left).ok()?;
        let height = u16::try_from(area.bottom - area.top).ok()?;
        let (format, bits) = match format {
            1 => (Format::Mono, 1),
            2 => (Format::Indexed4, 4),
            3 => (Format::Indexed8, 8),
            4 => (Format::Rgb555, 16),
            5 => (Format::Bgr24, 24),
            6 => (Format::Bgrx32, 32),
            _ => return None,
        };
        let row = (u32::from(width) * bits).div_ceil(8);
        let source_row = u32::try_from(size.width)
            .ok()?
            .checked_mul(bits)?
            .div_ceil(8);
        if stride.unsigned_abs() < source_row {
            return None;
        }
        let bytes = row
            .checked_mul(u32::from(height))
            .filter(|value| *value as u64 <= isize::MAX as u64)?;
        // SAFETY: capture runs at PASSIVE_LEVEL in a live GDI pointer callback.
        let memory = unsafe { Memory::zeroed(bytes) }.ok()?;
        for y in 0..height {
            let offset = isize::try_from(
                (i64::from(area.top) + i64::from(y)) * i64::from(stride)
                    + i64::from(area.left) * i64::from(bits) / 8,
            )
            .ok()?;
            let source = first.cast::<u8>().wrapping_offset(offset);
            // SAFETY: the destination allocation contains every tightly packed row.
            let destination =
                unsafe { memory.as_ptr().as_ptr().add(usize::from(y) * row as usize) };
            if bits < 8 {
                let skip = (area.left as u32 % (8 / bits)) as usize;
                let source_bytes = ((skip + usize::from(width)) * bits as usize).div_ceil(8);
                let packed = if bits == 1 {
                    crate::bitmap::Format::Indexed1
                } else {
                    crate::bitmap::Format::Indexed4
                };
                // SAFETY: the retained crop spans these defined source bytes;
                // the fresh allocation owns the entire disjoint output row.
                let (input, output) = unsafe {
                    (
                        core::slice::from_raw_parts(source, source_bytes),
                        core::slice::from_raw_parts_mut(destination, row as usize),
                    )
                };
                crate::bitmap::pack_indexed(packed, input, skip, usize::from(width), output)?;
            } else if format == Format::Bgrx32 && !alpha {
                for x in 0..usize::from(width) {
                    // SAFETY: colour cursor X bytes need not be initialised. Copy
                    // only the three defined components into zeroed CPU storage.
                    unsafe { copy_nonoverlapping(source.add(x * 4), destination.add(x * 4), 3) };
                }
            } else {
                // SAFETY: GDI retains the defined row bytes, including alpha when
                // requested. Neither source padding nor adjacent rows are copied.
                unsafe { copy_nonoverlapping(source, destination, row as usize) };
            }
        }
        Some(Self {
            memory,
            width,
            height,
            stride: row,
            format,
            colors: None,
        })
    }

    pub(crate) unsafe fn capture_image(
        surface: *mut SurfaceObject,
        translation: *mut c_void,
        area: Rect,
        alpha: bool,
    ) -> Option<Self> {
        // SAFETY: GDI retains both complete surface and translation records.
        let mut pixels = unsafe { Self::capture_rect(surface, alpha, area) }?;
        if matches!(
            pixels.format,
            Format::Mono | Format::Indexed4 | Format::Indexed8
        ) {
            if alpha {
                return None;
            }
            let maximum = match pixels.format {
                Format::Mono => 2,
                Format::Indexed4 => 16,
                _ => 256,
            };
            // SAFETY: capture completes every GDI call before the hardware lease.
            pixels.colors = Some(unsafe { ColorTable::capture(translation, maximum) }?);
        } else if !translation.is_null() {
            // SAFETY: GDI retains the complete XLATEOBJ flag word.
            if unsafe { translation.cast::<u32>().add(1).read() } & 1 == 0 {
                return None;
            }
        }
        Some(pixels)
    }

    pub(crate) fn indexed(
        &self,
    ) -> Option<(crate::bitmap::Bitmap<'_>, crate::palette::Colors<'_>)> {
        let colors = self.colors.as_ref()?;
        let format = match self.format {
            Format::Mono => crate::bitmap::Format::Indexed1,
            Format::Indexed4 => crate::bitmap::Format::Indexed4,
            Format::Indexed8 => crate::bitmap::Format::Indexed8,
            _ => return None,
        };
        Some((
            crate::bitmap::Bitmap::new(
                u32::from(self.width),
                u32::from(self.height),
                format,
                self.bytes(),
            )?,
            colors.colors()?,
        ))
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        let bytes = self.stride as usize * usize::from(self.height);
        // SAFETY: capture initialises this complete, tightly packed CPU allocation.
        unsafe { core::slice::from_raw_parts(self.memory.as_ptr().as_ptr(), bytes) }
    }

    pub(crate) fn bitmap(&self) -> Option<Bitmap<'_>> {
        let pixels = self.bytes();
        Bitmap::new(
            pixels,
            self.width,
            self.height,
            0,
            self.stride as i32,
            self.format,
        )
        .ok()
    }
}

#[derive(Debug)]
struct ColorTable {
    memory: Memory,
    count: u32,
    unique: u32,
}

impl ColorTable {
    unsafe fn capture(translation: *mut c_void, maximum: u32) -> Option<Self> {
        if translation.is_null() {
            return None;
        }
        let object = translation.cast::<crate::gdi::Translation>();
        // SAFETY: GDI retains the complete documented translation record.
        let flags = unsafe { addr_of!((*object).flags).read() };
        if flags & 2 == 0 || flags & !(1 | 2 | 0x20) != 0 {
            return None;
        }
        // SAFETY: GDI supplies the indexed translation, before taking any hardware lease.
        let table = unsafe { crate::engine::XLATEOBJ_piVector(translation) };
        // SAFETY: the vector lookup may initialise the retained record's table count.
        let (count, unique) = unsafe {
            (
                addr_of!((*object).count).read(),
                addr_of!((*object).unique).read(),
            )
        };
        if table.is_null() || count == 0 || count > maximum {
            return None;
        }
        // SAFETY: capture runs in the passive-level GDI allocation context.
        let memory = unsafe { Memory::zeroed(count * 4) }.ok()?;
        for index in 0..count as usize {
            // SAFETY: GDI retains count readable vector entries; the fresh aligned
            // allocation owns count outputs. The destination is 32-bit RGB.
            unsafe {
                memory
                    .as_ptr()
                    .as_ptr()
                    .cast::<u32>()
                    .add(index)
                    .write(table.add(index).read() & 0x00ff_ffff);
            }
        }
        Some(Self {
            memory,
            count,
            unique,
        })
    }

    fn colors(&self) -> Option<crate::palette::Colors<'_>> {
        // SAFETY: capture initialises every aligned colour word in this allocation.
        let colors = unsafe {
            core::slice::from_raw_parts(
                self.memory.as_ptr().as_ptr().cast::<u32>(),
                self.count as usize,
            )
        };
        crate::palette::Colors::new(self.unique, colors)
    }
}
