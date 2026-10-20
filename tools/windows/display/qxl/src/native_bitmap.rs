// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Native GDI binding and system/video migration from display/surface.c.

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::ptr::NonNull;

use crate::display_resources::{Error as PoolError, Pool};
use crate::kernel_object::Object;
use crate::ring::CommandRing;
use crate::surface::{Active, Layout, Surface};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    #[cfg(windows)]
    Gdi(crate::gdi::Error),
    Pool(PoolError),
    Memory(u32),
    Modify,
    InvalidState,
}

/// Immutable metadata behind the DHSURF of both primary and secondary bitmaps.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub parent: NonNull<c_void>,
    pub id: u32,
    pub layout: Layout,
}

impl Context {
    /// # Safety
    /// Supply this driver's live DHSURF, retaining the bitmap and its parent
    /// PDev through this call. It must have this exact context type.
    pub unsafe fn from_handle(handle: NonNull<c_void>) -> Self {
        // SAFETY: the caller retains the exact typed callback context.
        *unsafe { Object::<Self>::borrow_handle(handle) }
    }
}

#[derive(Debug)]
struct VideoStorage<'mapping>(Object<UnsafeCell<Option<Surface<'mapping, Active>>>>);

impl<'mapping> VideoStorage<'mapping> {
    fn fill(self, surface: Surface<'mapping, Active>) -> VideoSurface<'mapping> {
        // SAFETY: private storage is either fresh or was emptied by its unique
        // consuming owner. No reference/callback can access its value here.
        unsafe { *self.0.get().get() = Some(surface) };
        VideoSurface(self)
    }
}

/// Heap-owned video state retained through normal or compensating destruction.
#[derive(Debug)]
#[must_use]
pub struct VideoSurface<'mapping>(VideoStorage<'mapping>);

impl<'mapping> VideoSurface<'mapping> {
    fn get(&self) -> &Surface<'mapping, Active> {
        // SAFETY: only fill creates this type; consuming operations restore it
        // before returning on failure. A borrow prevents a consuming mutation.
        unsafe { &*self.0.0.get().get() }
            .as_ref()
            .expect("live video state")
    }

    /// # Safety
    /// Stop GDI/CPU users and preserve the same pool/ring generation and FIFO
    /// as all surface uses. Keep the returned owner on failure for retry.
    pub unsafe fn destroy(
        self,
        pool: &mut Pool<'mapping>,
        ring: &mut CommandRing,
    ) -> Result<bool, (PoolError, Self)> {
        // SAFETY: this consumed unique owner excludes all value borrows.
        let surface = unsafe { &mut *self.0.0.get().get() }
            .take()
            .expect("live video state");
        // SAFETY: the caller supplies the surface destruction/ordering contract.
        match unsafe { surface.destroy(pool, ring) } {
            Ok(notify) => Ok(notify),
            Err((error, surface)) => Err((error, self.0.fill(surface))),
        }
    }

    /// # Safety
    /// Prove old host accesses and delayed release writes stopped; retain the
    /// old pool/mapping through cleanup. A timeout does not prove quiescence.
    pub unsafe fn discard(self, pool: &mut Pool<'mapping>) -> Result<(), (PoolError, Self)> {
        // SAFETY: this consumed unique owner excludes all value borrows.
        let surface = unsafe { &mut *self.0.0.get().get() }
            .take()
            .expect("live video state");
        // SAFETY: the caller proves hardware quiescence before final release.
        match unsafe { surface.discard(pool) } {
            Ok(()) => Ok(()),
            Err((error, surface)) => Err((error, self.0.fill(surface))),
        }
    }
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
mod native {
    use super::*;
    use crate::engine::{EngModifySurface, Memory, allocate_object};
    use crate::gdi::{self, Bitmap, DrawArea, SurfaceObject};
    use crate::slots::MemorySlot;
    use core::num::NonZeroU32;
    use core::ptr::copy_nonoverlapping;

    unsafe fn video_storage<'mapping>() -> Result<VideoStorage<'mapping>, Error> {
        // SAFETY: callers retain the passive-level GDI context; allocate metadata
        // before publication, so error recovery needs no new heap allocation.
        unsafe { allocate_object(UnsafeCell::new(None)) }
            .map(VideoStorage)
            .map_err(Error::Memory)
    }

    #[derive(Debug)]
    struct Handle {
        owned: Option<Bitmap>,
        value: NonNull<c_void>,
    }

    impl Handle {
        fn new(bitmap: Bitmap) -> Self {
            Self {
                value: bitmap.handle(),
                owned: Some(bitmap),
            }
        }

        fn hand_off(&mut self) -> Option<NonNull<c_void>> {
            self.owned.take()?.into_raw();
            Some(self.value)
        }
    }

    #[derive(Debug)]
    struct Binding {
        // Delete an owned GDI handle before its borrowed callback context.
        handle: Handle,
        context: Object<Context>,
        engine: NonNull<c_void>,
        hooks: u32,
    }

    impl Binding {
        unsafe fn new(
            engine: NonNull<c_void>,
            description: Context,
            hooks: u32,
        ) -> Result<Self, Error> {
            // SAFETY: the caller retains the parent/device through every GDI user;
            // the heap object is fully initialised before GDI receives its handle.
            let context = unsafe { allocate_object(description) }.map_err(Error::Memory)?;
            // SAFETY: the stable context outlives bitmap deletion, including failure
            // cleanup. The caller supplies live device and registered hooks.
            let bitmap =
                unsafe { Bitmap::device(engine, context.handle(), description.layout, hooks) }
                    .map_err(Error::Gdi)?;
            Ok(Self {
                handle: Handle::new(bitmap),
                context,
                engine,
                hooks,
            })
        }

        unsafe fn modify(&self, pixels: Option<NonNull<u8>>) -> Result<(), Error> {
            let layout = self.context.get().layout;
            let (first_scan, stride, hooks, flags) = match pixels {
                Some(base) => {
                    let (first, stride) = gdi::bottom_up(base, layout);
                    (first, stride, 0, 0)
                }
                None => (core::ptr::null_mut(), 0, self.hooks, 1), // MS_NOTSYSTEMMEMORY
            };
            // SAFETY: only secondary bitmaps use this method. The live handle,
            // device/context and optional full CPU backing remain retained through
            // GDI access. The caller excludes simultaneous surface users.
            let result = unsafe {
                EngModifySurface(
                    self.handle.value.as_ptr(),
                    self.engine.as_ptr(),
                    hooks,
                    flags,
                    self.context.handle().as_ptr(),
                    first_scan,
                    stride,
                    core::ptr::null_mut(),
                )
            };
            if result == 0 {
                Err(Error::Modify)
            } else {
                Ok(())
            }
        }
    }

    /// Primary handles never use EngModifySurface after publication.
    #[derive(Debug)]
    pub struct PrimaryBitmap(Binding);

    impl PrimaryBitmap {
        /// # Safety
        /// Create/use/destroy at PASSIVE_LEVEL in serialised GDI callbacks. Retain
        /// the parent PDev and live GDI device through handle deletion. Every hook
        /// must have a registered implementation and support constructor cleanup.
        pub unsafe fn new(
            engine: NonNull<c_void>,
            parent: NonNull<c_void>,
            layout: Layout,
            hooks: u32,
        ) -> Result<Self, Error> {
            // SAFETY: the caller supplies the live retained GDI device/context.
            unsafe {
                Binding::new(
                    engine,
                    Context {
                        parent,
                        id: 0,
                        layout,
                    },
                    hooks,
                )
            }
            .map(Self)
        }

        /// Borrow the primary handle; its Rust owner retains deletion ownership.
        pub fn handle(&self) -> NonNull<c_void> {
            self.0.handle.value
        }
    }

    #[derive(Debug)]
    #[must_use]
    pub struct DeviceBitmap<'mapping> {
        // Unlock/delete the private draw area before bitmap/context/backing cleanup.
        draw_area: DrawArea<'mapping>,
        binding: Binding,
        surface: VideoSurface<'mapping>,
    }

    impl<'mapping> DeviceBitmap<'mapping> {
        /// # Safety
        /// Retain the parent/device/mappings and serialise all GDI/pool/ring access
        /// at PASSIVE_LEVEL. Use an unused ID below the advertised surface count,
        /// the active video slot and matching command-ring generation. Hooks must
        /// be fully implemented. Stop GDI users before destruction; destroy/release
        /// the video surface explicitly before resetting or unmapping its arenas.
        pub unsafe fn new(
            pool: &mut Pool<'mapping>,
            slot: MemorySlot,
            ring: &mut CommandRing,
            engine: NonNull<c_void>,
            description: Context,
            hooks: u32,
        ) -> Result<(Self, bool), Error> {
            let id = NonZeroU32::new(description.id).ok_or(Error::InvalidState)?;
            let layout = description.layout;
            // SAFETY: the caller retains the passive-level GDI allocation context.
            let storage = unsafe { video_storage() }?;
            let prepared = Surface::allocate(pool, slot, id, layout).map_err(Error::Pool)?;
            // SAFETY: the caller supplies live parent/device and supported hooks.
            let binding = match unsafe { Binding::new(engine, description, hooks) } {
                Ok(binding) => binding,
                Err(error) => {
                    prepared
                        .cancel(pool)
                        .expect("new bitmap resource ownership");
                    return Err(error);
                }
            };
            // SAFETY: the sole unpublished resource owns this complete stable pixel
            // allocation. The draw area drops before any rollback or final release.
            let area = unsafe {
                DrawArea::from_raw(
                    engine,
                    NonNull::new(prepared.as_ptr()).expect("surface allocation"),
                    layout,
                )
            };
            let draw_area = match area {
                Ok(area) => area,
                Err(error) => {
                    drop(binding);
                    prepared
                        .cancel(pool)
                        .expect("new bitmap resource ownership");
                    return Err(Error::Gdi(error));
                }
            };
            // SAFETY: the caller supplies the live mapping/slot/ring relationship;
            // no caller has received the GDI handle or any CPU view yet.
            match unsafe { prepared.publish(pool, ring, false) } {
                Ok((surface, notify)) => Ok((
                    Self {
                        draw_area,
                        binding,
                        surface: storage.fill(surface),
                    },
                    notify,
                )),
                Err((error, prepared)) => {
                    drop(draw_area);
                    drop(binding);
                    prepared
                        .cancel(pool)
                        .expect("new bitmap resource ownership");
                    Err(Error::Pool(error))
                }
            }
        }

        /// Borrow the live GDI handle without changing ownership.
        pub fn handle(&self) -> NonNull<c_void> {
            self.binding.handle.value
        }

        /// Borrow the private software view; synchronise host writes before use.
        pub fn draw_area(&self) -> *mut SurfaceObject {
            self.draw_area.as_ptr()
        }

        /// # Safety
        /// Transfer the handle once to GDI from DrvCreateDeviceBitmap. Retain this
        /// Rust object until DrvDeleteDeviceBitmap, then stop all GDI users before
        /// destroying it. GDI owns handle deletion after this transfer.
        pub unsafe fn hand_off(&mut self) -> Option<NonNull<c_void>> {
            self.binding.handle.hand_off()
        }

        /// # Safety
        /// Stop every GDI/CPU user and serialise pool/ring access. Queue destruction
        /// on the same generation/FIFO as creation and drawing. On error, retain the
        /// returned owner for retry; a GDI-deleted handle must not be used again.
        pub unsafe fn destroy(
            self,
            pool: &mut Pool<'mapping>,
            ring: &mut CommandRing,
        ) -> Result<bool, (Error, Self)> {
            let Self {
                draw_area,
                binding,
                surface,
            } = self;
            // SAFETY: the caller stopped users and preserves queue ordering. Pool
            // exclusivity prevents reclaiming the destroy reference until GDI drops.
            match unsafe { surface.destroy(pool, ring) } {
                Ok(notify) => {
                    drop(draw_area);
                    drop(binding);
                    Ok(notify)
                }
                Err((error, surface)) => Err((
                    Error::Pool(error),
                    Self {
                        draw_area,
                        binding,
                        surface,
                    },
                )),
            }
        }

        /// Switch GDI to RAM without prematurely releasing old video memory.
        /// # Safety
        /// Synchronise prior host writes and stop new drawing to this surface.
        /// Exclude all GDI/CPU/pool users at PASSIVE_LEVEL. Keep the returned RAM
        /// owner through GDI use and hardware shutdown; no reset/unmap before detach.
        pub unsafe fn to_ram(self) -> Result<RamBitmap<'mapping>, (Error, Self)> {
            let layout = self.binding.context.get().layout;
            // SAFETY: checked Layout bytes fit u32 and this is a passive GDI context.
            let pixels = match unsafe { Memory::zeroed(layout.bytes() as u32) } {
                Ok(pixels) => pixels,
                Err(error) => return Err((Error::Memory(error), self)),
            };
            // SAFETY: host writes are synchronised; source pixels and the fresh
            // disjoint CPU allocation are valid for the complete checked extent.
            unsafe {
                copy_nonoverlapping(
                    self.surface.get().as_ptr(),
                    pixels.as_ptr().as_ptr(),
                    layout.bytes(),
                )
            };
            // SAFETY: the new allocation survives through GDI use on success; failed
            // modification leaves the original device bitmap and backing unchanged.
            if let Err(error) = unsafe { self.binding.modify(Some(pixels.as_ptr())) } {
                return Err((error, self));
            }
            let Self {
                draw_area,
                binding,
                surface,
            } = self;
            drop(draw_area);
            Ok(RamBitmap {
                binding,
                pixels,
                retired: surface,
            })
        }
    }

    #[derive(Debug)]
    #[must_use]
    pub struct RamBitmap<'mapping> {
        binding: Binding,
        pixels: Memory,
        retired: VideoSurface<'mapping>,
    }

    impl<'mapping> RamBitmap<'mapping> {
        /// Release old video ownership only after hardware quiescence.
        /// # Safety
        /// Stop all host accesses and delayed release writes for the old generation,
        /// and retain that pool/mapping through cleanup. Serialise GDI metadata use.
        pub unsafe fn detach(
            self,
            pool: &mut Pool<'mapping>,
        ) -> Result<SystemBitmap, (Error, Self)> {
            let Self {
                binding,
                pixels,
                retired,
            } = self;
            // SAFETY: the caller proves actual old-generation hardware quiescence,
            // retaining mappings through release of this remaining surface reference.
            match unsafe { retired.discard(pool) } {
                Ok(()) => Ok(SystemBitmap { binding, pixels }),
                Err((error, retired)) => Err((
                    Error::Pool(error),
                    Self {
                        binding,
                        pixels,
                        retired,
                    },
                )),
            }
        }
    }

    /// A GDI RAM bitmap with no remaining dependency on an old video mapping.
    #[derive(Debug)]
    #[must_use]
    pub struct SystemBitmap {
        binding: Binding,
        pixels: Memory,
    }

    #[derive(Debug)]
    #[must_use]
    pub struct RestoreFailure<'mapping> {
        pub error: Error,
        pub bitmap: SystemBitmap,
        /// Retain/retry destruction if the compensating destroy could not be queued.
        pub retired: Option<(PoolError, VideoSurface<'mapping>)>,
        /// Notify the command ring even when the restore operation failed.
        pub notify: bool,
    }

    impl SystemBitmap {
        /// Borrow the live RAM-backed GDI handle.
        pub fn handle(&self) -> NonNull<c_void> {
            self.binding.handle.value
        }

        /// Restore into a new generation while keeping the RAM bitmap on failure.
        /// # Safety
        /// Stop GDI/CPU users and retain the live parent/device at PASSIVE_LEVEL.
        /// Supply the new pool/video slot/command ring with this surface's unused
        /// valid ID. Retain every returned candidate until host destruction/release;
        /// forward notification flags on both success and failure.
        pub unsafe fn restore<'mapping>(
            self,
            pool: &mut Pool<'mapping>,
            slot: MemorySlot,
            ring: &mut CommandRing,
        ) -> Result<(DeviceBitmap<'mapping>, bool), RestoreFailure<'mapping>> {
            let context = *self.binding.context.get();
            let failure = |error, bitmap| RestoreFailure {
                error,
                bitmap,
                retired: None,
                notify: false,
            };
            let Some(id) = NonZeroU32::new(context.id) else {
                return Err(failure(Error::InvalidState, self));
            };
            // SAFETY: the caller retains the passive-level GDI allocation context.
            let storage = match unsafe { video_storage() } {
                Ok(storage) => storage,
                Err(error) => return Err(failure(error, self)),
            };
            let mut prepared = match Surface::allocate(pool, slot, id, context.layout) {
                Ok(surface) => surface,
                Err(error) => return Err(failure(Error::Pool(error), self)),
            };
            // SAFETY: GDI users are stopped and the fresh video resource is private;
            // the retained CPU allocation has exactly this bitmap's checked extent.
            unsafe {
                copy_nonoverlapping(
                    self.pixels.as_ptr().as_ptr(),
                    prepared.pixels().as_mut_ptr(),
                    context.layout.bytes(),
                )
            };
            // SAFETY: this unpublished allocation remains owned through draw-area
            // deletion, including every failed publication/modify path below.
            let area = unsafe {
                DrawArea::from_raw(
                    self.binding.engine,
                    NonNull::new(prepared.as_ptr()).expect("surface allocation"),
                    context.layout,
                )
            };
            let draw_area = match area {
                Ok(area) => area,
                Err(error) => {
                    prepared
                        .cancel(pool)
                        .expect("restored bitmap resource ownership");
                    return Err(failure(Error::Gdi(error), self));
                }
            };
            // SAFETY: the copied pixels are complete and the new ID/generation is
            // available. KEEP_DATA preserves the copied RAM contents on creation.
            let (surface, notify) = match unsafe { prepared.publish(pool, ring, true) } {
                Ok(result) => result,
                Err((error, prepared)) => {
                    drop(draw_area);
                    prepared
                        .cancel(pool)
                        .expect("restored bitmap resource ownership");
                    return Err(failure(Error::Pool(error), self));
                }
            };
            let surface = storage.fill(surface);
            // SAFETY: this is a secondary bitmap; all GDI users are stopped and
            // the new host surface/pixels are retained before GDI switches to them.
            if let Err(error) = unsafe { self.binding.modify(None) } {
                drop(draw_area);
                // SAFETY: no GDI view uses candidate video pixels; its create and
                // compensating destroy use the same FIFO and retained generation.
                return Err(match unsafe { surface.destroy(pool, ring) } {
                    Ok(destroy_notify) => RestoreFailure {
                        error,
                        bitmap: self,
                        retired: None,
                        notify: notify || destroy_notify,
                    },
                    Err((retirement_error, surface)) => RestoreFailure {
                        error,
                        bitmap: self,
                        retired: Some((retirement_error, surface)),
                        notify,
                    },
                });
            }
            let Self { binding, pixels } = self;
            // GDI is opaque again; no user retains the old CPU pixel address.
            drop(pixels);
            Ok((
                DeviceBitmap {
                    draw_area,
                    binding,
                    surface,
                },
                notify,
            ))
        }
    }
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub use native::{DeviceBitmap, PrimaryBitmap, RamBitmap, RestoreFailure, SystemBitmap};

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::display_resources::test_utils::{Bytes, HostRing, empty, pool};
    use crate::slots::{MemorySlot, SlotLayout};
    use crate::surface::Format;
    use core::alloc::Layout as AllocationLayout;
    use core::num::NonZeroU32;

    type State<'a> = UnsafeCell<Option<Surface<'a, Active>>>;

    fn storage<'a>() -> VideoStorage<'a> {
        let bytes = Object::<State<'a>>::allocation_bytes().unwrap();
        let layout = AllocationLayout::from_size_align(bytes, 8).unwrap();
        // SAFETY: this test allocator supplies a complete uniquely owned extent
        // and the typed release function below reclaims the same exact layout.
        let pointer = NonNull::new(unsafe { std::alloc::alloc(layout) }).unwrap();
        // SAFETY: Object checks placement/alignment before initialising its value.
        VideoStorage(
            unsafe { Object::new_in(pointer, bytes, release, UnsafeCell::new(None)) }.unwrap(),
        )
    }

    unsafe extern "system" fn release(pointer: *mut c_void) {
        let bytes = Object::<State<'static>>::allocation_bytes().unwrap();
        let layout = AllocationLayout::from_size_align(bytes, 8).unwrap();
        // SAFETY: storage retained this exact allocation and drops its complete
        // value before invoking the matching monomorphised deallocator.
        unsafe { std::alloc::dealloc(pointer.cast(), layout) };
    }

    fn video_slot(memory: &mut [u64]) -> MemorySlot {
        SlotLayout::new(8, 8)
            .unwrap()
            .slot(
                2,
                7,
                0xf0000000,
                memory.as_ptr().addr() as u64,
                size_of_val(memory) as u64,
            )
            .unwrap()
    }

    #[test]
    fn failed_destroy_restores_the_same_heap_owned_video_state_for_retry() {
        let mut memory = [0u64; 1024];
        let mut video = [0u64; 64];
        let slot = video_slot(&mut video);
        let mut pool = pool(&mut memory, Some(&mut video));
        let mut command_bytes = Bytes([0; 532]);
        let host = HostRing::new(&mut command_bytes, 32, 16);
        // SAFETY: the live ring has one guest endpoint for this test generation.
        let mut ring = unsafe { CommandRing::new(host.base, 532).unwrap() };
        let prepared = Surface::allocate(
            &mut pool,
            slot,
            NonZeroU32::new(7).unwrap(),
            Layout::new(4, 4, Format::Xrgb8888).unwrap(),
        )
        .unwrap();
        let storage = storage();
        // SAFETY: pool, video slot and ring share this active mapped generation.
        let (surface, _) = unsafe { prepared.publish(&mut pool, &mut ring, true) }.unwrap();
        let owner = storage.fill(surface);
        let address = owner.get().as_ptr();
        host.set(4, 32);
        // SAFETY: all GDI/CPU users are stopped; full-ring failure must preserve
        // the same surface reference without a second heap allocation.
        let (error, owner) = unsafe { owner.destroy(&mut pool, &mut ring) }.unwrap_err();
        assert_eq!(error, PoolError::RingFull);
        assert_eq!(owner.get().as_ptr(), address);
        assert_eq!(owner.get().id(), 7);
        host.set(4, 1);
        host.set(12, 1);
        // SAFETY: the same FIFO/generation now has space for destruction.
        unsafe { owner.destroy(&mut pool, &mut ring) }.unwrap();
        // SAFETY: the simulated host stops all access without release writes.
        unsafe { pool.discard_pending() };
        empty(&mut pool);
    }

    #[test]
    fn wrong_pool_discard_retains_the_video_owner_until_quiescent_cleanup() {
        let mut memory = [0u64; 1024];
        let mut video = [0u64; 64];
        let slot = video_slot(&mut video);
        let mut pool = pool(&mut memory, Some(&mut video));
        let mut command_bytes = Bytes([0; 532]);
        let host = HostRing::new(&mut command_bytes, 32, 16);
        // SAFETY: this live mapping has one guest endpoint.
        let mut ring = unsafe { CommandRing::new(host.base, 532).unwrap() };
        let prepared = Surface::allocate(
            &mut pool,
            slot,
            NonZeroU32::new(9).unwrap(),
            Layout::new(4, 4, Format::Xrgb8888).unwrap(),
        )
        .unwrap();
        // SAFETY: these are the matching active device mappings and generation.
        let (surface, _) = unsafe { prepared.publish(&mut pool, &mut ring, false) }.unwrap();
        let owner = storage().fill(surface);
        // SAFETY: the simulated host stops all work before old-generation cleanup.
        unsafe { pool.discard_pending() };
        let mut foreign_memory = [0u64; 128];
        let mut foreign = self::pool(&mut foreign_memory, None);
        // SAFETY: host work stopped; wrong-pool rejection must retain ownership.
        let (error, owner) = unsafe { owner.discard(&mut foreign) }.unwrap_err();
        assert_eq!(error, PoolError::WrongPool);
        assert_eq!(owner.get().id(), 9);
        empty(&mut foreign);
        // SAFETY: the original pool/mapping remains live and host access stopped.
        unsafe { owner.discard(&mut pool) }.unwrap();
        empty(&mut pool);
    }
}
