// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Realised pattern brushes from display/brush.c.

use crate::engine::{BRUSHOBJ_pvAllocRbrush, BRUSHOBJ_pvGetRbrush, EngCopyBits, Memory};
use crate::gdi::{DrawArea, Point, Rect, SurfaceObject};
use crate::surface::{Format, Layout};
use core::ffi::c_void;
use core::ptr::{NonNull, addr_of, null_mut};

/// # Safety
/// GDI retains the brush, target, source pattern and optional translation at
/// PASSIVE_LEVEL. The realised allocation belongs to GDI after this callback.
pub unsafe extern "system" fn realize(
    brush: *mut c_void,
    target: *mut SurfaceObject,
    pattern: *mut SurfaceObject,
    mask: *mut SurfaceObject,
    translation: *mut c_void,
    _hatch: u32,
) -> i32 {
    // SAFETY: the callback retains all native operands through synchronous conversion.
    i32::from(unsafe { realise(brush, target, pattern, mask, translation) }.is_some())
}

unsafe fn realise(
    brush: *mut c_void,
    target: *mut SurfaceObject,
    pattern: *mut SurfaceObject,
    mask: *mut SurfaceObject,
    translation: *mut c_void,
) -> Option<()> {
    if brush.is_null() || target.is_null() || pattern.is_null() || !mask.is_null() {
        return None;
    }
    // SAFETY: GDI retains the initialised surface metadata.
    let (size, kind, format, engine) = unsafe {
        (
            addr_of!((*pattern).size).read(),
            addr_of!((*pattern).kind).read(),
            addr_of!((*target).bitmap_format).read(),
            addr_of!((*target).device).read(),
        )
    };
    if kind != 0 || format != 6 || size.width > 4096 || size.height > 4096 {
        return None;
    }
    let layout = Layout::new(
        u32::try_from(size.width).ok()?,
        u32::try_from(size.height).ok()?,
        Format::Xrgb8888,
    )
    .ok()?;
    let length = u32::try_from(layout.bytes()).ok()?;
    let extent = length.checked_add(8)?;
    // SAFETY: this callback supplies the passive-level GDI heap context.
    let memory = unsafe { Memory::zeroed(length) }.ok()?;
    // SAFETY: the owned pixels and native device outlive this synchronous view.
    let area =
        unsafe { DrawArea::from_raw(NonNull::new(engine)?, memory.as_ptr(), layout) }.ok()?;
    let mut rectangle = Rect {
        left: 0,
        top: 0,
        right: size.width,
        bottom: size.height,
    };
    let mut origin = Point { x: 0, y: 0 };
    // SAFETY: EngCopyBits converts the live source and XLATEOBJ to an unhooked
    // CPU bitmap; it cannot recurse through this driver's rendering callbacks.
    if unsafe {
        EngCopyBits(
            area.as_ptr(),
            pattern,
            null_mut(),
            translation,
            &mut rectangle,
            &mut origin,
        )
    } == 0
    {
        return None;
    }
    // SAFETY: the allocation is owned and freed by GDI with this BRUSHOBJ.
    let output = unsafe { BRUSHOBJ_pvAllocRbrush(brush, extent) }.cast::<u8>();
    if output.is_null() {
        return None;
    }
    // SAFETY: every output byte is initialised; source BGR bytes are defined by
    // the completed GDI copy. Undefined X bytes and row padding are not copied.
    unsafe {
        core::ptr::write_bytes(output, 0, extent as usize);
        output.cast::<u32>().write(layout.width());
        output.add(4).cast::<u32>().write(layout.height());
        for y in 0..layout.height() as usize {
            let source = memory
                .as_ptr()
                .as_ptr()
                .add((layout.height() as usize - 1 - y) * layout.stride() as usize);
            let destination = output.add(8 + y * layout.stride() as usize);
            for x in 0..layout.width() as usize {
                core::ptr::copy_nonoverlapping(source.add(x * 4), destination.add(x * 4), 3);
            }
        }
    }
    Some(())
}

/// # Safety
/// Retain this driver's BRUSHOBJ, origin and realised allocation through 'a.
/// Call before taking the hardware lease because realisation can enter GDI.
pub(crate) unsafe fn paint<'a>(
    brush: *mut c_void,
    origin: *mut Point,
) -> Option<crate::paint::Paint<'a>> {
    if brush.is_null() {
        return None;
    }
    // SAFETY: the retained BRUSHOBJ begins with the solid-colour word.
    let color = unsafe { brush.cast::<u32>().read() };
    if color != u32::MAX {
        return Some(crate::paint::Paint::Solid(color));
    }
    if origin.is_null() {
        return None;
    }
    // SAFETY: GDI retains the complete brush and invokes our realise callback.
    let pointer = unsafe { BRUSHOBJ_pvGetRbrush(brush) }.cast::<u8>();
    if pointer.is_null() {
        return None;
    }
    // SAFETY: only realise creates this driver's complete header and pixel array.
    let (width, height, origin) = unsafe {
        (
            pointer.cast::<u32>().read(),
            pointer.add(4).cast::<u32>().read(),
            origin.read(),
        )
    };
    let layout = Layout::new(width, height, Format::Xrgb8888).ok()?;
    // SAFETY: GDI retains the realised extent for the caller's specified lifetime.
    let pixels = unsafe { core::slice::from_raw_parts(pointer.add(8), layout.bytes()) };
    Some(crate::paint::Paint::Pattern(crate::paint::Pattern::new(
        width,
        height,
        pixels,
        [origin.x, origin.y],
    )?))
}
