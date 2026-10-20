// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! QXL miniport reset, interrupt and display-interface operations.

use core::cell::UnsafeCell;
use core::mem::size_of;
use core::ptr::{copy_nonoverlapping, null_mut, read_volatile, write_volatile};
use core::sync::atomic::{AtomicU8, Ordering, fence};

use crate::driver_info::{DriverInfo, GET_INFO, MemSlot, SET_CUSTOM_DISPLAY};
use crate::memory::{COMMAND_RING_OFFSET, CURSOR_RING_OFFSET, RELEASE_RING_OFFSET};
use crate::mode_pool::ModePool;
use crate::modes::ModeError;
use crate::resources::DeviceResources;
use crate::slots::SlotLayout;
use crate::videoport::*;

const INTERRUPT_MASK: u32 = 7;
const INVALID_DATA: u32 = 13;
const QUERY_AVAILABLE_MODES: u32 = 0x230000;
const QUERY_MODE_COUNT: u32 = 0x230004;
const QUERY_CURRENT_MODE: u32 = 0x230008;
const SET_CURRENT_MODE: u32 = 0x23000c;
const RESET_DEVICE: u32 = 0x230010;
const MAP_VIDEO_MEMORY: u32 = 0x230058;
const UNMAP_VIDEO_MEMORY: u32 = 0x23005c;

#[derive(Debug)]
pub struct Adapter {
    pub(crate) resources: DeviceResources,
    pub(crate) modes: UnsafeCell<ModePool>,
    generation: AtomicU8,
}

impl Adapter {
    /// # Safety
    /// Invoke at PASSIVE_LEVEL as a serialised VideoPort request. Both buffers
    /// must be valid for their declared lengths and must not alias driver state
    /// or the request record. Input/output may alias each other; inputs are
    /// consumed before an output borrow starts.
    pub unsafe fn dispatch(
        &self,
        request: &RequestPacket,
        surfaces_enabled: bool,
    ) -> Result<usize, u32> {
        // SAFETY: the caller supplies the IRP buffers and VideoPort serialisation.
        unsafe {
            match request.ioctl {
                QUERY_MODE_COUNT => {
                    let modes = &*self.modes.get();
                    let count = u32::try_from(modes.count()).map_err(|_| INVALID_DATA)?;
                    output(request, 8, |bytes| {
                        bytes[..4].copy_from_slice(&count.to_le_bytes());
                        bytes[4..].copy_from_slice(&80u32.to_le_bytes());
                        Some(8)
                    })
                }
                QUERY_AVAILABLE_MODES => {
                    let modes = &*self.modes.get();
                    let length = modes.count().checked_mul(80).ok_or(INVALID_DATA)?;
                    output(request, length, |bytes| {
                        for (mode, bytes) in modes.entries().zip(bytes.as_chunks_mut::<80>().0) {
                            mode.ok()?.encode(bytes)?;
                        }
                        Some(length)
                    })
                }
                SET_CURRENT_MODE => {
                    let id = u32::from_le_bytes(input::<4>(request)?);
                    (&mut *self.modes.get())
                        .set_current(id)
                        .map_err(mode_error)?;
                    Ok(0)
                }
                QUERY_CURRENT_MODE => {
                    let mode = (&*self.modes.get()).current().map_err(mode_error)?;
                    output(request, 80, |bytes| mode.encode(bytes))
                }
                MAP_VIDEO_MEMORY => {
                    if input::<{ size_of::<usize>() }>(request)?
                        .iter()
                        .any(|&byte| byte != 0)
                    {
                        return Err(87);
                    }
                    let base = self.resources.vram.address().as_ptr() as usize;
                    let length = self.resources.vram.length();
                    let size = if size_of::<usize>() == 8 { 32 } else { 16 };
                    output(request, size, |bytes| {
                        bytes.fill(0);
                        let second = size / 2;
                        for offset in [0, second] {
                            bytes[offset..offset + size_of::<usize>()]
                                .copy_from_slice(&base.to_le_bytes());
                            let offset = offset + size_of::<usize>();
                            bytes[offset..offset + 4].copy_from_slice(&length.to_le_bytes());
                        }
                        Some(size)
                    })
                }
                UNMAP_VIDEO_MEMORY => {
                    input::<{ size_of::<usize>() }>(request)?;
                    Ok(0)
                }
                RESET_DEVICE => {
                    self.initialize()?;
                    Ok(0)
                }
                GET_INFO => {
                    let info = self.driver_info(surfaces_enabled)?;
                    output(request, size_of::<DriverInfo>(), |bytes| info.encode(bytes))
                }
                SET_CUSTOM_DISPLAY => {
                    let bytes = input::<12>(request)?;
                    let chunks = bytes.as_chunks::<4>().0;
                    let words = [chunks[0], chunks[1], chunks[2]].map(u32::from_le_bytes);
                    (&mut *self.modes.get())
                        .set_custom(words[0], words[1], words[2])
                        .map_err(mode_error)?;
                    Ok(0)
                }
                _ => Err(1),
            }
        }
    }
    /// # Safety
    /// Retain the device extension and these resources throughout all callbacks.
    /// Construct and destroy at PASSIVE_LEVEL. Stop ISR, DPC and display access
    /// before destruction. Mode access must use VideoPort's request serialisation.
    pub unsafe fn new(resources: DeviceResources) -> Result<Self, u32> {
        // SAFETY: the caller supplies the live resources and passive-level contract.
        let modes = unsafe { ModePool::new(&resources) }?;
        Ok(Self {
            resources,
            modes: UnsafeCell::new(modes),
            generation: AtomicU8::new(0),
        })
    }

    fn header(&self, offset: u32) -> *mut u8 {
        self.resources
            .ram
            .address()
            .as_ptr()
            .wrapping_add((self.resources.info.ram_header + offset) as usize)
    }

    fn rom(&self, offset: u32) -> *mut u8 {
        self.resources
            .rom
            .address()
            .as_ptr()
            .wrapping_add(offset as usize)
    }

    unsafe fn mask(&self, value: u32) {
        // SAFETY: discovery checked the full RAM header and its four-byte alignment.
        unsafe { write_volatile(self.header(8).cast::<u32>(), value) };
    }

    unsafe fn write_port(&self, offset: u32, value: u8) -> Result<(), u32> {
        let port = self.resources.io.port(offset)?;
        fence(Ordering::Release);
        // SAFETY: this is a mapped and bounds-checked I/O port for the live device.
        unsafe { VideoPortWritePortUchar(port, value) };
        Ok(())
    }

    fn slot(&self) -> Result<MemSlot, u32> {
        let start = self.resources.ram.physical() as u64;
        let virtual_start = self.resources.ram.address().as_ptr() as usize as u64;
        let length = u64::from(self.resources.info.main_slot_bytes());
        Ok(MemSlot {
            generation: self.generation.load(Ordering::Acquire),
            physical_start: start,
            physical_end: start.checked_add(length).ok_or(INVALID_DATA)?,
            virtual_start,
            virtual_end: virtual_start.checked_add(length).ok_or(INVALID_DATA)?,
        })
    }

    /// # Safety
    /// Call at PASSIVE_LEVEL, serialised with mode and power requests. No display
    /// commands may use old slots or ring endpoints across this operation.
    pub unsafe fn initialize(&self) -> Result<(), u32> {
        // SAFETY: the caller has stopped display operations before hardware reset.
        unsafe {
            self.write_port(5, 0)?;
            self.resume()?;
        }
        Ok(())
    }

    /// # Safety
    /// Same serialisation as initialize, but the host/device has already restored
    /// its power state. This follows the original non-resetting power-on path.
    pub unsafe fn resume(&self) -> Result<(), u32> {
        // SAFETY: the caller serialises this setup with mode and power requests.
        let result = unsafe { self.register_slot() };
        if result.is_err() {
            // SAFETY: a failed setup must leave interrupts masked while the
            // framework handles failure; the mappings remain live throughout.
            unsafe {
                self.mask(0);
                let _ = self.write_port(3, 0);
            }
        }
        result
    }

    unsafe fn register_slot(&self) -> Result<(), u32> {
        let slot = self.slot()?;
        // SAFETY: the caller serialises this setup with display and power requests.
        unsafe {
            self.mask(INTERRUPT_MASK);
            for (offset, value) in [(5276, slot.physical_start), (5284, slot.physical_end)] {
                for (byte, value) in value.to_le_bytes().into_iter().enumerate() {
                    // The packed memory-slot fields need not be u64-aligned.
                    write_volatile(self.header(offset).add(byte), value);
                }
            }
            self.write_port(8, self.resources.info.first_slot)?;
        }
        let generation = self.resources.rom.snapshot::<1>(68)?[0];
        let info = self.resources.info;
        SlotLayout::new(info.slot_id_bits, info.slot_generation_bits)
            .and_then(|layout| {
                layout.slot(
                    info.first_slot,
                    generation,
                    slot.physical_start,
                    slot.virtual_start,
                    slot.physical_end - slot.physical_start,
                )
            })
            .map_err(|_| INVALID_DATA)?;
        self.generation.store(generation, Ordering::Release);
        Ok(())
    }

    /// # Safety
    /// Invoke only as the registered ISR, with live resources at device IRQL.
    /// callback must be a nonpageable DPC routine using this same extension.
    pub unsafe fn interrupt(&self, callback: Dpc) -> bool {
        // SAFETY: the aligned header remains live throughout the registered ISR.
        let (pending, mask) = unsafe {
            (
                read_volatile(self.header(4).cast::<u32>()),
                read_volatile(self.header(8).cast::<u32>()),
            )
        };
        if pending & mask == 0 {
            return false;
        }
        // SAFETY: only interrupt registers/events are used; no paged mode state.
        unsafe {
            self.mask(0);
            if self.write_port(3, 0).is_err() {
                return false;
            }
            let device = self.resources.device().as_ptr();
            if VideoPortQueueDpc(device, callback, null_mut()) == 0 {
                VideoPortLogError(device, null_mut(), 0x8000ffff, 1);
                self.mask(INTERRUPT_MASK);
                let _ = self.write_port(3, 0);
            }
        }
        true
    }

    /// # Safety
    /// Invoke as the registered VideoPort DPC with live nonpageable resources.
    pub unsafe fn complete_interrupt(&self) {
        // SAFETY: the counter is four-byte aligned, live, and accessed by the
        // same interlocked operation used by the original miniport.
        let pending =
            unsafe { VideoPortInterlockedExchange(self.header(4).cast::<i32>(), 0) } as u32;
        // SAFETY: these events belong to this device and remain live through the DPC.
        unsafe {
            for (bit, event) in [
                (1, &self.resources.events.display),
                (2, &self.resources.events.cursor),
                (4, &self.resources.events.io_command),
            ] {
                if pending & bit != 0 {
                    VideoPortSetEvent(self.resources.device().as_ptr(), event.address().as_ptr());
                }
            }
            self.mask(INTERRUPT_MASK);
            let _ = self.write_port(3, 0);
        }
    }

    /// Snapshot the version-3 miniport interface. The caller must keep this
    /// adapter live while the display driver uses any returned raw pointer.
    pub fn driver_info(&self, surfaces_enabled: bool) -> Result<DriverInfo, u32> {
        let resources = &self.resources;
        let info = resources.info;
        let port = |offset| resources.io.port(offset).unwrap_or(null_mut());
        let pages_virtual = resources
            .ram
            .address()
            .as_ptr()
            .wrapping_add(info.surface_bytes as usize);
        Ok(DriverInfo {
            version: 3,
            command_ring: self.header(COMMAND_RING_OFFSET).cast(),
            cursor_ring: self.header(CURSOR_RING_OFFSET).cast(),
            release_ring: self.header(RELEASE_RING_OFFSET).cast(),
            notify_command_port: port(0),
            notify_cursor_port: port(1),
            notify_oom_port: port(4),
            update_area_async_port: port(16),
            memslot_add_async_port: port(17),
            create_primary_async_port: port(18),
            destroy_primary_async_port: port(19),
            destroy_surface_async_port: port(20),
            destroy_all_surfaces_async_port: port(21),
            flush_surfaces_async_port: port(22),
            flush_release_port: port(23),
            display_event: resources.events.display.address().as_ptr(),
            cursor_event: resources.events.cursor.address().as_ptr(),
            sleep_event: resources.events.sleep.address().as_ptr(),
            io_command_event: resources.events.io_command.address().as_ptr(),
            page_count: info.page_count,
            io_pages_virtual: pages_virtual.cast(),
            io_pages_physical: (resources.ram.physical() as u64)
                .checked_add(u64::from(info.surface_bytes))
                .ok_or(INVALID_DATA)?,
            primary_area: resources.ram.address().as_ptr(),
            primary_area_size: info.surface_bytes,
            update_id: self.rom(8).cast(),
            compression_level: self.rom(12).cast(),
            update_area_port: port(2),
            update_area: self.header(5256).cast(),
            update_surface: self.header(5272).cast(),
            clock: self.rom(48).cast(),
            log_port: port(7),
            log_buffer: self.header(12),
            log_level: self.rom(16).cast(),
            slot_count: info.slot_end,
            main_slot_id: info.first_slot,
            slot_id_bits: info.slot_id_bits,
            slot_generation_bits: info.slot_generation_bits,
            slot_generation: self.rom(68),
            ram_slot_start: self.header(5276).cast(),
            ram_slot_end: self.header(5284).cast(),
            main_slot: self.slot()?,
            destroy_surface_wait_port: port(14),
            create_primary_port: port(12),
            destroy_primary_port: port(13),
            memslot_add_port: port(8),
            memslot_delete_port: port(9),
            destroy_all_surfaces_port: port(15),
            monitors_config_port: port(24),
            pci_revision: resources.revision,
            device_id: info.device_id,
            primary_surface_create: self.header(5292).cast(),
            surface_count: info.surface_count,
            framebuffer_physical: resources.vram.physical() as u64,
            create_non_primary_surfaces: u8::from(surfaces_enabled),
            monitors_config: self.header(5340).cast(),
        })
    }
}

fn mode_error(error: ModeError) -> u32 {
    match error {
        ModeError::InsufficientMemory => 8,
        ModeError::InvalidId => 87,
        _ => INVALID_DATA,
    }
}

unsafe fn input<const N: usize>(request: &RequestPacket) -> Result<[u8; N], u32> {
    if request.input.is_null() || (request.input_length as usize) < N {
        return Err(122);
    }
    let mut bytes = [0; N];
    // SAFETY: the callback contract guarantees the checked input range; copying
    // consumes it before any potentially aliasing output borrow is created.
    unsafe { copy_nonoverlapping(request.input.cast::<u8>(), bytes.as_mut_ptr(), N) };
    Ok(bytes)
}

unsafe fn output(
    request: &RequestPacket,
    length: usize,
    encode: impl FnOnce(&mut [u8]) -> Option<usize>,
) -> Result<usize, u32> {
    if request.output.is_null() || length > request.output_length as usize {
        return Err(122);
    }
    // SAFETY: the checked buffer is writable and does not alias driver state;
    // all input for this operation was consumed before entering this function.
    let bytes = unsafe { core::slice::from_raw_parts_mut(request.output.cast::<u8>(), length) };
    encode(bytes).ok_or(INVALID_DATA)
}
