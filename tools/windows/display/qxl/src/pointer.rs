// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Native pointer callbacks from display/pointer.c and res.c.

use super::engine::XLATEOBJ_iXlate;
use super::native_pixels::Pixels;
use super::pdev::primary_operation;
use crate::cursor::{Command, Format, Input, Palette, Position, Shape, Trail};
use crate::gdi::{Rect, SurfaceObject};
use core::ffi::c_void;

/// # Safety
/// GDI supplies the live primary and pointer bitmaps, palette translation and
/// coordinates at PASSIVE_LEVEL; retain them through the callback.
pub unsafe extern "system" fn set_pointer_shape(
    surface: *mut SurfaceObject,
    mask: *mut SurfaceObject,
    color: *mut SurfaceObject,
    translation: *mut c_void,
    hot_x: i32,
    hot_y: i32,
    x: i32,
    y: i32,
    _area: *mut Rect,
    flags: u32,
) -> u32 {
    // SAFETY: GDI retains the primary and pointer inputs through this operation.
    unsafe {
        primary_operation(surface, |hardware, palette_format| {
            let position = Position::new(x, y).ok()?;
            // SAFETY: this retained session owns the completed release endpoint.
            hardware.reap(64).ok()?;
            let mut notify = false;
            {
                let resources = hardware.resources().ok()?;
                if flags & 1 != 0 {
                    let hotspot = [u16::try_from(hot_x).ok()?, u16::try_from(hot_y).ok()?];
                    let alpha = flags & 0x10 != 0;
                    let color_pixels = if color.is_null() {
                        None
                    } else {
                        Some(Pixels::capture(color, alpha)?)
                    };
                    let mask_pixels = if mask.is_null() || alpha {
                        None
                    } else {
                        Some(Pixels::capture(mask, false)?)
                    };
                    let mut colors = [0u32; 256];
                    let input = if alpha {
                        Input::Alpha {
                            bitmap: color_pixels.as_ref()?.bitmap()?,
                            hotspot,
                        }
                    } else if mask.is_null() {
                        Input::Transparent
                    } else if color_pixels
                        .as_ref()
                        .is_some_and(|pixels| pixels.format != Format::Mono)
                    {
                        let bitmap = color_pixels.as_ref()?;
                        let entries = match bitmap.format {
                            Format::Indexed4 => 16,
                            Format::Indexed8 => 256,
                            _ => 0,
                        };
                        if entries != 0 && translation.is_null() {
                            return None;
                        }
                        for (index, color) in colors[..entries].iter_mut().enumerate() {
                            *color = XLATEOBJ_iXlate(translation, index as u32);
                        }
                        Input::Color {
                            bitmap: bitmap.bitmap()?,
                            mask: mask_pixels.as_ref()?.bitmap()?,
                            palette: (entries != 0).then_some(Palette {
                                colors: &colors[..entries],
                                format: palette_format,
                            }),
                            hotspot,
                        }
                    } else {
                        Input::Mono {
                            mask: mask_pixels.as_ref()?.bitmap()?,
                            hotspot,
                        }
                    };
                    let slot = resources.main_slot();
                    let shape = Shape::allocate(&mut resources.pool, slot, input, 0).ok()?;
                    let result = shape.set(&mut resources.pool, &mut resources.cursor, position);
                    shape.release(&mut resources.pool).ok()?;
                    notify |= result.ok()?;
                } else {
                    notify |= Command::Move(position)
                        .submit(&mut resources.pool, &mut resources.cursor)
                        .ok()?;
                }
                // The accepted shape or move still needs its notification if
                // the optional trail command cannot be published.
                if let Ok(trail_notify) = Command::Trail(Trail::from_flags(flags))
                    .submit(&mut resources.pool, &mut resources.cursor)
                {
                    notify |= trail_notify;
                }
            }
            if notify {
                hardware.notify(true).ok()?;
            }
            Some(2)
        })
    }
    .unwrap_or(0)
}

/// # Safety
/// GDI supplies its retained primary at PASSIVE_LEVEL and serialises pointer calls.
pub unsafe extern "system" fn move_pointer(
    surface: *mut SurfaceObject,
    x: i32,
    y: i32,
    _area: *mut Rect,
) {
    // SAFETY: the callback retains the live primary through cursor publication.
    unsafe {
        primary_operation(surface, |hardware, _| {
            let position = Position::new(x, y).ok()?;
            hardware.reap(64).ok()?;
            let resources = hardware.resources().ok()?;
            let notify = Command::Move(position)
                .submit(&mut resources.pool, &mut resources.cursor)
                .ok()?;
            if notify {
                hardware.notify(true).ok()?;
            }
            Some(0)
        })
    };
}
