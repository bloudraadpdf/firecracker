// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Native PDev setup, completion and destruction from display/driver.c.

use core::mem::size_of;
use core::ptr::NonNull;

use crate::pdev_config::{DeviceInfo, GdiInfo};

struct Buffers {
    caps: NonNull<u8>,
    info: NonNull<u8>,
}

impl Buffers {
    fn new(caps: *mut u8, caps_bytes: u32, info: *mut u8, info_bytes: u32) -> Option<Self> {
        let caps = NonNull::new(caps)?;
        let info = NonNull::new(info)?;
        let caps_end = caps.as_ptr().addr().checked_add(size_of::<GdiInfo>())?;
        let info_end = info.as_ptr().addr().checked_add(size_of::<DeviceInfo>())?;
        if (caps_bytes as usize) < size_of::<GdiInfo>()
            || (info_bytes as usize) < size_of::<DeviceInfo>()
            || (caps.as_ptr().addr() < info_end && info.as_ptr().addr() < caps_end)
        {
            return None;
        }
        Some(Self { caps, info })
    }

    unsafe fn write(&self, gdi: &GdiInfo, device: &DeviceInfo) -> Option<()> {
        // SAFETY: the GDI caller supplies both complete writable extents;
        // validation excludes overlap, and only the known prefix is borrowed.
        let caps =
            unsafe { core::slice::from_raw_parts_mut(self.caps.as_ptr(), size_of::<GdiInfo>()) };
        gdi.encode(caps)?;
        // SAFETY: the first output borrow ended and this complete second
        // writable output is disjoint from it and from private driver state.
        let info =
            unsafe { core::slice::from_raw_parts_mut(self.info.as_ptr(), size_of::<DeviceInfo>()) };
        device.encode(info)?;
        Some(())
    }
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
mod native {
    use super::*;
    use crate::display_modes::DEVMODE_BYTES;
    use crate::engine::{
        EngAcquireSemaphore, EngCreateSemaphore, EngDeleteSemaphore, EngReleaseSemaphore,
        allocate_object,
    };
    use crate::kernel_object::Object;
    use crate::pdev_config::PreparedDevice;
    use core::ffi::c_void;
    use core::ptr::null_mut;
    use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering, fence};

    static SERIALIZER: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
    static DEVICES: AtomicUsize = AtomicUsize::new(0);

    #[derive(Debug)]
    struct Guard(NonNull<c_void>);

    impl Drop for Guard {
        fn drop(&mut self) {
            fence(Ordering::Release);
            // SAFETY: this non-Send guard retains the acquired driver semaphore;
            // driver shutdown waits for all callbacks and device instances to stop.
            unsafe { EngReleaseSemaphore(self.0.as_ptr()) };
        }
    }

    unsafe fn lock_driver() -> Option<Guard> {
        let handle = NonNull::new(SERIALIZER.load(Ordering::Acquire))?;
        // SAFETY: the caller runs a passive-level GDI callback while the driver is
        // live; its shutdown cannot destroy the semaphore until callbacks stop.
        unsafe { EngAcquireSemaphore(handle.as_ptr()) };
        fence(Ordering::Acquire);
        Some(Guard(handle))
    }

    /// Initialise the shared lock before registering any PDev callbacks.
    /// # Safety
    /// GDI must serialise driver load/unload. Call at PASSIVE_LEVEL before any
    /// device callback can run; keep the DLL live until shutdown completes.
    pub unsafe fn initialise_driver() -> bool {
        if !SERIALIZER.load(Ordering::Acquire).is_null() {
            return false;
        }
        // SAFETY: the caller provides the passive-level, serialised load context.
        let handle = unsafe { EngCreateSemaphore() };
        if handle.is_null() {
            return false;
        }
        if SERIALIZER
            .compare_exchange(null_mut(), handle, Ordering::Release, Ordering::Relaxed)
            .is_err()
        {
            // SAFETY: this losing initialiser still owns its unused new semaphore.
            unsafe { EngDeleteSemaphore(handle) };
            return false;
        }
        true
    }

    /// Release driver state after all PDevs and callbacks have stopped.
    /// # Safety
    /// GDI must exclude new or in-progress callbacks and serialise load/unload.
    /// Never unload the DLL while a PDev or any interior user remains live.
    pub unsafe fn shutdown_driver() -> bool {
        if DEVICES.load(Ordering::Acquire) != 0 {
            return false;
        }
        if let Some(handle) = NonNull::new(SERIALIZER.swap(null_mut(), Ordering::AcqRel)) {
            // SAFETY: GDI stopped all callbacks, no device remains, and swap
            // transfers sole ownership of the semaphore to this shutdown call.
            unsafe { EngDeleteSemaphore(handle.as_ptr()) };
        }
        true
    }

    #[derive(Debug)]
    struct Instance;

    impl Instance {
        // The driver lock serialises construction; each counter token has one owner.
        fn new(_guard: &Guard) -> Option<Self> {
            let count = DEVICES.load(Ordering::Relaxed).checked_add(1)?;
            DEVICES.store(count, Ordering::Release);
            Some(Self)
        }
    }

    impl Drop for Instance {
        fn drop(&mut self) {
            DEVICES.fetch_sub(1, Ordering::Release);
        }
    }

    #[derive(Debug)]
    struct Device {
        prepared: PreparedDevice,
        engine: AtomicPtr<c_void>,
        _instance: Instance,
    }

    unsafe fn create(
        driver: NonNull<c_void>,
        devmode: NonNull<u8>,
        buffers: Buffers,
        guard: &Guard,
    ) -> Option<NonNull<c_void>> {
        // SAFETY: the GDI callback supplies a readable complete DEVMODEW prefix.
        let size = unsafe { devmode.as_ptr().add(68).cast::<u16>().read_unaligned() } as usize;
        if size < DEVMODE_BYTES {
            return None;
        }
        // SAFETY: the callback supplies the full declared DEVMODEW extent and the
        // input borrow ends before any capability buffer is written.
        let mode = unsafe { core::slice::from_raw_parts(devmode.as_ptr(), size) };
        // SAFETY: the callback retains its live miniport handle at PASSIVE_LEVEL.
        let prepared = unsafe { PreparedDevice::new(driver, mode) }.ok()?;
        let device = Device {
            prepared,
            engine: AtomicPtr::new(null_mut()),
            _instance: Instance::new(guard)?,
        };
        // SAFETY: the stable PDev and its destructor remain in this driver's GDI
        // context; its palette owner outlives all published capability handles.
        let object = unsafe { allocate_object(device) }.ok()?;
        // SAFETY: the validated output extents remain live; this stable object is
        // private until both records are written. Failure drops it and its palette.
        unsafe {
            buffers.write(
                &object.get().prepared.configuration().gdi,
                &object.get().prepared.device_info(),
            )
        }?;
        Some(object.into_handle())
    }

    /// PDev configuration callback; DDI table publication remains separate.
    /// # Safety
    /// GDI must supply a live miniport handle at PASSIVE_LEVEL, a readable DEVMODEW
    /// for its declared dmSize, and both writable output extents. Outputs must not
    /// alias private driver state. Retain the returned opaque handle through all
    /// subsequent callbacks; destroy it once, after every user stops.
    pub unsafe extern "system" fn enable_pdev(
        devmode: *mut u8,
        _logical_address: *mut u16,
        _pattern_count: u32,
        _patterns: *mut *mut c_void,
        caps_bytes: u32,
        caps: *mut u32,
        info_bytes: u32,
        info: *mut DeviceInfo,
        _engine: *mut c_void,
        _device_name: *mut u16,
        driver: *mut c_void,
    ) -> *mut c_void {
        let (Some(driver), Some(devmode), Some(buffers)) = (
            NonNull::new(driver),
            NonNull::new(devmode),
            Buffers::new(caps.cast(), caps_bytes, info.cast(), info_bytes),
        ) else {
            return null_mut();
        };
        // SAFETY: this callback runs while the initialised driver is live at
        // PASSIVE_LEVEL; the guard serialises instance creation across all PDevs.
        let Some(guard) = (unsafe { lock_driver() }) else {
            return null_mut();
        };
        // SAFETY: GDI supplies the live input/output buffers and miniport context.
        unsafe { create(driver, devmode, buffers, &guard) }.map_or(null_mut(), NonNull::as_ptr)
    }

    /// Store the installed GDI device handle; the input hdev to enable is not used.
    /// # Safety
    /// GDI must retain this exact opaque PDev handle at PASSIVE_LEVEL and supply
    /// its live installed GDI device. No destruction may race this callback.
    pub unsafe extern "system" fn complete_pdev(device: *mut c_void, engine: *mut c_void) {
        let Some(device) = NonNull::new(device) else {
            return;
        };
        // SAFETY: GDI retains this exact typed callback object through the call.
        let device = unsafe { Object::<Device>::borrow_handle(device) };
        device.engine.store(engine, Ordering::Release);
    }

    /// Destroy the configuration PDev after GDI stops every callback/user.
    /// # Safety
    /// GDI must return this exact opaque handle once at PASSIVE_LEVEL after all
    /// hardware, surfaces and interior users have stopped. No borrow may remain.
    pub unsafe extern "system" fn disable_pdev(device: *mut c_void) {
        let Some(device) = NonNull::new(device) else {
            return;
        };
        // SAFETY: the driver's lifetime covers this final callback; the guard
        // serialises instance removal with construction and driver teardown.
        let Some(_guard) = (unsafe { lock_driver() }) else {
            return;
        };
        // SAFETY: GDI stopped all users and transfers its sole opaque ownership
        // back once. Object drops the complete value before the original heap block.
        drop(unsafe { Object::<Device>::from_handle(device) });
    }
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub use native::{complete_pdev, disable_pdev, enable_pdev, initialise_driver, shutdown_driver};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_outputs_keep_native_handles_and_preserve_extra_bytes() {
        let mut caps = [0xcc; size_of::<GdiInfo>() + 8];
        let mut info = [0xcc; size_of::<DeviceInfo>() + 8];
        let mut palette_byte = 0x5au8;
        let outputs = Buffers::new(
            caps.as_mut_ptr(),
            caps.len() as u32,
            info.as_mut_ptr(),
            info.len() as u32,
        )
        .unwrap();
        let gdi = GdiInfo {
            width: 800,
            height: 600,
            ..GdiInfo::default()
        };
        let device = DeviceInfo {
            palette: core::ptr::from_mut(&mut palette_byte).cast(),
            ..DeviceInfo::default()
        };
        // SAFETY: both complete local outputs are disjoint and exclusive. The
        // live palette byte remains valid through the encoded pointer's use.
        unsafe { outputs.write(&gdi, &device) }.unwrap();
        assert_eq!(&caps[16..24], &[32, 3, 0, 0, 88, 2, 0, 0]);
        assert_eq!(&caps[size_of::<GdiInfo>()..], &[0xcc; 8]);
        assert_eq!(&info[size_of::<DeviceInfo>()..], &[0xcc; 8]);
        // SAFETY: the encoder initialized the complete native record/padding;
        // read_unaligned does not assume alignment of the local byte buffer.
        let decoded = unsafe { info.as_ptr().cast::<DeviceInfo>().read_unaligned() };
        // SAFETY: native handle encoding must retain the live original pointee.
        unsafe { decoded.palette.cast::<u8>().write(0x6b) };
        assert_eq!(palette_byte, 0x6b);
    }

    #[test]
    fn null_short_wrapping_and_overlapping_outputs_fail_validation() {
        fn pointer(address: usize) -> *mut u8 {
            core::ptr::without_provenance_mut(address)
        }
        let caps_bytes = size_of::<GdiInfo>() as u32;
        let info_bytes = size_of::<DeviceInfo>() as u32;
        for (caps, caps_size, info, info_size) in [
            (0, caps_bytes, 0x2000, info_bytes),
            (0x1000, caps_bytes, 0, info_bytes),
            (0x1000, caps_bytes - 1, 0x2000, info_bytes),
            (0x1000, caps_bytes, 0x2000, info_bytes - 1),
            (usize::MAX - 16, caps_bytes, 0x2000, info_bytes),
            (0x1000, caps_bytes, usize::MAX - 16, info_bytes),
            (0x1000, caps_bytes, 0x1001, info_bytes),
            (0x1001, caps_bytes, 0x1000, info_bytes),
        ] {
            assert!(Buffers::new(pointer(caps), caps_size, pointer(info), info_size).is_none());
        }
        assert!(
            Buffers::new(
                pointer(0x1000),
                caps_bytes,
                pointer(0x1000 + caps_bytes as usize),
                info_bytes
            )
            .is_some()
        );
    }
}
