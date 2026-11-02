// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Native source, pattern and mask capture from display/rop.c.

use crate::gdi::{Point, Rect, SurfaceObject};
use crate::native_pixels::Pixels;
use crate::paint::Paint;
use crate::raster_draw::{Bitmap, Mask, Raster};
use crate::rop::Rop3;
use core::ffi::c_void;
use core::ptr::addr_of;

#[derive(Debug)]
pub(crate) struct Inputs {
    pub(crate) destination: *mut SurfaceObject,
    pub(crate) source: *mut SurfaceObject,
    pub(crate) mask: *mut SurfaceObject,
    pub(crate) clip: *mut c_void,
    pub(crate) translation: *mut c_void,
    pub(crate) destination_rect: *mut Rect,
    pub(crate) source_rect: Option<Rect>,
    pub(crate) mask_origin: *mut Point,
    pub(crate) brush: *mut c_void,
    pub(crate) brush_origin: *mut Point,
    pub(crate) rop: u32,
}

impl Inputs {
    /// # Safety
    /// Retain all required GDI operands, clip, translation, brush and geometry
    /// records at PASSIVE_LEVEL through this synchronous capture/publication.
    pub(crate) unsafe fn submit(self) -> Option<i32> {
        if self.destination.is_null() || self.destination_rect.is_null() {
            return None;
        }
        let foreground = self.rop as u8;
        let background = (self.rop >> 8) as u8;
        let operations = [Rop3::new(foreground), Rop3::new(background)];
        let source_used = operations.iter().any(|operation| operation.inputs().source);
        let brush_used = operations.iter().any(|operation| operation.inputs().brush);
        let masked = foreground != background;
        // SAFETY: GDI retains the complete destination and operation rectangle.
        let (original, size, format) = unsafe {
            (
                self.destination_rect.read(),
                addr_of!((*self.destination).size).read(),
                addr_of!((*self.destination).bitmap_format).read(),
            )
        };
        if format != 6
            || original.left < 0
            || original.top < 0
            || original.right > size.width
            || original.bottom > size.height
        {
            return None;
        }
        let bounds = [original.left, original.top, original.right, original.bottom];
        if bounds[0] >= bounds[2] || bounds[1] >= bounds[3] {
            return None;
        }
        let mut clipped = original;
        if !self.clip.is_null() {
            // SAFETY: GDI retains the complete native CLIPOBJ through this call.
            let clip = unsafe { &*self.clip.cast::<crate::gdi::ClipObject>() };
            match clip.draw_complexity {
                0 => {}
                1 => {
                    clipped.left = clipped.left.max(clip.bounds.left);
                    clipped.top = clipped.top.max(clip.bounds.top);
                    clipped.right = clipped.right.min(clip.bounds.right);
                    clipped.bottom = clipped.bottom.min(clip.bounds.bottom);
                }
                _ => return None,
            }
        }
        if clipped.left >= clipped.right || clipped.top >= clipped.bottom {
            return Some(1);
        }
        let source = if source_used {
            if self.source.is_null() {
                return None;
            }
            // SAFETY: the required source surface metadata remains live.
            if unsafe { addr_of!((*self.source).bitmap_format).read() } != 6 {
                return None;
            }
            if !self.translation.is_null() {
                // SAFETY: flXlate is the second ULONG of the native XLATEOBJ.
                if unsafe { self.translation.cast::<u32>().add(1).read() } & 1 == 0 {
                    return None;
                }
            }
            // SAFETY: GDI retains the source bitmap and complete requested area.
            Some(unsafe { Pixels::capture_rect(self.source, false, self.source_rect?) }?)
        } else {
            None
        };
        let bitmap = match source.as_ref() {
            Some(source) => Some(Bitmap::new(
                u32::from(source.width),
                u32::from(source.height),
                source.bytes(),
            )?),
            None => None,
        };
        let paint = if brush_used {
            // SAFETY: native brush realisation finishes before taking the hardware lease.
            unsafe { crate::brush::paint(self.brush, self.brush_origin) }?
        } else {
            Paint::Solid(0)
        };
        let mask = if masked {
            if self.mask.is_null() || self.mask_origin.is_null() {
                return None;
            }
            // SAFETY: GDI retains the complete mask bitmap and source origin.
            let (pixels, origin) =
                unsafe { (Pixels::capture(self.mask, false)?, self.mask_origin.read()) };
            if pixels.format != crate::cursor::Format::Mono {
                return None;
            }
            Some((pixels, origin))
        } else {
            None
        };
        let make = |truth, inverted| {
            let mask = match mask.as_ref() {
                Some((mask, origin)) => Some(Mask::new(
                    u32::from(mask.width),
                    u32::from(mask.height),
                    mask.bytes(),
                    [origin.x, origin.y],
                    inverted,
                )?),
                None => None,
            };
            Raster::new(bounds, truth, bitmap, paint, mask)?.with_clip([
                clipped.left,
                clipped.top,
                clipped.right,
                clipped.bottom,
            ])
        };
        let first = make(foreground, false)?;
        let second = if masked {
            Some(make(background, true)?)
        } else {
            None
        };
        // SAFETY: all CPU sources remain live until independent retained resources
        // are published. A failure after publication is never replayed through GDI.
        unsafe {
            crate::pdev::primary_operation(self.destination, |hardware, _| {
                if foreground != 0xaa && hardware.raster(first).is_err() {
                    return Some(0);
                }
                if let Some(second) = second
                    && background != 0xaa
                    && hardware.raster(second).is_err()
                {
                    return Some(0);
                }
                Some(1)
            })
        }
        .map(|result| result as i32)
    }
}
