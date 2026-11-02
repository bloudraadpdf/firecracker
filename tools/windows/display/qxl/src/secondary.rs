// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! PDev-owned non-primary bitmap callbacks and migration from display/surface.c.

use core::ffi::c_void;
use core::mem::{align_of, size_of};
use core::ptr::NonNull;

use crate::engine::Memory;
use crate::gdi::{DrawArea, Size};
use crate::hardware::{Hardware, State};
use crate::native_bitmap::{self, Context, DeviceBitmap, RamBitmap, SystemBitmap, VideoSurface};
use crate::pdev::SurfaceError;
use crate::surface::Layout;

#[derive(Debug)]
enum Bitmap {
    Video(DeviceBitmap<'static>),
    Ram(RamBitmap<'static>),
    System(SystemBitmap),
}

impl Bitmap {
    fn description(&self) -> native_bitmap::Description {
        match self {
            Self::Video(bitmap) => bitmap.description(),
            Self::Ram(bitmap) => bitmap.description(),
            Self::System(bitmap) => bitmap.description(),
        }
    }
}

#[derive(Debug)]
struct Entry {
    bitmap: Option<Bitmap>,
    retired: Option<VideoSurface<'static>>,
    deleting: bool,
}

impl Entry {
    fn empty() -> Self {
        Self {
            bitmap: None,
            retired: None,
            deleting: false,
        }
    }
    fn unused(&self) -> bool {
        !self.deleting && self.bitmap.is_none() && self.retired.is_none()
    }
}

#[derive(Debug)]
pub(crate) struct Bitmaps {
    memory: Option<Memory>,
    count: usize,
}

impl Bitmaps {
    /// # Safety
    /// Run at PASSIVE_LEVEL and retain this registry until all GDI bitmaps delete.
    pub(crate) unsafe fn new(surface_count: u32) -> Result<Self, SurfaceError> {
        let count = surface_count
            .checked_sub(1)
            .ok_or(SurfaceError::InvalidState)? as usize;
        if count == 0 {
            return Ok(Self {
                memory: None,
                count,
            });
        }
        let bytes = count
            .checked_mul(size_of::<Entry>())
            .filter(|n| *n <= isize::MAX as usize)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or(SurfaceError::InvalidState)?;
        // SAFETY: caller retains the passive GDI allocation context.
        let memory = unsafe { Memory::zeroed(bytes) }.map_err(SurfaceError::Memory)?;
        let base = memory.as_ptr().as_ptr().cast::<Entry>();
        if !base.addr().is_multiple_of(align_of::<Entry>()) {
            return Err(SurfaceError::InvalidState);
        }
        for index in 0..count {
            // SAFETY: the fresh aligned allocation covers every disjoint entry.
            unsafe { base.add(index).write(Entry::empty()) };
        }
        Ok(Self {
            memory: Some(memory),
            count,
        })
    }

    fn entries(&self) -> &[Entry] {
        match &self.memory {
            None => &[],
            // SAFETY: construction initialises all entries and this borrow retains them.
            Some(memory) => unsafe {
                core::slice::from_raw_parts(memory.as_ptr().as_ptr().cast(), self.count)
            },
        }
    }

    fn entries_mut(&mut self) -> &mut [Entry] {
        match &mut self.memory {
            None => &mut [],
            // SAFETY: this exclusive registry borrow covers its complete typed storage.
            Some(memory) => unsafe {
                core::slice::from_raw_parts_mut(memory.as_ptr().as_ptr().cast(), self.count)
            },
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries().iter().all(Entry::unused)
    }

    pub(crate) fn find_context(&self, context: NonNull<c_void>) -> Option<usize> {
        self.entries().iter().position(|entry| {
            entry
                .bitmap
                .as_ref()
                .is_some_and(|bitmap| bitmap.description().context == context)
        })
    }

    /// # Safety
    /// Hold the exclusive PDev/hardware lease and the live parent/GDI device.
    pub(crate) unsafe fn create(
        &mut self,
        hardware: &mut Hardware<'static>,
        engine: NonNull<c_void>,
        parent: NonNull<c_void>,
        size: Size,
        format: u32,
        hooks: u32,
    ) -> Result<Option<NonNull<c_void>>, SurfaceError> {
        let Some(slot) = hardware.video_slot() else {
            return Ok(None);
        };
        let count = hardware.secondary_count();
        let Some(index) = self
            .entries()
            .iter()
            .take(count.saturating_sub(1) as usize)
            .position(Entry::unused)
        else {
            return Ok(None);
        };
        let layout = match (u32::try_from(size.width), u32::try_from(size.height)) {
            (Ok(width), Ok(height)) if format == hardware.pixel_format().bitmap_format() => {
                Layout::new(width, height, hardware.pixel_format())
                    .map_err(|e| SurfaceError::Bitmap(native_bitmap::Error::Pool(e)))?
            }
            _ => return Ok(None),
        };
        let description = Context {
            parent,
            id: index as u32 + 1,
            layout,
        };
        let resources = hardware.resources().map_err(SurfaceError::Hardware)?;
        // SAFETY: the unused advertised ID, live video slot, parent and queue are retained.
        let (bitmap, notify) = unsafe {
            DeviceBitmap::new(
                &mut resources.pool,
                slot,
                &mut resources.commands,
                engine,
                description,
                hooks,
            )
        }
        .map_err(SurfaceError::Bitmap)?;
        self.entries_mut()[index].bitmap = Some(Bitmap::Video(bitmap));
        if notify {
            // SAFETY: the registry retained every published resource before notification.
            if let Err(error) = unsafe { hardware.notify(false) } {
                self.entries_mut()[index].deleting = true;
                return Err(SurfaceError::Hardware(error));
            }
        }
        let Some(Bitmap::Video(bitmap)) = self.entries_mut()[index].bitmap.as_mut() else {
            return Err(SurfaceError::InvalidState);
        };
        // SAFETY: publication succeeded; registry ownership now lasts until the deletion callback.
        Ok(unsafe { bitmap.hand_off() })
    }

    /// # Safety
    /// Exclude GDI/CPU users; preserve the bitmap's pool/ring generation through cleanup.
    pub(crate) unsafe fn delete(
        &mut self,
        index: usize,
        hardware: Option<&mut Hardware<'static>>,
    ) -> Result<(), SurfaceError> {
        let entry = self
            .entries_mut()
            .get_mut(index)
            .ok_or(SurfaceError::InvalidState)?;
        entry.deleting = true;
        let mut notify = false;
        if let Some(hardware) = hardware {
            let stopped = hardware.state() == State::Stopped;
            let result = (|| {
                let resources = hardware
                    .retained_resources()
                    .map_err(SurfaceError::Hardware)?;
                if let Some(retired) = entry.retired.take() {
                    // SAFETY: caller retains the generation; stopped proves host quiescence.
                    let result = unsafe {
                        if stopped {
                            retired.discard(&mut resources.pool).map(|()| false)
                        } else {
                            retired.destroy(&mut resources.pool, &mut resources.commands)
                        }
                    };
                    match result {
                        Ok(value) => notify |= value,
                        Err((error, retired)) => {
                            entry.retired = Some(retired);
                            return Err(SurfaceError::Bitmap(native_bitmap::Error::Pool(error)));
                        }
                    }
                }
                if let Some(bitmap) = entry.bitmap.take() {
                    // SAFETY: GDI deletion ended all bitmap users; every failure returns its owner.
                    let result = unsafe {
                        match bitmap {
                            Bitmap::Video(bitmap) if stopped => bitmap
                                .discard(&mut resources.pool)
                                .map(|()| false)
                                .map_err(|(e, b)| (e, Bitmap::Video(b))),
                            Bitmap::Video(bitmap) => bitmap
                                .destroy(&mut resources.pool, &mut resources.commands)
                                .map_err(|(e, b)| (e, Bitmap::Video(b))),
                            Bitmap::Ram(bitmap) if stopped => bitmap
                                .detach(&mut resources.pool)
                                .map(|bitmap| {
                                    drop(bitmap);
                                    false
                                })
                                .map_err(|(e, b)| (e, Bitmap::Ram(b))),
                            Bitmap::Ram(bitmap) => bitmap
                                .destroy(&mut resources.pool, &mut resources.commands)
                                .map_err(|(e, b)| (e, Bitmap::Ram(b))),
                            Bitmap::System(bitmap) => {
                                drop(bitmap);
                                Ok(false)
                            }
                        }
                    };
                    match result {
                        Ok(value) => notify |= value,
                        Err((error, bitmap)) => {
                            entry.bitmap = Some(bitmap);
                            return Err(SurfaceError::Bitmap(error));
                        }
                    }
                }
                Ok(())
            })();
            if notify && !stopped {
                // SAFETY: all preceding commands are retained on this generation's queue.
                unsafe { hardware.notify(false) }.map_err(SurfaceError::Hardware)?;
            }
            result?;
        } else {
            if entry.retired.is_some()
                || matches!(entry.bitmap, Some(Bitmap::Video(_) | Bitmap::Ram(_)))
            {
                return Err(SurfaceError::InvalidState);
            }
            drop(entry.bitmap.take());
        }
        entry.deleting = false;
        Ok(())
    }

    /// # Safety
    /// Stop all GDI users and retain the active generation through synchronous copies.
    pub(crate) unsafe fn suspend(
        &mut self,
        hardware: &mut Hardware<'static>,
    ) -> Result<(), SurfaceError> {
        for index in 0..self.count {
            if self.entries()[index].deleting {
                // SAFETY: the caller stopped all GDI users; retry deferred deletion.
                unsafe { self.delete(index, Some(hardware)) }?;
            }
            let entry = &mut self.entries_mut()[index];
            let Some(Bitmap::Video(bitmap)) = entry.bitmap.as_ref() else {
                continue;
            };
            let description = bitmap.description();
            let layout = description.info.layout;
            // SAFETY: synchronise the complete retained surface before reading its pixels.
            unsafe {
                hardware.update(
                    description.info.id,
                    [0, 0, layout.height() as i32, layout.width() as i32],
                )
            }
            .map_err(SurfaceError::Hardware)?;
            let Some(Bitmap::Video(bitmap)) = entry.bitmap.take() else {
                return Err(SurfaceError::InvalidState);
            };
            // SAFETY: the update completed and external GDI/CPU users remain stopped.
            match unsafe { bitmap.to_ram() } {
                Ok(bitmap) => entry.bitmap = Some(Bitmap::Ram(bitmap)),
                Err((error, bitmap)) => {
                    entry.bitmap = Some(Bitmap::Video(bitmap));
                    return Err(SurfaceError::Bitmap(error));
                }
            }
        }
        Ok(())
    }

    /// # Safety
    /// Hardware must be stopped; retain its old arenas until every detach succeeds.
    pub(crate) unsafe fn detach(
        &mut self,
        hardware: &mut Hardware<'static>,
    ) -> Result<(), SurfaceError> {
        if hardware.state() != State::Stopped {
            return Err(SurfaceError::InvalidState);
        }
        let resources = hardware
            .retained_resources()
            .map_err(SurfaceError::Hardware)?;
        for entry in self.entries_mut() {
            if let Some(retired) = entry.retired.take() {
                // SAFETY: the stopped state proves host and delayed release quiescence.
                if let Err((error, retired)) = unsafe { retired.discard(&mut resources.pool) } {
                    entry.retired = Some(retired);
                    return Err(SurfaceError::Bitmap(native_bitmap::Error::Pool(error)));
                }
            }
            if let Some(Bitmap::Ram(_)) = entry.bitmap {
                let Some(Bitmap::Ram(bitmap)) = entry.bitmap.take() else {
                    return Err(SurfaceError::InvalidState);
                };
                // SAFETY: hardware stopped, and the old mapping remains retained here.
                match unsafe { bitmap.detach(&mut resources.pool) } {
                    Ok(bitmap) => entry.bitmap = Some(Bitmap::System(bitmap)),
                    Err((error, bitmap)) => {
                        entry.bitmap = Some(Bitmap::Ram(bitmap));
                        return Err(SurfaceError::Bitmap(error));
                    }
                }
            }
        }
        Ok(())
    }

    /// # Safety
    /// Hold the exclusive PDev lease with all GDI users stopped during mode resume.
    pub(crate) unsafe fn restore(
        &mut self,
        hardware: &mut Hardware<'static>,
    ) -> Result<(), SurfaceError> {
        let Some(slot) = hardware.video_slot() else {
            return Ok(());
        };
        let count = hardware.secondary_count();
        for index in 0..self.count.min(count.saturating_sub(1) as usize) {
            let entry = &mut self.entries_mut()[index];
            if entry.deleting || entry.retired.is_some() {
                continue;
            }
            if !matches!(entry.bitmap, Some(Bitmap::System(_))) {
                continue;
            }
            let resources = hardware.resources().map_err(SurfaceError::Hardware)?;
            let Some(Bitmap::System(bitmap)) = entry.bitmap.take() else {
                return Err(SurfaceError::InvalidState);
            };
            // SAFETY: the ID is valid in the new generation; RAM/GDI ownership survives failure.
            let notify =
                match unsafe { bitmap.restore(&mut resources.pool, slot, &mut resources.commands) }
                {
                    Ok((bitmap, notify)) => {
                        entry.bitmap = Some(Bitmap::Video(bitmap));
                        notify
                    }
                    Err(failure) => {
                        entry.bitmap = Some(Bitmap::System(failure.bitmap));
                        entry.retired = failure.retired.map(|(_, surface)| surface);
                        failure.notify
                    }
                };
            if notify {
                // SAFETY: both success and compensation commands have retained owners.
                unsafe { hardware.notify(false) }.map_err(SurfaceError::Hardware)?;
            }
        }
        Ok(())
    }

    /// # Safety
    /// Retain the exclusive PDev/hardware lease and end the view before it is released.
    pub(crate) unsafe fn cpu_view<'a>(
        &mut self,
        handle: *mut c_void,
        hardware: Option<&mut Hardware<'static>>,
    ) -> Result<Option<DrawArea<'a>>, SurfaceError> {
        let Some(bitmap) = self
            .entries()
            .iter()
            .filter(|entry| !entry.deleting)
            .filter_map(|entry| entry.bitmap.as_ref())
            .find(|bitmap| bitmap.description().handle.as_ptr() == handle)
        else {
            return Ok(None);
        };
        let description = bitmap.description();
        let layout = description.info.layout;
        if matches!(bitmap, Bitmap::Video(_)) {
            let hardware = hardware.ok_or(SurfaceError::InvalidState)?;
            // SAFETY: the complete live video surface is retained under this lease.
            unsafe {
                hardware.update(
                    description.info.id,
                    [0, 0, layout.height() as i32, layout.width() as i32],
                )
            }
            .map_err(SurfaceError::Hardware)?;
        }
        let pixels = NonNull::new(description.pixels).ok_or(SurfaceError::InvalidState)?;
        // SAFETY: the registry retains the backing and the caller's lease excludes
        // migration, deletion and competing views through the returned lifetime.
        unsafe { DrawArea::from_raw(description.engine, pixels, layout) }
            .map(Some)
            .map_err(|error| SurfaceError::Bitmap(native_bitmap::Error::Gdi(error)))
    }
}

impl Drop for Bitmaps {
    fn drop(&mut self) {
        assert!(
            self.is_empty(),
            "QXL secondary registry dropped with live bitmaps"
        );
        for entry in self.entries_mut() {
            // SAFETY: every typed entry is initialised and has no remaining owners.
            unsafe { core::ptr::drop_in_place(entry) };
        }
    }
}
