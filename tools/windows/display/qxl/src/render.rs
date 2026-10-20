// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Kernel-GDI raster paths from display/rop.c, text.c and driver.c.

use crate::rop::Rop3;

fn operands(rop: u32) -> (bool, bool) {
    let foreground = rop as u8;
    let background = (rop >> 8) as u8;
    (
        Rop3::new(foreground).inputs().source || Rop3::new(background).inputs().source,
        foreground != background,
    )
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
mod native {
    use super::*;
    use crate::engine::{
        EngAlphaBlend, EngBitBlt, EngCopyBits, EngStretchBlt, EngStretchBltROP, EngStrokePath,
        EngTextOut, EngTransparentBlt,
    };
    use crate::gdi::{Point, Rect, SurfaceObject};
    use crate::pdev::with_cpu_surfaces;
    use crate::rop::Rop3;
    use core::ffi::c_void;
    use core::ptr::null_mut;

    unsafe fn draw(
        surfaces: [*mut SurfaceObject; 3],
        operation: impl FnOnce([*mut SurfaceObject; 3]) -> i32,
    ) -> i32 {
        // SAFETY: callers retain every GDI object and record through this
        // synchronous operation. The closure does not retain surface pointers.
        let result = unsafe { with_cpu_surfaces(surfaces, |views| operation(views) != 0) };
        i32::from(matches!(result, Ok(true)))
    }

    unsafe fn solid(
        destination: *mut SurfaceObject,
        clip: *mut c_void,
        rectangle: *mut Rect,
        brush: *mut c_void,
        rop: u32,
    ) -> Option<i32> {
        if destination.is_null() || rectangle.is_null() {
            return None;
        }
        let truth = rop as u8;
        let operation = Rop3::new(truth);
        if rop & 0xffff != u32::from(truth) * 0x101 || operation.inputs().source {
            return None;
        }
        // SAFETY: callback records remain live for this synchronous operation.
        let bounds = unsafe { destination_bounds(destination, clip, rectangle) }?;
        if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
            return Some(1);
        }
        let color = if operation.inputs().brush {
            if brush.is_null() {
                return None;
            }
            // SAFETY: iSolidColor is the initial ULONG of the live BRUSHOBJ.
            let color = unsafe { brush.cast::<u32>().read() };
            if color == u32::MAX {
                return None;
            }
            color
        } else {
            0
        };
        // SAFETY: the callback retains its primary and the helper takes its
        // exclusive hardware lease. A published operation is never replayed.
        unsafe {
            crate::pdev::primary_operation(destination, |hardware, _| {
                let area = [bounds.left, bounds.top, bounds.right, bounds.bottom];
                // SAFETY: this closure owns the primary's hardware lease and mappings.
                Some(u32::from(hardware.solid(area, color, truth).is_ok()))
            })
        }
        .map(|result| result as i32)
    }

    unsafe fn pattern_fill(
        destination: *mut SurfaceObject,
        clip: *mut c_void,
        rectangle: *mut Rect,
        brush: *mut c_void,
        origin: *mut Point,
        rop: u32,
    ) -> Option<i32> {
        let truth = rop as u8;
        if origin.is_null()
            || brush.is_null()
            || rop & 0xffff != u32::from(truth) * 0x101
            || Rop3::new(truth).inputs().source
        {
            return None;
        }
        // SAFETY: the retained BRUSHOBJ starts with its solid colour sentinel.
        if unsafe { brush.cast::<u32>().read() } != u32::MAX {
            return None;
        }
        // SAFETY: GDI retains the primary and optional rectangular clip.
        let bounds = unsafe { destination_bounds(destination, clip, rectangle) }?;
        if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
            return Some(1);
        }
        // SAFETY: the brush and origin remain live; realisation precedes the lease.
        let paint = unsafe { crate::brush::paint(brush, origin) }?;
        let crate::paint::Paint::Pattern(pattern) = paint else {
            return None;
        };
        // SAFETY: the callback retains this complete brush origin.
        let origin = unsafe { origin.read() };
        let draw = crate::draw::PatternFill::new(
            [bounds.left, bounds.top, bounds.right, bounds.bottom],
            [origin.x, origin.y],
            truth,
        )?;
        // SAFETY: the exclusive lease copies and retains this pattern before return.
        unsafe {
            crate::pdev::primary_operation(destination, |hardware, _| {
                Some(u32::from(
                    hardware
                        .pattern(draw, pattern.width, pattern.height, pattern.pixels)
                        .is_ok(),
                ))
            })
        }
        .map(|result| result as i32)
    }

    unsafe fn destination_bounds(
        destination: *mut SurfaceObject,
        clip: *mut c_void,
        rectangle: *mut Rect,
    ) -> Option<Rect> {
        if destination.is_null() || rectangle.is_null() {
            return None;
        }
        // SAFETY: GDI retains these initialised native surface and geometry fields.
        let (kind, size, mut bounds) = unsafe {
            (
                core::ptr::addr_of!((*destination).kind).read(),
                core::ptr::addr_of!((*destination).size).read(),
                rectangle.read(),
            )
        };
        if kind == 0 {
            return None;
        }
        if !clip.is_null() {
            // SAFETY: GDI supplies the complete CLIPOBJ for this operation.
            let clip = unsafe { clip.cast::<crate::gdi::ClipObject>().as_ref() }?;
            match clip.draw_complexity {
                0 => {}
                1 => {
                    bounds.left = bounds.left.max(clip.bounds.left);
                    bounds.top = bounds.top.max(clip.bounds.top);
                    bounds.right = bounds.right.min(clip.bounds.right);
                    bounds.bottom = bounds.bottom.min(clip.bounds.bottom);
                }
                _ => return None,
            }
        }
        bounds.left = bounds.left.max(0);
        bounds.top = bounds.top.max(0);
        bounds.right = bounds.right.min(size.width);
        bounds.bottom = bounds.bottom.min(size.height);
        Some(bounds)
    }

    unsafe fn copy_primary(
        destination: *mut SurfaceObject,
        source: *mut SurfaceObject,
        clip: *mut c_void,
        translation: *mut c_void,
        rectangle: *mut Rect,
        origin: *mut Point,
    ) -> Option<i32> {
        if destination.is_null() || source.is_null() || origin.is_null() || rectangle.is_null() {
            return None;
        }
        // SAFETY: both native HSURF fields remain live through this callback.
        let same = unsafe {
            core::ptr::addr_of!((*destination).surface).read()
                == core::ptr::addr_of!((*source).surface).read()
        };
        if !same {
            return None;
        }
        if !translation.is_null() {
            // SAFETY: flXlate is the second ULONG of the supplied live XLATEOBJ.
            if unsafe { translation.cast::<u8>().add(4).cast::<u32>().read() } & 1 == 0 {
                return None;
            }
        }
        // SAFETY: the callback retains these input records throughout lookup.
        let bounds = unsafe { destination_bounds(destination, clip, rectangle) }?;
        if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
            return Some(1);
        }
        // SAFETY: GDI retains the original source point and destination rectangle.
        let (origin, original) = unsafe { (origin.read(), rectangle.read()) };
        let x =
            i32::try_from(i64::from(origin.x) + i64::from(bounds.left) - i64::from(original.left))
                .ok()?;
        let y =
            i32::try_from(i64::from(origin.y) + i64::from(bounds.top) - i64::from(original.top))
                .ok()?;
        // SAFETY: the callback retains the one primary and its exclusive lease.
        unsafe {
            crate::pdev::primary_operation(destination, |hardware, _| {
                let area = [bounds.left, bounds.top, bounds.right, bounds.bottom];
                // SAFETY: this closure owns the session's primary and mappings.
                Some(u32::from(hardware.copy_bits(area, [x, y]).is_ok()))
            })
        }
        .map(|result| result as i32)
    }

    #[derive(Clone, Copy)]
    enum ImagePixels {
        Copy,
        Color(bool),
    }

    unsafe fn capture_image(
        destination: *mut SurfaceObject,
        source: *mut SurfaceObject,
        clip: *mut c_void,
        translation: *mut c_void,
        rectangle: *mut Rect,
        source_rect: Rect,
        capture: ImagePixels,
    ) -> Option<(Rect, crate::native_pixels::Pixels)> {
        if destination.is_null() || source.is_null() || rectangle.is_null() {
            return None;
        }
        // SAFETY: GDI retains the initialised format fields of both surfaces.
        if unsafe {
            core::ptr::addr_of!((*destination).bitmap_format).read() != 6
                || match core::ptr::addr_of!((*source).bitmap_format).read() {
                    6 => false,
                    1..=3 => !matches!(capture, ImagePixels::Copy),
                    _ => true,
                }
        } {
            return None;
        }
        // SAFETY: the callback retains all native geometry records.
        let (original, bounds) = unsafe {
            (
                rectangle.read(),
                destination_bounds(destination, clip, rectangle)?,
            )
        };
        if i64::from(original.right) - i64::from(original.left)
            != i64::from(source_rect.right) - i64::from(source_rect.left)
            || i64::from(original.bottom) - i64::from(original.top)
                != i64::from(source_rect.bottom) - i64::from(source_rect.top)
            || bounds.left >= bounds.right
            || bounds.top >= bounds.bottom
        {
            return None;
        }
        let x = i32::try_from(
            i64::from(source_rect.left) + i64::from(bounds.left) - i64::from(original.left),
        )
        .ok()?;
        let y = i32::try_from(
            i64::from(source_rect.top) + i64::from(bounds.top) - i64::from(original.top),
        )
        .ok()?;
        let area = Rect {
            left: x,
            top: y,
            right: x.checked_add(bounds.right - bounds.left)?,
            bottom: y.checked_add(bounds.bottom - bounds.top)?,
        };
        // SAFETY: capture copies only the retained bitmap's defined pixel bytes.
        let pixels = unsafe {
            crate::native_pixels::Pixels::capture_image(
                source,
                translation,
                area,
                matches!(capture, ImagePixels::Color(true)),
            )
        }?;
        Some((bounds, pixels))
    }

    unsafe fn submit_image(
        destination: *mut SurfaceObject,
        draw: crate::draw::ImageDraw,
        pixels: &crate::native_pixels::Pixels,
        alpha: bool,
    ) -> Option<i32> {
        // SAFETY: the callback retains its primary and copied source bytes until
        // publication has retained an independent, immutable image resource.
        unsafe {
            crate::pdev::primary_operation(destination, |hardware, _| {
                // SAFETY: this lease exclusively retains the destination and mappings.
                if let Some((bitmap, colors)) = pixels.indexed() {
                    return Some(u32::from(
                        hardware.indexed_image(draw, bitmap, colors).is_ok(),
                    ));
                }
                Some(u32::from(
                    hardware
                        .image(
                            draw,
                            u32::from(pixels.width),
                            u32::from(pixels.height),
                            pixels.bytes(),
                            alpha,
                        )
                        .is_ok(),
                ))
            })
        }
        .map(|result| result as i32)
    }

    unsafe fn copy_image(
        destination: *mut SurfaceObject,
        source: *mut SurfaceObject,
        clip: *mut c_void,
        translation: *mut c_void,
        rectangle: *mut Rect,
        origin: *mut Point,
        rop: u32,
    ) -> Option<i32> {
        if origin.is_null() || rectangle.is_null() {
            return None;
        }
        let truth = rop as u8;
        if rop & 0xffff != u32::from(truth) * 0x101
            || !matches!(
                Rop3::new(truth).method(),
                crate::rop::Method::Copy(_) | crate::rop::Method::Blend(_)
            )
        {
            return None;
        }
        // SAFETY: GDI retains both complete native geometry records.
        let (origin, original) = unsafe { (origin.read(), rectangle.read()) };
        let width = original.right.checked_sub(original.left)?;
        let height = original.bottom.checked_sub(original.top)?;
        let area = Rect {
            left: origin.x,
            top: origin.y,
            right: origin.x.checked_add(width)?,
            bottom: origin.y.checked_add(height)?,
        };
        // SAFETY: the callback retains the source bitmap and destination geometry.
        let (bounds, pixels) = unsafe {
            capture_image(
                destination,
                source,
                clip,
                translation,
                rectangle,
                area,
                ImagePixels::Copy,
            )
        }?;
        let draw = crate::draw::ImageDraw::copy(
            [bounds.left, bounds.top, bounds.right, bounds.bottom],
            [0, 0, i32::from(pixels.width), i32::from(pixels.height)],
            truth,
        )?;
        // SAFETY: the copied source stays live through independent publication.
        unsafe { submit_image(destination, draw, &pixels, false) }
    }

    /// Kernel GDI copy callback for standard bitmaps and retained primaries.
    /// # Safety
    /// GDI supplies live SURFOBJs, nonempty ordered destination rectangle and
    /// source origin at PASSIVE_LEVEL. Retain every optional clip/translation
    /// object through the call and exclude surface destruction.
    pub unsafe extern "system" fn copy_bits(
        destination: *mut SurfaceObject,
        source: *mut SurfaceObject,
        clip: *mut c_void,
        translation: *mut c_void,
        destination_rect: *mut Rect,
        source_origin: *mut Point,
    ) -> i32 {
        if source.is_null() || destination_rect.is_null() || source_origin.is_null() {
            return 0;
        }
        // SAFETY: GDI retains both surfaces and all source/clip geometry.
        if let Some(result) = unsafe {
            copy_primary(
                destination,
                source,
                clip,
                translation,
                destination_rect,
                source_origin,
            )
        } {
            return result;
        }
        // SAFETY: GDI retains the source bitmap, destination and geometry records.
        if let Some(result) = unsafe {
            copy_image(
                destination,
                source,
                clip,
                translation,
                destination_rect,
                source_origin,
                0xcccc,
            )
        } {
            return result;
        }
        // SAFETY: GDI retains all callback inputs; the lease synchronises primary
        // views and the closure neither stores pointers nor changes ownership.
        unsafe {
            draw([destination, source, null_mut()], |surfaces| {
                // SAFETY: both surfaces are live standard bitmaps during this
                // closure; GDI supplied the remaining native operation records.
                EngCopyBits(
                    surfaces[0],
                    surfaces[1],
                    clip,
                    translation,
                    destination_rect,
                    source_origin,
                )
            })
        }
    }

    /// Kernel GDI raster callback, including patterns, masks and ROP4 selection.
    /// # Safety
    /// GDI supplies live destination and required operands at PASSIVE_LEVEL,
    /// including a nonempty ordered destination rectangle. Retain all referenced
    /// objects and exclude surface destruction. Unused source/mask are ignored.
    pub unsafe extern "system" fn bit_blt(
        destination: *mut SurfaceObject,
        source: *mut SurfaceObject,
        mask: *mut SurfaceObject,
        clip: *mut c_void,
        translation: *mut c_void,
        destination_rect: *mut Rect,
        source_origin: *mut Point,
        mask_origin: *mut Point,
        brush: *mut c_void,
        brush_origin: *mut Point,
        rop: u32,
    ) -> i32 {
        let (needs_source, needs_mask) = operands(rop);
        if destination_rect.is_null()
            || (needs_source && (source.is_null() || source_origin.is_null()))
        {
            return 0;
        }
        if rop & 0xffff == 0xcccc {
            // SAFETY: the required source and geometry have been validated above.
            if let Some(result) = unsafe {
                copy_primary(
                    destination,
                    source,
                    clip,
                    translation,
                    destination_rect,
                    source_origin,
                )
            } {
                return result;
            }
        }
        if !needs_source && !needs_mask {
            // SAFETY: GDI retains the destination, clip, brush and rectangle.
            if let Some(result) = unsafe { solid(destination, clip, destination_rect, brush, rop) }
            {
                return result;
            }
        }
        if !needs_source && !needs_mask {
            // SAFETY: GDI retains the destination, realised brush and origin.
            let accelerated = unsafe {
                pattern_fill(
                    destination,
                    clip,
                    destination_rect,
                    brush,
                    brush_origin,
                    rop,
                )
            };
            if let Some(result) = accelerated {
                return result;
            }
        }
        // SAFETY: required source and geometry records were validated above.
        if let Some(result) = unsafe {
            copy_image(
                destination,
                source,
                clip,
                translation,
                destination_rect,
                source_origin,
                rop,
            )
        } {
            return result;
        }
        let source_rect = (|| {
            if !needs_source || source_origin.is_null() {
                return None;
            }
            // SAFETY: required source and destination records were validated above.
            let (origin, rectangle) = unsafe { (source_origin.read(), destination_rect.read()) };
            Some(Rect {
                left: origin.x,
                top: origin.y,
                right: origin
                    .x
                    .checked_add(rectangle.right.checked_sub(rectangle.left)?)?,
                bottom: origin
                    .y
                    .checked_add(rectangle.bottom.checked_sub(rectangle.top)?)?,
            })
        })();
        // SAFETY: GDI retains all source, mask, brush, clip and geometry operands.
        let accelerated = unsafe {
            crate::native_raster::Inputs {
                destination,
                source,
                mask,
                clip,
                translation,
                destination_rect,
                source_rect,
                mask_origin,
                brush,
                brush_origin,
                rop,
            }
            .submit()
        };
        if let Some(result) = accelerated {
            return result;
        }
        let source = if needs_source { source } else { null_mut() };
        let mask = if needs_mask { mask } else { null_mut() };
        // SAFETY: only required surface operands enter the view builder; GDI
        // retains the operation records and the closure ends all pointer use.
        unsafe {
            draw([destination, source, mask], |surfaces| {
                // SAFETY: live standard bitmap views and original GDI parameters
                // satisfy EngBitBlt; a null mask selects its implicit brush mask.
                EngBitBlt(
                    surfaces[0],
                    surfaces[1],
                    surfaces[2],
                    clip,
                    translation,
                    destination_rect,
                    source_origin,
                    mask_origin,
                    brush,
                    brush_origin,
                    rop,
                )
            })
        }
    }
    unsafe fn text_primary(
        destination: *mut SurfaceObject,
        text: *mut c_void,
        font: *mut c_void,
        clip: *mut c_void,
        extra: *mut Rect,
        paint: (*mut Rect, *mut c_void, *mut c_void, *mut Point, u32),
    ) -> Option<i32> {
        let (opaque, foreground, background, brush_origin, mix) = paint;
        if text.is_null() || foreground.is_null() || !extra.is_null() {
            return None;
        }
        // SAFETY: GDI retains the STROBJ and brush realisation through this call.
        let (mut area, foreground) = unsafe {
            (
                core::ptr::addr_of!((*text.cast::<crate::native_text::StringObject>()).background)
                    .read(),
                crate::brush::paint(foreground, brush_origin)?,
            )
        };
        let background = if !opaque.is_null() {
            if background.is_null() {
                return None;
            }
            // SAFETY: the optional opaque rectangle and background brush are live.
            let (rectangle, color) = unsafe {
                (
                    opaque.read(),
                    crate::brush::paint(background, brush_origin)?,
                )
            };
            area.left = area.left.min(rectangle.left);
            area.top = area.top.min(rectangle.top);
            area.right = area.right.max(rectangle.right);
            area.bottom = area.bottom.max(rectangle.bottom);
            Some((
                [
                    rectangle.left,
                    rectangle.top,
                    rectangle.right,
                    rectangle.bottom,
                ],
                color,
            ))
        } else {
            None
        };
        // SAFETY: native clipping and primary dimensions remain live through capture.
        let bounds = unsafe { destination_bounds(destination, clip, &mut area) }?;
        if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
            return Some(1);
        }
        let draw = crate::text::Text::new(
            [bounds.left, bounds.top, bounds.right, bounds.bottom],
            foreground,
            background,
            mix as u8,
        )?;
        // SAFETY: GDI retains the font and all enumerated glyphs during capture.
        let captured = unsafe { crate::native_text::Captured::capture(text, font) }?;
        let glyphs = captured.glyphs()?;
        // SAFETY: the callback retains its primary; the exclusive lease publishes
        // a separate immutable copy of the captured glyphs for host completion.
        unsafe {
            crate::pdev::primary_operation(destination, |hardware, _| {
                Some(u32::from(hardware.text(draw, glyphs).is_ok()))
            })
        }
        .map(|result| result as i32)
    }

    unsafe fn stroke_primary(
        destination: *mut SurfaceObject,
        path: *mut c_void,
        clip: *mut c_void,
        brush: *mut c_void,
        attributes: *mut c_void,
        brush_origin: *mut Point,
        mix: u32,
    ) -> Option<i32> {
        let operation = crate::rop::Rop2::new(mix as u8)?;
        let color = if operation.uses_brush() {
            // SAFETY: GDI retains this brush and its origin before the hardware lease.
            unsafe { crate::brush::paint(brush, brush_origin) }?
        } else {
            crate::paint::Paint::Solid(0)
        };
        // SAFETY: the callback retains its native path and cosmetic attributes.
        let (mut bounds, style) = unsafe {
            (
                crate::native_path::bounds(path)?,
                crate::native_path::style(attributes)?,
            )
        };
        // SAFETY: the primary and native clip remain live through publication.
        let bounds = unsafe { destination_bounds(destination, clip, &mut bounds) }?;
        if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
            return Some(1);
        }
        let draw = crate::path::Stroke::new(
            [bounds.left, bounds.top, bounds.right, bounds.bottom],
            color,
            mix,
        )?;
        // SAFETY: all GDI path points remain live through their private capture.
        let captured = unsafe { crate::native_path::Captured::capture(path) }?;
        let segments = captured.segments()?;
        // SAFETY: the exclusive lease publishes an independent retained path copy.
        unsafe {
            crate::pdev::primary_operation(destination, |hardware, _| {
                Some(u32::from(hardware.stroke(draw, segments, &style).is_ok()))
            })
        }
        .map(|result| result as i32)
    }

    /// GDI software rendering on standard bitmaps and retained primaries.
    /// # Safety
    /// GDI supplies all required live operands and records at PASSIVE_LEVEL.
    /// Retain them through the call and exclude surface destruction. The native
    /// EngTextOut contract applies to rectangles, formats and optional objects.
    pub unsafe extern "system" fn text_out(
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
    ) -> i32 {
        // SAFETY: GDI retains all native text, brush and clip records.
        if let Some(result) = unsafe {
            text_primary(
                destination,
                text,
                font,
                clip,
                extra,
                (opaque_rect, foreground, background, brush_origin, mix),
            )
        } {
            return result;
        }
        // SAFETY: GDI retains the complete callback data and the scoped
        // operation uses only synchronised bitmap views without retaining them.
        unsafe {
            draw([destination, null_mut(), null_mut()], |views| {
                EngTextOut(
                    views[0],
                    text,
                    font,
                    clip,
                    extra,
                    opaque_rect,
                    foreground,
                    background,
                    brush_origin,
                    mix,
                )
            })
        }
    }

    /// GDI software rendering on standard bitmaps and retained primaries.
    /// # Safety
    /// GDI supplies all required live operands and records at PASSIVE_LEVEL.
    /// Retain them through the call and exclude surface destruction. The native
    /// EngStrokePath contract applies to rectangles, formats and optional objects.
    pub unsafe extern "system" fn stroke_path(
        destination: *mut SurfaceObject,
        path: *mut c_void,
        clip: *mut c_void,
        transform: *mut c_void,
        brush: *mut c_void,
        brush_origin: *mut Point,
        attributes: *mut c_void,
        mix: u32,
    ) -> i32 {
        // SAFETY: GDI retains all path, clip, brush and line-attribute inputs.
        let accelerated = unsafe {
            stroke_primary(
                destination,
                path,
                clip,
                brush,
                attributes,
                brush_origin,
                mix,
            )
        };
        if let Some(result) = accelerated {
            return result;
        }
        // SAFETY: GDI retains the complete callback data and the scoped
        // operation uses only synchronised bitmap views without retaining them.
        unsafe {
            draw([destination, null_mut(), null_mut()], |views| {
                EngStrokePath(
                    views[0],
                    path,
                    clip,
                    transform,
                    brush,
                    brush_origin,
                    attributes,
                    mix,
                )
            })
        }
    }

    /// GDI software rendering on standard bitmaps and retained primaries.
    /// # Safety
    /// GDI supplies all required live operands and records at PASSIVE_LEVEL.
    /// Retain them through the call and exclude surface destruction. The native
    /// EngStretchBlt contract applies to rectangles, formats and optional objects.
    pub unsafe extern "system" fn stretch_blt(
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
    ) -> i32 {
        if mode == 3 && color_adjustment.is_null() && !source_rect.is_null() {
            // SAFETY: the callback retains the full source rectangle and all inputs.
            let accelerated = unsafe {
                crate::native_raster::Inputs {
                    destination,
                    source,
                    mask,
                    clip,
                    translation,
                    destination_rect,
                    source_rect: Some(source_rect.read()),
                    mask_origin,
                    brush: null_mut(),
                    brush_origin: null_mut(),
                    rop: if mask.is_null() { 0xcccc } else { 0xaacc },
                }
                .submit()
            };
            if let Some(result) = accelerated {
                return result;
            }
        }
        // SAFETY: GDI retains the complete callback data and the scoped
        // operation uses only synchronised bitmap views without retaining them.
        unsafe {
            draw([destination, source, mask], |views| {
                EngStretchBlt(
                    views[0],
                    views[1],
                    views[2],
                    clip,
                    translation,
                    color_adjustment,
                    halftone_origin,
                    destination_rect,
                    source_rect,
                    mask_origin,
                    mode,
                )
            })
        }
    }

    /// GDI software rendering on standard bitmaps and retained primaries.
    /// # Safety
    /// GDI supplies all required live operands and records at PASSIVE_LEVEL.
    /// Retain them through the call and exclude surface destruction. The native
    /// EngStretchBltROP contract applies to rectangles, formats and optional objects.
    pub unsafe extern "system" fn stretch_blt_rop(
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
    ) -> i32 {
        let (needs_source, needs_mask) = operands(rop);
        if mode == 3 && color_adjustment.is_null() && (!needs_source || !source_rect.is_null()) {
            // SAFETY: GDI retains required source geometry and every operation operand.
            let accelerated = unsafe {
                crate::native_raster::Inputs {
                    destination,
                    source,
                    mask,
                    clip,
                    translation,
                    destination_rect,
                    source_rect: if needs_source {
                        Some(source_rect.read())
                    } else {
                        None
                    },
                    mask_origin,
                    brush,
                    brush_origin,
                    rop,
                }
                .submit()
            };
            if let Some(result) = accelerated {
                return result;
            }
        }
        let source = if needs_source { source } else { null_mut() };
        let mask = if needs_mask { mask } else { null_mut() };
        // SAFETY: GDI retains the complete callback data and the scoped
        // operation uses only synchronised bitmap views without retaining them.
        unsafe {
            draw([destination, source, mask], |views| {
                EngStretchBltROP(
                    views[0],
                    views[1],
                    views[2],
                    clip,
                    translation,
                    color_adjustment,
                    brush_origin,
                    destination_rect,
                    source_rect,
                    mask_origin,
                    mode,
                    brush,
                    rop,
                )
            })
        }
    }

    /// GDI software rendering on standard bitmaps and retained primaries.
    /// # Safety
    /// GDI supplies all required live operands and records at PASSIVE_LEVEL.
    /// Retain them through the call and exclude surface destruction. The native
    /// EngAlphaBlend contract applies to rectangles, formats and optional objects.
    pub unsafe extern "system" fn alpha_blend(
        destination: *mut SurfaceObject,
        source: *mut SurfaceObject,
        clip: *mut c_void,
        translation: *mut c_void,
        destination_rect: *mut Rect,
        source_rect: *mut Rect,
        blend: *mut c_void,
    ) -> i32 {
        let accelerated = (|| {
            if source_rect.is_null() || blend.is_null() {
                return None;
            }
            // SAFETY: BLENDOBJ begins with the supplied four-byte BLENDFUNCTION.
            let flags = unsafe { blend.cast::<[u8; 4]>().read() };
            if flags[0] != 0 || flags[1] != 0 || flags[3] > 1 {
                return None;
            }
            if flags[2] == 0 {
                return Some(1);
            }
            let alpha = flags[3] == 1;
            // SAFETY: the callback retains the source rectangle and source pixels.
            let (bounds, pixels) = unsafe {
                capture_image(
                    destination,
                    source,
                    clip,
                    translation,
                    destination_rect,
                    source_rect.read(),
                    ImagePixels::Color(alpha),
                )
            }?;
            let draw = crate::draw::ImageDraw::alpha(
                [bounds.left, bounds.top, bounds.right, bounds.bottom],
                [0, 0, i32::from(pixels.width), i32::from(pixels.height)],
                flags[2],
            )?;
            // SAFETY: copied premultiplied pixels stay live through publication.
            unsafe { submit_image(destination, draw, &pixels, alpha) }
        })();
        if let Some(result) = accelerated {
            return result;
        }
        // SAFETY: GDI retains the complete callback data and the scoped
        // operation uses only synchronised bitmap views without retaining them.
        unsafe {
            draw([destination, source, null_mut()], |views| {
                EngAlphaBlend(
                    views[0],
                    views[1],
                    clip,
                    translation,
                    destination_rect,
                    source_rect,
                    blend,
                )
            })
        }
    }

    /// Transparent copy using the synchronised GDI bitmap views.
    /// # Safety
    /// GDI supplies live surfaces, clip/translation objects and nonempty ordered
    /// rectangles at PASSIVE_LEVEL. The source rectangle is in bounds and does
    /// not overlap the destination on the same surface. Retain all inputs.
    pub unsafe extern "system" fn transparent_blt(
        destination: *mut SurfaceObject,
        source: *mut SurfaceObject,
        clip: *mut c_void,
        translation: *mut c_void,
        destination_rect: *mut Rect,
        source_rect: *mut Rect,
        color: u32,
        _reserved: u32,
    ) -> i32 {
        let accelerated = (|| {
            if source_rect.is_null() {
                return None;
            }
            // SAFETY: GDI retains the native rectangle and unscaled source pixels.
            let (bounds, pixels) = unsafe {
                capture_image(
                    destination,
                    source,
                    clip,
                    translation,
                    destination_rect,
                    source_rect.read(),
                    ImagePixels::Color(false),
                )
            }?;
            let draw = crate::draw::ImageDraw::transparent(
                [bounds.left, bounds.top, bounds.right, bounds.bottom],
                [0, 0, i32::from(pixels.width), i32::from(pixels.height)],
                color,
            )?;
            // SAFETY: copied source pixels stay live through publication.
            unsafe { submit_image(destination, draw, &pixels, false) }
        })();
        if let Some(result) = accelerated {
            return result;
        }
        // SAFETY: GDI retains the native inputs; the scoped call owns synchronised
        // bitmap views and preserves the engine's reserved-zero requirement.
        unsafe {
            draw([destination, source, null_mut()], |views| {
                EngTransparentBlt(
                    views[0],
                    views[1],
                    clip,
                    translation,
                    destination_rect,
                    source_rect,
                    color,
                    0,
                )
            })
        }
    }
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub use native::{
    alpha_blend, bit_blt, copy_bits, stretch_blt, stretch_blt_rop, stroke_path, text_out,
    transparent_blt,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rop4_operand_selection_matches_all_truth_tables() {
        for rop in 0..=u16::MAX {
            let output = |source: u32, mask: u32, pattern: u32, destination: u32| {
                let byte = u32::from(rop >> ((1 - mask) * 8)) & 0xff;
                (byte >> (pattern * 4 + source * 2 + destination)) & 1
            };
            let mut source_changes = false;
            let mut mask_changes = false;
            for pattern in 0..2 {
                for destination in 0..2 {
                    for other in 0..2 {
                        source_changes |= output(0, other, pattern, destination)
                            != output(1, other, pattern, destination);
                        mask_changes |= output(other, 0, pattern, destination)
                            != output(other, 1, pattern, destination);
                    }
                }
            }
            assert_eq!(
                operands(u32::from(rop)),
                (source_changes, mask_changes),
                "ROP4 {rop:04x}"
            );
        }
    }
}
