// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Miniport PCI discovery and ownership of VideoPort mappings and events.

use core::ffi::c_void;
use core::ptr::{NonNull, null_mut, read_volatile};

use crate::memory::{ROM_PREFIX_BYTES, RomInfo};
use crate::videoport::*;

const INVALID_PARAMETER: u32 = 87;
const INVALID_DATA: u32 = 13;
const NOT_ENOUGH_MEMORY: u32 = 8;

#[derive(Debug)]
pub struct Mapping {
    device: NonNull<c_void>,
    base: NonNull<u8>,
    physical: i64,
    length: u32,
}

impl Mapping {
    unsafe fn new(device: NonNull<c_void>, range: AccessRange) -> Result<Self, u32> {
        if range.in_io_space != 0
            || range.length == 0
            || range.start < 0
            || range.start.checked_add(i64::from(range.length)).is_none()
        {
            return Err(INVALID_DATA);
        }
        let mut length = range.length;
        let mut io_space = 0;
        let mut address = null_mut();
        // SAFETY: discovery supplies a live extension and an OS-claimed memory range.
        let status = unsafe {
            VideoPortMapMemory(
                device.as_ptr(),
                range.start,
                &mut length,
                &mut io_space,
                &mut address,
            )
        };
        if status != 0 {
            return Err(status);
        }
        let mapping = Self {
            device,
            base: NonNull::new(address.cast()).ok_or(NOT_ENOUGH_MEMORY)?,
            physical: range.start,
            length: range.length,
        };
        if length < range.length {
            return Err(NOT_ENOUGH_MEMORY);
        }
        Ok(mapping)
    }

    pub fn length(&self) -> u32 {
        self.length
    }

    pub fn physical(&self) -> i64 {
        self.physical
    }

    pub fn address(&self) -> NonNull<u8> {
        self.base
    }

    pub fn snapshot<const N: usize>(&self, offset: u32) -> Result<[u8; N], u32> {
        let offset = offset as usize;
        if offset
            .checked_add(N)
            .is_none_or(|end| end > self.length as usize)
        {
            return Err(INVALID_DATA);
        }
        let mut bytes = [0; N];
        for (index, byte) in bytes.iter_mut().enumerate() {
            // SAFETY: this mapping is live and each byte is in its requested range.
            unsafe { *byte = read_volatile(self.base.as_ptr().add(offset + index)) };
        }
        Ok(bytes)
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: this value owns a successful mapping; its context remains live.
        unsafe {
            VideoPortUnmapMemory(self.device.as_ptr(), self.base.as_ptr().cast(), null_mut())
        };
    }
}

#[derive(Debug)]
pub struct IoMapping {
    device: NonNull<c_void>,
    base: NonNull<u8>,
    length: u32,
}

impl IoMapping {
    unsafe fn new(device: NonNull<c_void>, range: AccessRange, revision: u8) -> Result<Self, u32> {
        let minimum = match revision {
            2 => 16,
            3 => 24,
            _ => 25,
        };
        if range.in_io_space == 0
            || range.length < minimum
            || range.start < 0
            || range
                .start
                .checked_add(i64::from(range.length))
                .is_none_or(|end| end > 65536)
        {
            return Err(INVALID_DATA);
        }
        // SAFETY: the range was claimed by VideoPortGetAccessRanges for this device.
        let base = unsafe { VideoPortGetDeviceBase(device.as_ptr(), range.start, range.length, 1) };
        Ok(Self {
            device,
            base: NonNull::new(base.cast()).ok_or(NOT_ENOUGH_MEMORY)?,
            length: range.length,
        })
    }

    pub fn port(&self, offset: u32) -> Result<*mut u8, u32> {
        if offset >= self.length {
            return Err(INVALID_PARAMETER);
        }
        // Port addresses are opaque integer values, not Rust memory allocations.
        Ok(self.base.as_ptr().wrapping_add(offset as usize))
    }
}

impl Drop for IoMapping {
    fn drop(&mut self) {
        // SAFETY: this object exclusively owns the successful device-base mapping.
        unsafe { VideoPortFreeDeviceBase(self.device.as_ptr(), self.base.as_ptr().cast()) };
    }
}

#[derive(Debug)]
pub struct Event {
    device: NonNull<c_void>,
    address: NonNull<c_void>,
}

impl Event {
    unsafe fn new(device: NonNull<c_void>) -> Result<Self, u32> {
        let mut event = null_mut();
        // SAFETY: the extension is live, this call is at PASSIVE_LEVEL, and the
        // output points to a writable pointer-sized slot.
        let status = unsafe { VideoPortCreateEvent(device.as_ptr(), 0, null_mut(), &mut event) };
        if status != 0 {
            return Err(status);
        }
        Ok(Self {
            device,
            address: NonNull::new(event).ok_or(NOT_ENOUGH_MEMORY)?,
        })
    }

    pub fn address(&self) -> NonNull<c_void> {
        self.address
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: this object owns the event and the device extension remains live.
        unsafe { VideoPortDeleteEvent(self.device.as_ptr(), self.address.as_ptr()) };
    }
}

#[derive(Debug)]
pub struct Events {
    pub display: Event,
    pub cursor: Event,
    pub sleep: Event,
    pub io_command: Event,
}

#[derive(Debug)]
pub struct DeviceResources {
    pub revision: u8,
    pub mode_count: u32,
    pub info: RomInfo,
    pub events: Events,
    pub ram: Mapping,
    pub vram: Mapping,
    pub rom: Mapping,
    pub io: IoMapping,
}

impl DeviceResources {
    /// # Safety
    /// `device` must be a live VideoPort device extension. Call at PASSIVE_LEVEL
    /// with no concurrent driver access. Retain the extension until these
    /// resources are dropped, also at PASSIVE_LEVEL. Stop interrupts, DPCs and
    /// all display-driver use of the mappings and events before dropping them.
    pub unsafe fn discover(
        device: NonNull<c_void>,
        config: &ConfigInfoPrefix,
    ) -> Result<Self, u32> {
        if config.length < core::mem::size_of::<ConfigInfoPrefix>() as u32
            || config.adapter_interface_type != 5
            || config.bus_interrupt_level == 0 && config.bus_interrupt_vector == 0
        {
            return Err(INVALID_PARAMETER);
        }
        let mut pci = [0u8; 256];
        // SAFETY: the live extension is PCI; pci is writable for the requested length.
        let size =
            unsafe { VideoPortGetBusData(device.as_ptr(), 4, 0, pci.as_mut_ptr().cast(), 0, 256) };
        if size != 256
            || u16::from_le_bytes([pci[0], pci[1]]) != 0x1b36
            || u16::from_le_bytes([pci[2], pci[3]]) != 0x0100
            || pci[8] < 2
        {
            return Err(INVALID_PARAMETER);
        }
        let revision = pci[8];
        let mut ranges = [AccessRange::default(); 4];
        // SAFETY: ranges provides four writable records; optional inputs are null.
        let status = unsafe {
            VideoPortGetAccessRanges(
                device.as_ptr(),
                0,
                null_mut(),
                4,
                ranges.as_mut_ptr(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        if status != 0 {
            return Err(status);
        }
        // SAFETY: discovery owns the claimed ranges and the extension stays live.
        let io = unsafe { IoMapping::new(device, ranges[3], revision) }?;
        // SAFETY: each returned mapping owns its range and rolls back on failure.
        let rom = unsafe { Mapping::new(device, ranges[2]) }?;
        let info = RomInfo::parse(&rom.snapshot::<ROM_PREFIX_BYTES>(0)?, ranges[0].length)
            .map_err(|_| INVALID_DATA)?;
        let mode_count = u32::from_le_bytes(rom.snapshot::<4>(info.mode_table_offset())?);
        let mode_end = mode_count
            .checked_mul(32)
            .and_then(|bytes| bytes.checked_add(4))
            .and_then(|bytes| info.mode_table_offset().checked_add(bytes));
        if mode_count == 0 || mode_end.is_none_or(|end| end > rom.length()) {
            return Err(INVALID_DATA);
        }
        // SAFETY: the ROM layout was validated against this RAM range's length.
        let ram = unsafe { Mapping::new(device, ranges[0]) }?;
        if ram.snapshot::<4>(info.header_offset())? != *b"QXRA" {
            return Err(INVALID_DATA);
        }
        // SAFETY: discovery owns the VRAM range; no display access has started.
        let vram = unsafe { Mapping::new(device, ranges[1]) }?;
        // SAFETY: all four events share the constructor's device and IRQL contract.
        let events = unsafe {
            Events {
                display: Event::new(device)?,
                cursor: Event::new(device)?,
                sleep: Event::new(device)?,
                io_command: Event::new(device)?,
            }
        };
        Ok(Self {
            revision,
            mode_count,
            info,
            events,
            ram,
            vram,
            rom,
            io,
        })
    }
}
