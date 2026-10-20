// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Kernel-GDI raster paths from display/rop.c.

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
    use crate::engine::{EngBitBlt, EngCopyBits};
    use crate::gdi::{Point, Rect, SurfaceObject};
    use crate::pdev::with_cpu_surfaces;
    use core::ffi::c_void;
    use core::ptr::null_mut;

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
        // SAFETY: GDI retains all callback inputs; the lease synchronises primary
        // views and the closure neither stores pointers nor changes ownership.
        let result = unsafe {
            with_cpu_surfaces([destination, source, null_mut()], |surfaces| {
                // SAFETY: both surfaces are live standard bitmaps during this
                // closure; GDI supplied the remaining native operation records.
                EngCopyBits(
                    surfaces[0],
                    surfaces[1],
                    clip,
                    translation,
                    destination_rect,
                    source_origin,
                ) != 0
            })
        };
        i32::from(matches!(result, Ok(true)))
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
        let source = if needs_source { source } else { null_mut() };
        let mask = if needs_mask { mask } else { null_mut() };
        // SAFETY: only required surface operands enter the view builder; GDI
        // retains the operation records and the closure ends all pointer use.
        let result = unsafe {
            with_cpu_surfaces([destination, source, mask], |surfaces| {
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
                ) != 0
            })
        };
        i32::from(matches!(result, Ok(true)))
    }
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub use native::{bit_blt, copy_bits};

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
