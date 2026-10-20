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
            })
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
            self.video_slot = None;
            self.state = State::Stopped;
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
