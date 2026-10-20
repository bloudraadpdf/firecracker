// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Native PDev setup, completion and destruction from display/driver.c.

use core::mem::size_of;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::pdev_config::{DeviceInfo, GdiInfo};

struct Buffers {
    caps: NonNull<u8>,
    info: NonNull<u8>,
}

#[derive(Debug)]
struct HardwareBorrow<'a>(&'a AtomicBool);

impl<'a> HardwareBorrow<'a> {
    fn acquire(flag: &'a AtomicBool) -> Option<Self> {
        (!flag.swap(true, Ordering::Acquire)).then(|| Self(flag))
    }
}

impl Drop for HardwareBorrow<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
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
    use crate::display_device::MappedDevice;
    use crate::display_modes::DEVMODE_BYTES;
    use crate::engine::{
        EngAcquireSemaphore, EngCreateSemaphore, EngDeleteSemaphore, EngReleaseSemaphore,
        allocate_object,
    };
    use crate::hardware::{self, Hardware};
    use crate::kernel_object::Object;
    use crate::native_bitmap::PrimaryBitmap;
    use crate::pdev_config::PreparedDevice;
    use core::cell::UnsafeCell;
    use core::ffi::c_void;
    use core::ptr::null_mut;
    use core::sync::atomic::{AtomicPtr, AtomicUsize, fence};

    static SERIALIZER: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
    static DEVICES: AtomicUsize = AtomicUsize::new(0);
    static FIRST: AtomicPtr<Device> = AtomicPtr::new(null_mut());
    static HARDWARE_BORROWED: AtomicBool = AtomicBool::new(false);

    #[derive(Debug)]
    struct Guard {
        semaphore: NonNull<c_void>,
        hardware: Option<HardwareBorrow<'static>>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            drop(self.hardware.take());
            fence(Ordering::Release);
            // SAFETY: this non-Send guard retains the acquired driver semaphore;
            // driver shutdown waits for all callbacks and device instances to stop.
            unsafe { EngReleaseSemaphore(self.semaphore.as_ptr()) };
        }
    }

    unsafe fn lock_driver() -> Option<Guard> {
        let handle = NonNull::new(SERIALIZER.load(Ordering::Acquire))?;
        // SAFETY: the caller runs a passive-level GDI callback while the driver is
        // live; its shutdown cannot destroy the semaphore until callbacks stop.
        unsafe { EngAcquireSemaphore(handle.as_ptr()) };
        fence(Ordering::Acquire);
        Some(Guard {
            semaphore: handle,
            hardware: None,
        })
    }

    unsafe fn lock_hardware() -> Option<Guard> {
        // SAFETY: hardware callbacks run within the live GDI driver context.
        let mut guard = unsafe { lock_driver() }?;
        guard.hardware = Some(HardwareBorrow::acquire(&HARDWARE_BORROWED)?);
        Some(guard)
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
        driver: NonNull<c_void>,
        attachment: UnsafeCell<Option<Attachment>>,
        next: AtomicPtr<Device>,
        _instance: Instance,
    }

    #[derive(Debug)]
    struct Attachment {
        _primary: PrimaryBitmap,
        session: Option<Session>,
    }

    #[derive(Debug)]
    struct Session {
        hardware: Hardware<'static>,
        // The stable allocation outlives its private, lifetime-erased borrower.
        _mapping: Object<MappedDevice>,
    }

    impl Session {
        unsafe fn prepare(device: &Device) -> Result<Self, SurfaceError> {
            let mode = device.prepared.configuration();
            // SAFETY: no PDev owns this adapter's rings or mappings; the driver lock
            // also serialises mode selection against the other native callbacks.
            let mapping =
                unsafe { MappedDevice::new(device.driver, mode) }.map_err(SurfaceError::Mapping)?;
            // SAFETY: the passive-level heap owner pins the mapping until teardown.
            let mapping = unsafe { allocate_object(mapping) }.map_err(SurfaceError::Memory)?;
            // SAFETY: Object never moves its value. The erased borrow stays private
            // in Session and is destroyed before its mapping owner on every path.
            let mapped = unsafe { &*core::ptr::from_ref(mapping.get()) };
            // SAFETY: this is the adapter's sole generation; no prior token survives.
            let hardware = unsafe { Hardware::prepare(mapped, mode.layout) }
                .map_err(SurfaceError::Hardware)?;
            Ok(Self {
                hardware,
                _mapping: mapping,
            })
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum SurfaceError {
        Busy,
        InvalidState,
        Memory(u32),
        Mapping(crate::display_device::Error),
        Bitmap(crate::native_bitmap::Error),
        Hardware(hardware::Error),
        UnsupportedSurface,
    }

    unsafe fn primary_owner(surface: *mut c_void, _guard: &Guard) -> Option<NonNull<Device>> {
        let mut current = FIRST.load(Ordering::Relaxed);
        while let Some(pointer) = NonNull::new(current) {
            // SAFETY: the hardware lease keeps list nodes live and excludes all
            // attachment borrows. Lookup completes before CPU views are borrowed.
            let device = unsafe { pointer.as_ref() };
            // SAFETY: no hardware mutation or other attachment borrow is active.
            let attachment = unsafe { &*device.attachment.get() };
            if attachment
                .as_ref()
                .is_some_and(|value| value._primary.handle().as_ptr() == surface)
            {
                return Some(pointer);
            }
            current = device.next.load(Ordering::Relaxed);
        }
        None
    }

    struct CpuView<'a> {
        area: Option<crate::gdi::DrawArea<'a>>,
        hardware: NonNull<Hardware<'static>>,
        extent: [i32; 4],
        dirty: bool,
    }

    impl<'a> CpuView<'a> {
        unsafe fn new(pointer: NonNull<Device>, _guard: &'a Guard) -> Result<Self, SurfaceError> {
            // SAFETY: the list owner retains this PDev throughout the hardware
            // lease. The caller creates only one view for each distinct primary.
            let device = unsafe { pointer.as_ref() };
            let engine = NonNull::new(device.engine.load(Ordering::Acquire))
                .ok_or(SurfaceError::InvalidState)?;
            let layout = device.prepared.configuration().layout;
            // SAFETY: lookup ended before view creation. Each view uses a distinct
            // PDev and hardware entry/reentry is excluded by the retained guard.
            let attachment = unsafe { &mut *device.attachment.get() };
            let session = attachment
                .as_mut()
                .and_then(|value| value.session.as_mut())
                .ok_or(SurfaceError::InvalidState)?;
            let mut hardware = NonNull::from(&mut session.hardware);
            // SAFETY: the lease retains this unique active session through view
            // deletion; update synchronises all prior host writes before CPU use.
            let area = unsafe { hardware.as_mut().primary_draw_area(engine) }
                .map_err(SurfaceError::Hardware)?;
            Ok(Self {
                area: Some(area),
                hardware,
                extent: [0, 0, layout.height() as i32, layout.width() as i32],
                dirty: false,
            })
        }

        unsafe fn finish(mut self) -> Result<(), SurfaceError> {
            // End the exclusive pixel borrow before using its hardware again.
            drop(self.area.take());
            if self.dirty {
                // SAFETY: the CPU view was unlocked/deleted, all GDI writes ended,
                // and the guard still excludes every other adapter user.
                unsafe { self.hardware.as_mut().update(0, self.extent) }
                    .map_err(SurfaceError::Hardware)?;
            }
            Ok(())
        }
    }

    /// Run one GDI raster operation with synchronised primary CPU views.
    /// # Safety
    /// Supply live GDI SURFOBJs at PASSIVE_LEVEL; only source/mask may be null.
    /// The closure must not retain pointers, change ownership or draw outside
    /// the destination. Keep all referenced surfaces and callback data live.
    pub(crate) unsafe fn with_cpu_surfaces(
        mut surfaces: [*mut crate::gdi::SurfaceObject; 3],
        draw: impl FnOnce([*mut crate::gdi::SurfaceObject; 3]) -> bool,
    ) -> Result<bool, SurfaceError> {
        // SAFETY: the raster callback retains the live passive GDI context.
        let guard = unsafe { lock_hardware() }.ok_or(SurfaceError::Busy)?;
        if surfaces[0].is_null() {
            return Err(SurfaceError::InvalidState);
        }
        let mut owners = [None; 3];
        let mut handles = [null_mut(); 3];
        // Resolve ownership before creating mutable session/pixel borrows.
        for (index, surface) in surfaces.iter().copied().enumerate() {
            if surface.is_null() {
                continue;
            }
            // SAFETY: GDI supplies the initialised surface kind; do not copy
            // unrelated SURFOBJ fields that an opaque surface need not expose.
            let kind = unsafe { core::ptr::addr_of!((*surface).kind).read() };
            if kind == 0 {
                // STYPE_BITMAP
                continue;
            }
            // SAFETY: GDI retains this surface's initialised native HSURF field.
            let handle = unsafe { core::ptr::addr_of!((*surface).surface).read() };
            handles[index] = handle;
            // SAFETY: the lease retains the list; no CPU views exist yet.
            owners[index] = Some(
                unsafe { primary_owner(handle, &guard) }.ok_or(SurfaceError::UnsupportedSurface)?,
            );
        }
        let mut views: [Option<CpuView<'_>>; 3] = [None, None, None];
        for index in 0..surfaces.len() {
            let Some(owner) = owners[index] else { continue };
            if let Some(previous) = (0..index).find(|&previous| handles[previous] == handles[index])
            {
                surfaces[index] = surfaces[previous];
                continue;
            }
            // SAFETY: each distinct primary is resolved once before view creation;
            // the lease excludes all competing and recursive adapter access.
            let mut view = unsafe { CpuView::new(owner, &guard) }?;
            let area = view.area.as_ref().ok_or(SurfaceError::InvalidState)?;
            surfaces[index] = area.as_ptr();
            view.dirty = index == 0;
            views[index] = Some(view);
        }
        let result = draw(surfaces);
        let mut failure = None;
        for view in views.into_iter().flatten() {
            // SAFETY: the closure ended all pixel use; the guard remains live.
            if let Err(error) = unsafe { view.finish() } {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(result), Err)
    }

    unsafe fn adapter_attached(driver: NonNull<c_void>) -> bool {
        let mut current = FIRST.load(Ordering::Relaxed);
        while let Some(pointer) = NonNull::new(current) {
            // SAFETY: the driver lock retains all list nodes; exclusive hardware
            // entry excludes mutable attachment borrows, including GDI reentry.
            let device = unsafe { pointer.as_ref() };
            // SAFETY: the hardware lease excludes every attachment mutation.
            let attachment = unsafe { &*device.attachment.get() };
            if device.driver == driver
                && attachment
                    .as_ref()
                    .is_some_and(|value| value.session.is_some())
            {
                return true;
            }
            current = device.next.load(Ordering::Relaxed);
        }
        false
    }

    /// Attach the primary with the hooks supplied by the complete DDI table.
    /// # Safety
    /// Supply this driver's retained PDev at PASSIVE_LEVEL after complete_pdev.
    /// Every supplied hook must be registered and implemented. Keep the PDev
    /// live through all GDI/CPU users; mode/power operations use this same lock.
    /// A failure can retain hardware state: stop it before disabling the surface.
    pub unsafe fn enable_surface(
        handle: NonNull<c_void>,
        hooks: u32,
    ) -> Result<NonNull<c_void>, SurfaceError> {
        // SAFETY: the caller retains the live driver and passive GDI context.
        let _guard = unsafe { lock_hardware() }.ok_or(SurfaceError::Busy)?;
        // SAFETY: the caller retains this exact typed PDev through the call.
        let device = unsafe { Object::<Device>::borrow_handle(handle) };
        // SAFETY: this lease excludes all attachment mutations and reentry.
        if unsafe { &*device.attachment.get() }.is_some() {
            return Err(SurfaceError::InvalidState);
        }
        // SAFETY: the acquired hardware lease excludes all attachment users.
        if unsafe { adapter_attached(device.driver) } {
            return Err(SurfaceError::Busy);
        }
        let engine = NonNull::new(device.engine.load(Ordering::Acquire))
            .ok_or(SurfaceError::InvalidState)?;
        let mode = device.prepared.configuration();
        // SAFETY: no PDev owns an active generation of this adapter.
        let session = unsafe { Session::prepare(device) }?;
        // SAFETY: handle/device remain stable and hooks are supplied by the DDI
        // caller; hardware has not started and no GDI user sees this bitmap yet.
        let primary = unsafe { PrimaryBitmap::new(engine, handle, mode.layout, hooks) }
            .map_err(SurfaceError::Bitmap)?;
        let surface = primary.handle();
        // SAFETY: the exclusive hardware lease excludes recursive attachment
        // access. Retain all owners in the PDev before any host publication.
        let attachment = unsafe { &mut *device.attachment.get() };
        *attachment = Some(Attachment {
            _primary: primary,
            session: Some(session),
        });
        let session = attachment
            .as_mut()
            .and_then(|value| value.session.as_mut())
            .ok_or(SurfaceError::InvalidState)?;
        // SAFETY: PDev retains all mappings and the unique adapter session.
        unsafe { session.hardware.activate() }.map_err(SurfaceError::Hardware)?;
        Ok(surface)
    }

    /// Stop this PDev's hardware in the mode-assertion path.
    /// # Safety
    /// Supply the retained typed PDev at PASSIVE_LEVEL. Hide the cursor and stop
    /// GDI/CPU submissions first. Preserve all secondary pixels in RAM and stop
    /// their users before calling. Failure retains all owners for recovery.
    pub unsafe fn stop_surface(handle: NonNull<c_void>) -> Result<(), SurfaceError> {
        // SAFETY: the caller supplies the normal shutdown contract.
        unsafe { end_session(handle, false) }
    }

    /// Recreate a stopped generation while retaining the original GDI bitmap.
    /// # Safety
    /// Supply the live PDev at PASSIVE_LEVEL with GDI/CPU users stopped, after
    /// old session/token cleanup. Restore secondary surfaces separately before
    /// drawing resumes. Failed activation retains the new generation for recovery.
    pub unsafe fn resume_surface(handle: NonNull<c_void>) -> Result<(), SurfaceError> {
        // SAFETY: the caller supplies the live passive GDI driver context.
        let _guard = unsafe { lock_hardware() }.ok_or(SurfaceError::Busy)?;
        // SAFETY: this exact typed PDev remains live through the callback.
        let device = unsafe { Object::<Device>::borrow_handle(handle) };
        // SAFETY: the hardware lease excludes all attachment access and reentry.
        if unsafe { adapter_attached(device.driver) } {
            return Err(SurfaceError::Busy);
        }
        // SAFETY: exclusive entry excludes attachment borrows, including reentry.
        let attachment = unsafe { &mut *device.attachment.get() };
        let attachment = attachment.as_mut().ok_or(SurfaceError::InvalidState)?;
        // SAFETY: no PDev owns an active generation or live token on this adapter.
        attachment.session = Some(unsafe { Session::prepare(device) }?);
        let session = attachment
            .session
            .as_mut()
            .ok_or(SurfaceError::InvalidState)?;
        // SAFETY: the complete generation is retained before host publication.
        unsafe { session.hardware.activate() }.map_err(SurfaceError::Hardware)?;
        Ok(())
    }

    /// Explicitly reset a retained failed session; never a timeout fallback.
    /// # Safety
    /// Supply this driver's live PDev at PASSIVE_LEVEL with all adapter, power,
    /// GDI and CPU users stopped. Preserve required pixels first. The miniport's
    /// completed reset must stop host accesses and delayed completion writes.
    pub unsafe fn reset_surface(handle: NonNull<c_void>) -> Result<(), SurfaceError> {
        // SAFETY: the caller supplies the explicit reset/quiescence contract.
        unsafe { end_session(handle, true) }
    }

    unsafe fn end_session(handle: NonNull<c_void>, reset: bool) -> Result<(), SurfaceError> {
        // SAFETY: callers supply the live passive GDI driver context.
        let _guard = unsafe { lock_hardware() }.ok_or(SurfaceError::Busy)?;
        // SAFETY: the exact PDev stays live; the lease excludes recursive users.
        let device = unsafe { Object::<Device>::borrow_handle(handle) };
        // SAFETY: the hardware lease excludes every other attachment borrow.
        let attachment = unsafe { &mut *device.attachment.get() };
        let attachment = attachment.as_mut().ok_or(SurfaceError::InvalidState)?;
        let session = attachment
            .session
            .as_mut()
            .ok_or(SurfaceError::InvalidState)?;
        // SAFETY: each entry point requires stopped external users and preserved
        // pixels; reset additionally requires the miniport's quiescence guarantee.
        unsafe {
            if reset {
                session.hardware.reset()
            } else {
                session.hardware.stop()
            }
        }
        .map_err(SurfaceError::Hardware)?;
        if session.hardware.can_unmap() {
            drop(attachment.session.take());
        }
        Ok(())
    }

    /// Delete the primary and unmap only an already quiescent attachment.
    /// # Safety
    /// Supply the retained typed PDev at PASSIVE_LEVEL after mode shutdown,
    /// final pool-token cleanup and cessation of every GDI/CPU surface user.
    pub unsafe fn disable_surface(handle: NonNull<c_void>) -> Result<(), SurfaceError> {
        // SAFETY: the caller supplies the live passive GDI driver context.
        let _guard = unsafe { lock_hardware() }.ok_or(SurfaceError::Busy)?;
        // SAFETY: the caller retains the exact PDev and this lease excludes reentry.
        let device = unsafe { Object::<Device>::borrow_handle(handle) };
        // SAFETY: exclusive hardware entry owns this attachment's mutable state.
        let attachment = unsafe { &mut *device.attachment.get() };
        if attachment
            .as_ref()
            .and_then(|value| value.session.as_ref())
            .is_some_and(|value| !value.hardware.can_unmap())
        {
            return Err(SurfaceError::InvalidState);
        }
        drop(attachment.take());
        Ok(())
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
            driver,
            attachment: UnsafeCell::new(None),
            next: AtomicPtr::new(null_mut()),
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
        let pointer = core::ptr::from_ref(object.get()).cast_mut();
        object
            .get()
            .next
            .store(FIRST.load(Ordering::Relaxed), Ordering::Relaxed);
        FIRST.store(pointer, Ordering::Relaxed);
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
        let Some(_guard) = (unsafe { lock_hardware() }) else {
            return;
        };
        // SAFETY: the framework retains this exact typed PDev until recovery;
        // the hardware lease excludes attachment borrows and recursive teardown.
        let value = unsafe { Object::<Device>::borrow_handle(device) };
        // SAFETY: this hardware lease excludes mutable attachment access.
        if unsafe { &*value.attachment.get() }.is_some() {
            return;
        }
        let pointer = core::ptr::from_ref(value).cast_mut();
        let mut link = &FIRST;
        while let Some(current) = NonNull::new(link.load(Ordering::Relaxed)) {
            // SAFETY: the driver lock keeps every intrusive-list node live.
            let current_value = unsafe { current.as_ref() };
            if current.as_ptr() == pointer {
                link.store(
                    current_value.next.load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                break;
            }
            link = &current_value.next;
        }
        // SAFETY: GDI stopped all users and transfers its sole opaque ownership
        // back once. Object drops the complete value before the original heap block.
        drop(unsafe { Object::<Device>::from_handle(device) });
    }
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub use native::{
    SurfaceError, complete_pdev, disable_pdev, disable_surface, enable_pdev, enable_surface,
    initialise_driver, reset_surface, resume_surface, shutdown_driver, stop_surface,
};

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub(crate) use native::with_cpu_surfaces;

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn hardware_lease_rejects_reentry_and_other_threads_until_owner_drops() {
        let flag = AtomicBool::new(false);
        let owner = HardwareBorrow::acquire(&flag).unwrap();
        assert!(HardwareBorrow::acquire(&flag).is_none());
        // A rejected entry must not release the existing owner's lease.
        assert!(HardwareBorrow::acquire(&flag).is_none());
        let blocked = std::thread::scope(|scope| {
            scope
                .spawn(|| HardwareBorrow::acquire(&flag).is_none())
                .join()
                .unwrap()
        });
        assert!(blocked);
        drop(owner);
        let next = HardwareBorrow::acquire(&flag).unwrap();
        assert!(HardwareBorrow::acquire(&flag).is_none());
        drop(next);
        assert!(!flag.load(Ordering::Acquire));
    }

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
