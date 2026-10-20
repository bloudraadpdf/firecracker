// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Mapped native hardware lifetime from display/driver.c and display/res.c.

use core::ptr::NonNull;

use crate::control;
use crate::display_device::{self, Interface, Region};
use crate::display_resources::{self, Pool};
use crate::driver_info::DriverInfo;
use crate::ring::{CommandRing, ReleaseRing, RingError};
use crate::slots::{MemorySlot, SlotError};
use crate::surface::Layout;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Mapping(display_device::Error),
    Ring(RingError),
    Pool(display_resources::Error),
    Control(control::Error),
    Slot(SlotError),
    Ioctl(u32),
    InvalidState,
    OutstandingOutputs,
    #[cfg(windows)]
    Gdi(crate::gdi::Error),
}

/// Ring endpoints and allocation state for one mapped device generation.
#[derive(Debug)]
pub struct Resources<'mapping> {
    pub pool: Pool<'mapping>,
    pub commands: CommandRing,
    pub cursor: CommandRing,
    releases: ReleaseRing,
    interface: Interface,
}

impl<'mapping> Resources<'mapping> {
    /// # Safety
    /// The interface and framebuffer must describe these live miniport mappings
    /// for 'mapping. Take exclusive ownership of all three guest ring endpoints
    /// and both allocation arenas, with no old outstanding commands/resources.
    /// Serialise every access and retain mappings until all tokens are released
    /// and hardware/host users stop. The primary pixel area is not an arena.
    pub unsafe fn new(
        info: &'mapping DriverInfo,
        layout: Layout,
        framebuffer: Region,
    ) -> Result<Self, Error> {
        let interface = Interface::validate(info, layout).map_err(Error::Mapping)?;
        let pointer = |pointer| {
            NonNull::new(pointer).ok_or(Error::Mapping(display_device::Error::InvalidRange))
        };
        // SAFETY: validation checks each ring's complete fixed header geometry;
        // the caller supplies live mappings and exclusive guest endpoints.
        let (commands, cursor, releases) = unsafe {
            (
                CommandRing::new(pointer(info.command_ring.cast())?, CommandRing::BYTES)
                    .map_err(Error::Ring)?,
                CommandRing::new(pointer(info.cursor_ring.cast())?, CommandRing::BYTES)
                    .map_err(Error::Ring)?,
                ReleaseRing::new(pointer(info.release_ring.cast())?, ReleaseRing::BYTES)
                    .map_err(Error::Ring)?,
            )
        };
        let pages = interface.pages();
        let video = interface
            .secondary_slot()
            .map(|_| (framebuffer.base(), framebuffer.len()));
        // SAFETY: validation excludes primary pixels/control records from the
        // IO arena; the caller supplies the disjoint live video mapping. Neither
        // arena has old users, so writing initial free-block headers is valid.
        let pool = unsafe { Pool::new(pages.base(), pages.len(), video, interface.main_slot()) }
            .map_err(Error::Pool)?;
        Ok(Self {
            pool,
            commands,
            cursor,
            releases,
            interface,
        })
    }

    /// Allocation-slot address conversion for this mapping generation.
    pub fn main_slot(&self) -> MemorySlot {
        self.interface.main_slot()
    }

    /// # Safety
    /// The host must publish completed releases for this generation only, after
    /// all accesses to each command and its resources stop. Retain mappings.
    pub unsafe fn reap(&mut self, budget: usize) -> Result<display_resources::Reclaimed, Error> {
        // SAFETY: this owner retains the unique release endpoint and the caller
        // supplies the host-completion/mapping contract.
        unsafe { self.pool.reap(&mut self.releases, budget) }.map_err(Error::Pool)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Prepared,
    Active,
    Stopped,
    Failed,
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
mod native {
    use super::*;
    use crate::control::{Control, Operation};
    use crate::display_device::MappedDevice;
    use crate::engine::EngDeviceIoControl;
    use crate::ioctl::RESET_DEVICE;
    use crate::memory::REVISION_ASYNC;
    use crate::slots::SlotLayout;
    use core::ffi::c_void;

    #[derive(Debug)]
    #[must_use]
    pub struct Hardware<'mapping> {
        device: &'mapping MappedDevice,
        resources: Resources<'mapping>,
        control: Control<'mapping>,
        layout: Layout,
        video_slot: Option<MemorySlot>,
        state: State,
        monitors: Option<crate::display_resources::Resource<'mapping>>,
        images: crate::image_cache::Images<'mapping>,
    }

    impl<'mapping> Hardware<'mapping> {
        /// # Safety
        /// The mapped device must be the sole active hardware context for this
        /// physical adapter, shared across all its PDevs. Run every operation at
        /// PASSIVE_LEVEL with exclusive adapter access, including mode and power
        /// callbacks. The layout must match this device's selected mode.
        /// No old commands, resources or CPU pixel users may remain.
        /// Stop all hardware access and release all tokens before destroying
        /// this owner or its mapping; errors and Drop do not stop hardware.
        pub unsafe fn prepare(
            device: &'mapping MappedDevice,
            layout: Layout,
        ) -> Result<Self, Error> {
            let cache_limit = layout.bytes().saturating_add(layout.bytes() / 2);
            // SAFETY: preparation runs in the retained passive GDI context.
            let images =
                unsafe { crate::image_cache::Images::new(cache_limit) }.map_err(Error::Ioctl)?;
            // SAFETY: the caller transfers the sole adapter generation's arenas
            // and guest endpoints, retaining the device through the result.
            let resources = unsafe { Resources::new(device.info(), layout, device.framebuffer()) }?;
            // SAFETY: this context is unique across the physical adapter and
            // no previous control command remains outstanding.
            let control = unsafe { Control::new(device) };
            Ok(Self {
                device,
                resources,
                control,
                layout,
                video_slot: None,
                state: State::Prepared,
                monitors: None,
                images,
            })
        }

        pub(crate) fn secondary_count(&self) -> u32 {
            let info = self.device.info();
            if info.create_non_primary_surfaces != 0 {
                info.surface_count
            } else {
                1
            }
        }

        pub(crate) fn pixel_format(&self) -> crate::surface::Format {
            self.layout.format()
        }

        pub(crate) fn retained_resources(&mut self) -> Result<&mut Resources<'mapping>, Error> {
            if !matches!(self.state, State::Active | State::Stopped) {
                return Err(Error::InvalidState);
            }
            Ok(&mut self.resources)
        }

        /// Current activation/shutdown state.
        pub fn state(&self) -> State {
            self.state
        }
        /// The acknowledged video slot, if registration has completed.
        pub fn video_slot(&self) -> Option<MemorySlot> {
            self.video_slot
        }
        /// Borrow this active session's endpoints and allocation state.
        pub fn resources(&mut self) -> Result<&mut Resources<'mapping>, Error> {
            if self.state != State::Active {
                return Err(Error::InvalidState);
            }
            Ok(&mut self.resources)
        }

        /// # Safety
        /// Retain exclusive primary access and the live mapping through completion.
        pub unsafe fn solid(
            &mut self,
            bounds: [i32; 4],
            color: u32,
            truth: u8,
        ) -> Result<(), Error> {
            let [left, top, right, bottom] = bounds;
            if self.state != State::Active
                || right > self.layout.width() as i32
                || bottom > self.layout.height() as i32
            {
                return Err(Error::InvalidState);
            }
            let draw = crate::draw::Solid::new([left, top, right, bottom], color, truth)
                .ok_or(Error::InvalidState)?;
            // SAFETY: bounds and this session's primary were validated above.
            unsafe {
                self.draw(|resources, clock| {
                    draw.submit(&mut resources.pool, &mut resources.commands, clock)
                })
            }
        }

        /// # Safety
        /// Keep exclusive primary access and mappings through host completion.
        pub unsafe fn copy_bits(
            &mut self,
            bounds: [i32; 4],
            source: [i32; 2],
        ) -> Result<(), Error> {
            let draw = crate::draw::CopyBits::new(bounds, source).ok_or(Error::InvalidState)?;
            if self.state != State::Active
                || bounds[2] > self.layout.width() as i32
                || bounds[3] > self.layout.height() as i32
                || source[0] + (bounds[2] - bounds[0]) > self.layout.width() as i32
                || source[1] + (bounds[3] - bounds[1]) > self.layout.height() as i32
            {
                return Err(Error::InvalidState);
            }
            // SAFETY: both rectangles fit the exclusively retained primary.
            unsafe {
                self.draw(|resources, clock| {
                    draw.submit(&mut resources.pool, &mut resources.commands, clock)
                })
            }
        }

        /// # Safety
        /// Retain exclusive destination access and mappings through publication.
        /// Pixels must be the fully defined, top-down source bitmap.
        pub unsafe fn image(
            &mut self,
            draw: crate::draw::ImageDraw,
            width: u32,
            height: u32,
            pixels: &[u8],
            alpha: bool,
        ) -> Result<(), Error> {
            let bounds = draw.bounds();
            if self.state != State::Active
                || bounds[2] > self.layout.width() as i32
                || bounds[3] > self.layout.height() as i32
            {
                return Err(Error::InvalidState);
            }
            // SAFETY: the destination bounds and active session were checked.
            unsafe {
                self.draw_image(width, height, pixels, alpha, |image, resources, clock| {
                    draw.submit(image, &mut resources.pool, &mut resources.commands, clock)
                })
            }
        }

        /// # Safety
        /// Retain the primary and captured indexed source until publication completes.
        pub unsafe fn indexed_image(
            &mut self,
            draw: crate::draw::ImageDraw,
            bitmap: crate::bitmap::Bitmap<'_>,
            colors: crate::palette::Colors<'_>,
        ) -> Result<(), Error> {
            let bounds = draw.bounds();
            if self.state != State::Active
                || bounds[2] > self.layout.width() as i32
                || bounds[3] > self.layout.height() as i32
            {
                return Err(Error::InvalidState);
            }
            // SAFETY: the session owns the mapped release endpoint.
            unsafe { self.reap_completed(64) }?;
            let slot = self.resources.main_slot();
            let image = self
                .images
                .acquire_indexed(&mut self.resources.pool, slot, bitmap, colors)
                .map_err(Error::Pool)?;
            // SAFETY: publication retains the image and its palette independently.
            let result = unsafe {
                self.draw(|resources, clock| {
                    draw.submit(&image, &mut resources.pool, &mut resources.commands, clock)
                })
            };
            image
                .release(&mut self.resources.pool)
                .map_err(|(error, _)| Error::Pool(error))?;
            result
        }

        /// # Safety
        /// Retain exclusive destination access and immutable top-down pattern pixels.
        pub unsafe fn pattern(
            &mut self,
            draw: crate::draw::PatternFill,
            width: u32,
            height: u32,
            pixels: &[u8],
        ) -> Result<(), Error> {
            let bounds = draw.bounds();
            if self.state != State::Active
                || bounds[2] > self.layout.width() as i32
                || bounds[3] > self.layout.height() as i32
            {
                return Err(Error::InvalidState);
            }
            // SAFETY: the destination and retained pattern satisfy publication bounds.
            unsafe {
                self.draw_image(width, height, pixels, false, |image, resources, clock| {
                    draw.submit(image, &mut resources.pool, &mut resources.commands, clock)
                })
            }
        }

        unsafe fn draw_image(
            &mut self,
            width: u32,
            height: u32,
            pixels: &[u8],
            alpha: bool,
            operation: impl FnOnce(
                &crate::image::Image<'mapping>,
                &mut Resources<'mapping>,
                u32,
            ) -> Result<bool, display_resources::Error>,
        ) -> Result<(), Error> {
            // SAFETY: the retained session exclusively owns the release endpoint.
            unsafe { self.reap_completed(64) }?;
            let slot = self.resources.main_slot();
            // SAFETY: mapped-device validation retains this aligned ROM field.
            let compression = unsafe { self.device.info().compression_level.read_volatile() };
            let header = crate::quic::Header {
                format: if alpha {
                    crate::quic::Format::Bgra32
                } else {
                    crate::quic::Format::Bgrx32
                },
                width: width as usize,
                height: height as usize,
            };
            let compressed = if compression != 0 {
                // SAFETY: this active session exclusively owns the pool and slot.
                unsafe {
                    crate::quic::native::compress(header, pixels, &mut self.resources.pool, slot)
                }
            } else {
                None
            };
            let image = match compressed {
                Some(image) => image,
                None => self
                    .images
                    .acquire(&mut self.resources.pool, slot, width, height, pixels, alpha)
                    .map_err(Error::Pool)?,
            };
            // SAFETY: both image and destination remain retained by this session.
            let result =
                unsafe { self.draw(|resources, clock| operation(&image, resources, clock)) };
            image
                .release(&mut self.resources.pool)
                .map_err(|(error, _)| Error::Pool(error))?;
            result
        }

        /// # Safety
        /// Retain exclusive primary access and its mappings through publication.
        pub unsafe fn text(
            &mut self,
            draw: crate::text::Text<'_>,
            glyphs: crate::text::Glyphs<'_>,
        ) -> Result<(), Error> {
            let bounds = draw.bounds();
            if self.state != State::Active
                || bounds[2] > self.layout.width() as i32
                || bounds[3] > self.layout.height() as i32
            {
                return Err(Error::InvalidState);
            }
            let slot = self.resources.main_slot();
            // SAFETY: the session owns its primary, mappings and queue endpoints.
            unsafe {
                self.draw(|resources, clock| {
                    draw.submit(
                        glyphs,
                        &mut resources.pool,
                        slot,
                        &mut resources.commands,
                        clock,
                    )
                })
            }
        }

        /// # Safety
        /// Retain exclusive primary access and mappings through publication.
        pub unsafe fn stroke(
            &mut self,
            draw: crate::path::Stroke<'_>,
            path: crate::path::Segments<'_>,
            style: &crate::cosmetic::LineStyle,
        ) -> Result<(), Error> {
            let bounds = draw.bounds();
            if self.state != State::Active
                || bounds[2] > self.layout.width() as i32
                || bounds[3] > self.layout.height() as i32
            {
                return Err(Error::InvalidState);
            }
            let slot = self.resources.main_slot();
            // SAFETY: the session retains its destination and exclusive endpoints.
            unsafe {
                self.draw(|resources, clock| {
                    draw.submit(
                        path,
                        style,
                        &mut resources.pool,
                        slot,
                        &mut resources.commands,
                        clock,
                    )
                })
            }
        }

        /// # Safety
        /// Retain exclusive primary access and mappings through host completion.
        pub unsafe fn raster(&mut self, draw: crate::raster_draw::Raster<'_>) -> Result<(), Error> {
            let bounds = draw.bounds();
            if self.state != State::Active
                || bounds[2] > self.layout.width() as i32
                || bounds[3] > self.layout.height() as i32
            {
                return Err(Error::InvalidState);
            }
            let slot = self.resources.main_slot();
            // SAFETY: the session owns its primary and both queue endpoints.
            unsafe {
                self.draw(|resources, clock| {
                    draw.submit(&mut resources.pool, slot, &mut resources.commands, clock)
                })
            }
        }

        unsafe fn draw(
            &mut self,
            operation: impl FnOnce(
                &mut Resources<'mapping>,
                u32,
            ) -> Result<bool, display_resources::Error>,
        ) -> Result<(), Error> {
            // SAFETY: callers retain the exclusive active session and endpoints.
            unsafe { self.reap_completed(64) }?;
            // SAFETY: the validated ROM clock stays mapped through this session.
            let clock = unsafe { self.device.info().clock.read_volatile() };
            let notify = operation(&mut self.resources, clock).map_err(Error::Pool)?;
            if notify {
                // SAFETY: the operation finished publication before notification.
                unsafe { self.notify(false) }?;
            }
            Ok(())
        }

        /// Pool access remains available for releasing tokens after shutdown.
        pub fn pool(&mut self) -> &mut Pool<'mapping> {
            &mut self.resources.pool
        }

        unsafe fn command(&mut self, operation: Operation) -> Result<(), Error> {
            // SAFETY: callers retain the exclusive passive-level adapter context
            // and live operation resources required by the control channel.
            let result = unsafe { self.control.issue(operation) }.map_err(Error::Control);
            if self.control.failed() {
                self.state = State::Failed;
            }
            result
        }

        unsafe fn reap_completed(
            &mut self,
            budget: usize,
        ) -> Result<display_resources::Reclaimed, Error> {
            // SAFETY: callers retain this generation and the host-completion
            // contract; every release error stops further session operations.
            let result = unsafe { self.resources.reap(budget) };
            if result.is_err() {
                self.state = State::Failed;
            }
            result
        }

        /// # Safety
        /// Retain the exclusive passive-level context from prepare. No GDI or
        /// CPU user may access primary pixels while the host creates/clears them.
        /// Preserve this entire session/mapping after any error until reset
        /// proves quiescence; a timeout does not permit arena reuse or unmap.
        pub unsafe fn activate(&mut self) -> Result<(), Error> {
            if self.state != State::Prepared {
                return Err(Error::InvalidState);
            }
            let info = self.device.info();
            let interface = self.device.interface();
            let framebuffer = self.device.framebuffer();
            let physical = interface
                .main_slot()
                .encode(
                    interface.primary().base().as_ptr().addr() as u64,
                    self.layout.bytes() as u64,
                )
                .map_err(Error::Slot)?;
            self.state = State::Failed;
            if let Some(slot) = interface.secondary_slot() {
                let end = info
                    .framebuffer_physical
                    .checked_add(framebuffer.len() as u64)
                    .ok_or(Error::Slot(SlotError::InvalidRange))?;
                // SAFETY: this is the validated video BAR and unused secondary
                // slot. The unique control channel serialises header and wait.
                unsafe {
                    self.command(Operation::AddSlot {
                        slot,
                        start: info.framebuffer_physical,
                        end,
                    })
                }?;
                // SAFETY: successful slot-add completion precedes this ROM read;
                // the mapped ROM remains live through this borrowed session.
                let generation = unsafe { info.slot_generation.read_volatile() };
                self.video_slot = Some(
                    SlotLayout::new(info.slot_id_bits, info.slot_generation_bits)
                        .and_then(|layout| {
                            layout.slot(
                                slot,
                                generation,
                                info.framebuffer_physical,
                                framebuffer.base().as_ptr().addr() as u64,
                                framebuffer.len() as u64,
                            )
                        })
                        .map_err(Error::Slot)?,
                );
            }
            // SAFETY: the mapped primary extent and checked encoded address are
            // live; all CPU/GDI users remain stopped until this completes.
            unsafe {
                self.command(Operation::CreatePrimary {
                    layout: self.layout,
                    physical,
                })
            }?;
            self.state = State::Active;
            Ok(())
        }

        /// # Safety
        /// Run in the exclusive context from prepare after publishing complete
        /// commands into this generation's corresponding ring.
        pub unsafe fn notify(&mut self, cursor: bool) -> Result<(), Error> {
            if self.state != State::Active {
                return Err(Error::InvalidState);
            }
            // SAFETY: the caller finished publication under the same adapter
            // ownership; these synchronous ports do not use the completion event.
            unsafe {
                self.command(if cursor {
                    Operation::NotifyCursor
                } else {
                    Operation::NotifyCommand
                })
            }
        }

        /// # Safety
        /// Keep the passive-level exclusive adapter context and retain the
        /// addressed surface. The rectangle must fit that surface. End all CPU
        /// pixel writes before the request; do not resume them during the wait.
        pub unsafe fn update(&mut self, surface: u32, area: [i32; 4]) -> Result<(), Error> {
            if self.state != State::Active {
                return Err(Error::InvalidState);
            }
            // SAFETY: the caller retains the surface and its bounded rectangle
            // and excludes CPU/GDI access through command completion.
            unsafe { self.command(Operation::Update { surface, area }) }
        }

        /// Borrow an unlocked-on-drop GDI view after synchronising the primary.
        /// # Safety
        /// Keep the live GDI device and exclusive passive-level context. Exclude
        /// every other CPU/GDI pixel user. After GDI writes, drop the draw area
        /// and call update before relying on the host's display contents.
        pub unsafe fn primary_draw_area(
            &mut self,
            engine: NonNull<c_void>,
        ) -> Result<crate::gdi::DrawArea<'_>, Error> {
            let area = [
                0,
                0,
                self.layout.height() as i32,
                self.layout.width() as i32,
            ];
            // SAFETY: the primary extent/layout are validated and all other
            // users are stopped, so the full rectangle synchronises prior draws.
            unsafe { self.update(0, area) }?;
            // SAFETY: the session borrow retains primary pixels and excludes
            // new hardware access through draw-area unlock/deletion on Drop.
            unsafe {
                crate::gdi::DrawArea::from_raw(
                    engine,
                    self.device.interface().primary().base(),
                    self.layout,
                )
            }
            .map_err(Error::Gdi)
        }

        /// # Safety
        /// Preserve host-completion and mapping guarantees from Resources::reap.
        /// All pool and notification access remains exclusive at PASSIVE_LEVEL.
        pub unsafe fn reap(
            &mut self,
            budget: usize,
        ) -> Result<display_resources::Reclaimed, Error> {
            if self.state != State::Active {
                return Err(Error::InvalidState);
            }
            // SAFETY: the caller retains this generation through host releases.
            let reclaimed = unsafe { self.reap_completed(budget) }?;
            if reclaimed.notify {
                // SAFETY: notification follows completed release consumption;
                // this context retains exclusive adapter control ownership.
                unsafe { self.command(Operation::NotifyOutOfMemory) }?;
            }
            Ok(reclaimed)
        }

        /// # Safety
        /// Publish the cursor-hide command and stop new GDI/CPU submissions.
        /// Flush/read back any live GDI
        /// bitmaps to retained system memory before their host surfaces vanish.
        /// Do not call from DrvDisableSurface: hardware shutdown belongs to
        /// DrvAssertMode. Retain all mappings and tokens after an error.
        pub unsafe fn stop(&mut self) -> Result<(), Error> {
            match self.state {
                State::Prepared => {
                    self.state = State::Stopped;
                    return Ok(());
                }
                State::Active => {}
                State::Stopped | State::Failed => return Err(Error::InvalidState),
            }
            self.state = State::Failed;
            // SAFETY: all new users are stopped, with CPU copies retained.
            unsafe { self.command(Operation::DestroyAll) }?;
            if self.device.info().pci_revision < REVISION_ASYNC {
                // SAFETY: legacy reset stops all device and delayed release
                // accesses, invalidating old ring/slot state before discard.
                return unsafe { self.reset() };
            }
            // SAFETY: destroy completed; force remaining completed releases
            // into the release ring before draining this retained generation.
            unsafe { self.command(Operation::FlushRelease) }?;
            loop {
                // SAFETY: this endpoint retains the mapping; each host release
                // follows all command and resource reads for that output.
                if unsafe { self.reap_completed(50) }?.outputs == 0 {
                    break;
                }
            }
            if self.resources.pool.outstanding_outputs() != 0 {
                return Err(Error::OutstandingOutputs);
            }
            if let Some(slot) = self.device.interface().secondary_slot() {
                // SAFETY: all host commands completed; no new surface/slot users
                // may start, so removing the registered video slot is valid.
                unsafe { self.command(Operation::DeleteSlot(slot)) }?;
            }
            self.images
                .clear(&mut self.resources.pool)
                .map_err(Error::Pool)?;
            self.release_monitors()?;
            self.video_slot = None;
            self.state = State::Stopped;
            Ok(())
        }

        /// # Safety
        /// Retain exclusive adapter access and all live mappings through completion.
        pub unsafe fn set_monitor(&mut self, head: &[u8; 28]) -> Result<(), Error> {
            if self.state != State::Active
                || self.device.info().pci_revision < crate::memory::REVISION_MONITORS
            {
                return Err(Error::InvalidState);
            }
            if self.monitors.is_none() {
                self.monitors = Some(
                    self.resources
                        .pool
                        .resource(crate::display_resources::Region::Main, 32)
                        .map_err(Error::Pool)?,
                );
            }
            let resource = self.monitors.as_ref().ok_or(Error::InvalidState)?;
            let mut config = [0u8; 32];
            config[..4].copy_from_slice(&[1, 0, 1, 0]);
            config[12..28].copy_from_slice(&head[8..24]);
            // SAFETY: this retained 32-byte main-slot resource has no CPU borrower;
            // the control channel serialises reads with the previous configuration.
            unsafe {
                core::ptr::copy_nonoverlapping(config.as_ptr(), resource.as_ptr(), config.len())
            };
            let physical = self
                .resources
                .main_slot()
                .encode(resource.as_ptr().addr() as u64, 32)
                .map_err(Error::Slot)?;
            // SAFETY: the persistent resource remains owned through stop/reset.
            unsafe { self.command(Operation::Monitors(physical)) }
        }

        fn release_monitors(&mut self) -> Result<(), Error> {
            if let Some(resource) = self.monitors.take()
                && let Err((error, resource)) = self.resources.pool.release(resource)
            {
                self.monitors = Some(resource);
                return Err(Error::Pool(error));
            }
            Ok(())
        }

        /// Explicitly stop unknown outstanding work; never an automatic fallback.
        /// # Safety
        /// Exclude every GDI/CPU/control/power user of this physical adapter.
        /// Preserve required pixel data before reset and keep all BAR mappings
        /// live. The miniport's completed reset must stop old device reads and
        /// delayed release writes. Never reuse old ring/slot endpoints afterward.
        pub unsafe fn reset(&mut self) -> Result<(), Error> {
            if self.state == State::Stopped {
                return Err(Error::InvalidState);
            }
            self.state = State::Failed;
            let mut returned = 0;
            // SAFETY: the mapped owner retains the live miniport handle; the
            // reset request has no input/output and runs in the exclusive path.
            let status = unsafe {
                EngDeviceIoControl(
                    self.device.driver().as_ptr(),
                    RESET_DEVICE,
                    core::ptr::null_mut::<c_void>(),
                    0,
                    core::ptr::null_mut(),
                    0,
                    &mut returned,
                )
            };
            if status != 0 {
                return Err(Error::Ioctl(status));
            }
            // SAFETY: completed reset explicitly stops all old host command and
            // release access. Arena mappings remain live; external cache tokens
            // are still retained and must be released by their owners.
            unsafe { self.resources.pool.discard_pending() };
            self.images
                .clear(&mut self.resources.pool)
                .map_err(Error::Pool)?;
            self.release_monitors()?;
            self.video_slot = None;
            self.state = State::Stopped;
            Ok(())
        }

        /// All CPU/GDI users must also stop; this checks only session/pool state.
        pub fn can_unmap(&self) -> bool {
            self.state == State::Stopped && self.resources.pool.is_empty()
        }
    }
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub use native::Hardware;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor::{Command, Input, Position, Shape};
    use crate::display_device::{VideoMemoryInformation, test_utils::mapped_info};
    use crate::memory::*;
    use crate::surface::Format;

    fn initialize(ram: &mut [u64], rom: &mut [u32]) -> DriverInfo {
        let base = NonNull::from(ram).cast::<u8>();
        let info = mapped_info(base.as_ptr(), rom.as_mut_ptr().cast());
        for (offset, capacity) in [
            (COMMAND_RING_OFFSET, 32u32),
            (CURSOR_RING_OFFSET, 32),
            (RELEASE_RING_OFFSET, 8),
        ] {
            // SAFETY: these aligned ring headers lie in the retained RAM header
            // (0x4000 offset) within the complete live allocation. No endpoint
            // exists during initialisation and the remainder is zeroed by tests.
            unsafe {
                base.as_ptr()
                    .add(0x4000 + offset as usize)
                    .cast::<u32>()
                    .write(capacity)
            };
        }
        info
    }

    fn framebuffer(info: &DriverInfo, layout: Layout, video: &mut [u64]) -> Region {
        let length = size_of_val(video) as u32;
        let pointer = video.as_mut_ptr().cast();
        Interface::validate(info, layout)
            .unwrap()
            .framebuffer(&VideoMemoryInformation {
                video_base: pointer,
                video_length: length,
                framebuffer_base: pointer,
                framebuffer_length: length,
            })
            .unwrap()
    }

    #[test]
    fn mapped_resources_keep_primary_pixels_outside_the_allocators() {
        let mut ram = [0x5a5a5a5a5a5a5a5au64; 8192];
        // Only the device header starts zeroed; the primary must stay untouched.
        ram[0x4000 / 8..].fill(0);
        let mut rom = [0u32; 32];
        let mut video = [0u64; 1024];
        let info = initialize(&mut ram, &mut rom);
        let layout = Layout::new(8, 8, Format::Xrgb8888).unwrap();
        let framebuffer = framebuffer(&info, layout, &mut video);
        // SAFETY: all live allocations are disjoint, no old commands/tokens
        // exist, and this scope exclusively owns the endpoints and arenas.
        let mut resources = unsafe { Resources::new(&info, layout, framebuffer) }.unwrap();
        let slot = resources.main_slot();
        let shape = Shape::allocate(&mut resources.pool, slot, Input::Transparent, 0).unwrap();
        assert_eq!(resources.pool.outstanding_outputs(), 0);
        // SAFETY: the shape and ring share this actual mapped allocation/slot.
        unsafe {
            shape.set(
                &mut resources.pool,
                &mut resources.cursor,
                Position::At([2, 3]),
            )
        }
        .unwrap();
        assert_eq!(resources.pool.outstanding_outputs(), 1);
        shape.release(&mut resources.pool).unwrap();
        // SAFETY: this simulated host stops all accesses without release writes.
        unsafe { resources.pool.discard_pending() };
        assert!(resources.pool.is_empty());
        // SAFETY: the same live pool and cursor endpoint still belong together.
        unsafe {
            Command::Move(Position::Hidden).submit(&mut resources.pool, &mut resources.cursor)
        }
        .unwrap();
        // SAFETY: the simulated host stops before discarding its pending output.
        unsafe { resources.pool.discard_pending() };
        assert!(resources.pool.is_empty());
        assert!(
            ram[..4096 / 8]
                .iter()
                .all(|value| *value == 0x5a5a5a5a5a5a5a5a)
        );
    }

    #[test]
    fn malformed_ring_capacity_fails_before_initialising_allocator_headers() {
        let mut ram = [0x5a5a5a5a5a5a5a5au64; 8192];
        ram[0x4000 / 8..].fill(0);
        let mut rom = [0u32; 32];
        let mut video = [0u64; 1024];
        let info = initialize(&mut ram, &mut rom);
        let layout = Layout::new(8, 8, Format::Xrgb8888).unwrap();
        let framebuffer = framebuffer(&info, layout, &mut video);
        // SAFETY: this live RAM header is writable and has no endpoint yet.
        unsafe { info.release_ring.cast::<u32>().write(4) };
        // SAFETY: mappings remain live and exclusive; the bad ring is rejected.
        let result = unsafe { Resources::new(&info, layout, framebuffer) };
        assert!(matches!(
            result,
            Err(Error::Ring(RingError::InvalidCapacity))
        ));
        assert!(
            ram[4096 / 8..12288 / 8]
                .iter()
                .all(|value| *value == 0x5a5a5a5a5a5a5a5a)
        );
    }
}
