// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Native kernel GDI surface ABI and bitmap ownership.

use core::ffi::c_void;
use core::mem::{offset_of, size_of};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SurfaceObject {
    pub driver_surface: *mut c_void,
    pub surface: *mut c_void,
    pub driver_device: *mut c_void,
    pub device: *mut c_void,
    pub size: Size,
    pub bits_size: u32,
    pub bits: *mut c_void,
    pub first_scan: *mut c_void,
    pub stride: i32,
    pub unique: u32,
    pub bitmap_format: u32,
    pub kind: u16,
    pub flags: u16,
}

const _: () = {
    assert!(size_of::<Size>() == 8);
    assert!(offset_of!(SurfaceObject, driver_device) == 2 * size_of::<usize>());
    assert!(offset_of!(SurfaceObject, size) == 4 * size_of::<usize>());
};

#[cfg(target_pointer_width = "32")]
const _: () = {
    assert!(size_of::<SurfaceObject>() == 52);
    assert!(offset_of!(SurfaceObject, bits) == 28);
    assert!(offset_of!(SurfaceObject, first_scan) == 32);
    assert!(offset_of!(SurfaceObject, bitmap_format) == 44);
};

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<SurfaceObject>() == 80);
    assert!(offset_of!(SurfaceObject, bits) == 48);
    assert!(offset_of!(SurfaceObject, first_scan) == 56);
    assert!(offset_of!(SurfaceObject, bitmap_format) == 72);
};

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
mod native {
    use super::*;
    use core::marker::PhantomData;
    use core::ptr::NonNull;

    use crate::engine::{
        EngAssociateSurface, EngCreateBitmap, EngCreateDeviceBitmap, EngDeleteSurface,
        EngLockSurface, EngUnlockSurface,
    };
    use crate::surface::{Active, Format, Layout, Surface};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Error {
        CreateBitmap,
        AssociateSurface,
        LockSurface,
    }

    #[derive(Debug)]
    pub struct Bitmap(NonNull<c_void>);

    impl Bitmap {
        /// # Safety
        /// Call at PASSIVE_LEVEL with a live GDI device. The stable driver
        /// surface context must support registered callbacks, including deletion
        /// during construction failure, and outlive GDI's deletion callback.
        /// All supplied hooks must have registered implementations.
        pub unsafe fn device(
            engine: NonNull<c_void>,
            context: NonNull<c_void>,
            layout: Layout,
            hooks: u32,
        ) -> Result<Self, Error> {
            // SAFETY: the caller supplies the live callback context and device;
            // checked layouts have positive dimensions that fit GDI's SIZEL.
            let handle = unsafe {
                EngCreateDeviceBitmap(context.as_ptr(), dimensions(layout), bitmap_format(layout))
            };
            let bitmap = Self(NonNull::new(handle).ok_or(Error::CreateBitmap)?);
            // SAFETY: the bitmap was created above and the device/hooks are live.
            unsafe { bitmap.associate(engine, hooks)? };
            Ok(bitmap)
        }

        unsafe fn associate(&self, engine: NonNull<c_void>, hooks: u32) -> Result<(), Error> {
            // SAFETY: callers retain the live device, handle and registered hooks.
            if unsafe { EngAssociateSurface(self.0.as_ptr(), engine.as_ptr(), hooks) } == 0 {
                return Err(Error::AssociateSurface);
            }
            Ok(())
        }

        /// Transfer this device-bitmap handle to GDI; its deletion callback owns
        /// the remaining driver context lifetime.
        pub fn into_raw(self) -> *mut c_void {
            let handle = self.0.as_ptr();
            core::mem::forget(self);
            handle
        }
    }

    impl Drop for Bitmap {
        fn drop(&mut self) {
            // SAFETY: this unique owner deletes only an unlocked handle. DrawArea
            // unlocks before field destruction; failed constructors never hand off.
            unsafe { EngDeleteSurface(self.0.as_ptr()) };
        }
    }

    #[derive(Debug)]
    pub struct DrawArea<'pixels> {
        bitmap: Bitmap,
        object: NonNull<SurfaceObject>,
        pixels: PhantomData<&'pixels mut [u8]>,
    }

    impl<'pixels> DrawArea<'pixels> {
        /// # Safety
        /// Call and destroy at PASSIVE_LEVEL in serialised GDI callbacks with a
        /// live device. Synchronise CPU/host access before GDI touches pixels.
        pub unsafe fn from_surface(
            surface: &'pixels Surface<'_, Active>,
            engine: NonNull<c_void>,
        ) -> Result<Self, Error> {
            // SAFETY: the surface borrow retains its complete pixel allocation
            // through this draw area's lifetime. Surface allocations are non-null.
            unsafe {
                Self::from_raw(
                    engine,
                    NonNull::new_unchecked(surface.as_ptr()),
                    surface.layout(),
                )
            }
        }

        /// # Safety
        /// In addition to the GDI callback contract, pixels must remain writable
        /// for layout.bytes() through 'pixels. Retain their owner until this draw
        /// area drops, and synchronise all GDI, guest and host pixel access.
        pub(crate) unsafe fn from_raw(
            engine: NonNull<c_void>,
            pixels: NonNull<u8>,
            layout: Layout,
        ) -> Result<Self, Error> {
            // SAFETY: the caller retains the complete mapping; layout checks
            // dimensions/stride. Flags0 give the original bottom-up GDI bitmap.
            let handle = unsafe {
                EngCreateBitmap(
                    dimensions(layout),
                    layout.stride(),
                    bitmap_format(layout),
                    0,
                    pixels.as_ptr().cast(),
                )
            };
            let bitmap = Bitmap(NonNull::new(handle).ok_or(Error::CreateBitmap)?);
            // SAFETY: the private software bitmap has no driver rendering hooks.
            unsafe { bitmap.associate(engine, 0)? };
            // SAFETY: association succeeded and the handle remains uniquely owned.
            let object = unsafe { EngLockSurface(bitmap.0.as_ptr()) };
            Ok(Self {
                bitmap,
                object: NonNull::new(object).ok_or(Error::LockSurface)?,
                pixels: PhantomData,
            })
        }

        pub fn as_ptr(&self) -> *mut SurfaceObject {
            self.object.as_ptr()
        }

        pub fn handle(&self) -> *mut c_void {
            self.bitmap.0.as_ptr()
        }
    }

    impl Drop for DrawArea<'_> {
        fn drop(&mut self) {
            // SAFETY: this object was successfully locked and is still live.
            // Bitmap field destruction deletes the handle after this unlock.
            unsafe { EngUnlockSurface(self.object.as_ptr()) };
        }
    }

    fn dimensions(layout: Layout) -> Size {
        Size {
            width: layout.width() as i32,
            height: layout.height() as i32,
        }
    }

    fn bitmap_format(layout: Layout) -> u32 {
        match layout.format() {
            Format::Rgb555 => 4,
            Format::Xrgb8888 => 6,
        }
    }
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub use native::{Bitmap, DrawArea, Error};
