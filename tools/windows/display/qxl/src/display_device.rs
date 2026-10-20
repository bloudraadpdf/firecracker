// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Display-side miniport interface validation and framebuffer mapping.

use core::ffi::c_void;
use core::mem::{offset_of, size_of};
use core::ptr::NonNull;

use crate::abi::record;
use crate::driver_info::DriverInfo;
use crate::memory::*;
use crate::ring::{CommandRing, ReleaseRing};
use crate::slots::{MemorySlot, SlotLayout};
use crate::surface::Layout;

record! { VideoMemoryInformation {
    video_base: *mut c_void,
    video_length: u32,
    framebuffer_base: *mut c_void,
    framebuffer_length: u32,
}}

const _: () = {
    assert!(offset_of!(VideoMemoryInformation, framebuffer_base) == 2 * size_of::<usize>());
    assert!(size_of::<VideoMemoryInformation>() == 4 * size_of::<usize>());
    assert!(CURSOR_RING_OFFSET as usize - COMMAND_RING_OFFSET as usize == CommandRing::BYTES);
    assert!(RELEASE_RING_OFFSET as usize - CURSOR_RING_OFFSET as usize == CommandRing::BYTES);
    assert!(UPDATE_AREA_OFFSET as usize - RELEASE_RING_OFFSET as usize == ReleaseRing::BYTES);
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Ioctl(u32),
    InvalidResponse,
    InvalidSlot,
    InvalidRange,
    InvalidHeader,
    InvalidRom,
    InvalidEvent,
    InvalidPort,
}

#[derive(Clone, Copy, Debug)]
pub struct Region {
    base: NonNull<u8>,
    length: usize,
}

impl Region {
    pub fn base(self) -> NonNull<u8> {
        self.base
    }
    pub fn len(self) -> usize {
        self.length
    }
    pub fn is_empty(self) -> bool {
        false
    }

    fn new(pointer: *mut u8, length: usize, alignment: usize) -> Result<Self, Error> {
        let base = NonNull::new(pointer).ok_or(Error::InvalidRange)?;
        if length == 0
            || length > isize::MAX as usize
            || !pointer.addr().is_multiple_of(alignment)
            || pointer.addr().checked_add(length).is_none()
        {
            return Err(Error::InvalidRange);
        }
        Ok(Self { base, length })
    }

    fn end(self) -> usize {
        self.base.as_ptr().addr() + self.length
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Interface {
    main_slot: MemorySlot,
    primary: Region,
    pages: Region,
    secondary_slot: Option<u8>,
    header: Region,
    rom: Region,
    framebuffer_physical: u64,
}

impl Interface {
    pub fn validate(info: &DriverInfo, layout: Layout) -> Result<Self, Error> {
        if info.version != 3
            || info.pci_revision < 2
            || info.surface_count == 0
            || info.create_non_primary_surfaces > 1
        {
            return Err(Error::InvalidResponse);
        }
        let slot_layout = SlotLayout::new(info.slot_id_bits, info.slot_generation_bits)
            .map_err(|_| Error::InvalidSlot)?;
        if info.main_slot_id >= info.slot_count
            || u16::from(info.slot_count) > 1u16 << info.slot_id_bits
        {
            return Err(Error::InvalidSlot);
        }
        let slot = info.main_slot;
        let bytes = slot
            .physical_end
            .checked_sub(slot.physical_start)
            .ok_or(Error::InvalidSlot)?;
        if slot.virtual_start == 0
            || slot.virtual_end.checked_sub(slot.virtual_start) != Some(bytes)
            || slot.virtual_end > usize::MAX as u64
        {
            return Err(Error::InvalidSlot);
        }
        let main_slot = slot_layout
            .slot(
                info.main_slot_id,
                slot.generation,
                slot.physical_start,
                slot.virtual_start,
                bytes,
            )
            .map_err(|_| Error::InvalidSlot)?;
        let page_bytes = info
            .page_count
            .checked_mul(4096)
            .ok_or(Error::InvalidRange)? as usize;
        let pages = Region::new(info.io_pages_virtual.cast(), page_bytes, 8)?;
        let primary = Region::new(info.primary_area, info.primary_area_size as usize, 4)?;
        if primary.length < layout.bytes() {
            return Err(Error::InvalidRange);
        }
        for region in [pages, primary] {
            main_slot
                .encode(region.base.as_ptr().addr() as u64, region.length as u64)
                .map_err(|_| Error::InvalidRange)?;
        }
        if primary.base.as_ptr().addr() < pages.end() && pages.base.as_ptr().addr() < primary.end()
        {
            return Err(Error::InvalidRange);
        }
        let offset = (pages.base.as_ptr().addr() as u64)
            .checked_sub(slot.virtual_start)
            .ok_or(Error::InvalidRange)?;
        if slot.physical_start.checked_add(offset) != Some(info.io_pages_physical) {
            return Err(Error::InvalidRange);
        }
        let header = validate_header(info, slot.virtual_end as usize)?;
        let rom = validate_rom(info)?;
        for event in [
            info.display_event,
            info.cursor_event,
            info.sleep_event,
            info.io_command_event,
        ] {
            if event.is_null() {
                return Err(Error::InvalidEvent);
            }
        }
        for port in [
            info.notify_command_port,
            info.notify_cursor_port,
            info.notify_oom_port,
            info.update_area_port,
            info.log_port,
            info.destroy_surface_wait_port,
            info.create_primary_port,
            info.destroy_primary_port,
            info.memslot_add_port,
            info.memslot_delete_port,
            info.destroy_all_surfaces_port,
        ] {
            if port.is_null() {
                return Err(Error::InvalidPort);
            }
        }
        if info.pci_revision >= 4 {
            for port in [
                info.update_area_async_port,
                info.memslot_add_async_port,
                info.create_primary_async_port,
                info.destroy_primary_async_port,
                info.destroy_surface_async_port,
                info.destroy_all_surfaces_async_port,
                info.flush_surfaces_async_port,
                info.flush_release_port,
            ] {
                if port.is_null() {
                    return Err(Error::InvalidPort);
                }
            }
        }
        if info.pci_revision >= 5 && info.monitors_config_port.is_null() {
            return Err(Error::InvalidPort);
        }
        for port in [
            info.notify_command_port,
            info.notify_cursor_port,
            info.notify_oom_port,
            info.update_area_async_port,
            info.memslot_add_async_port,
            info.create_primary_async_port,
            info.destroy_primary_async_port,
            info.destroy_surface_async_port,
            info.destroy_all_surfaces_async_port,
            info.flush_surfaces_async_port,
            info.flush_release_port,
            info.update_area_port,
            info.log_port,
            info.destroy_surface_wait_port,
            info.create_primary_port,
            info.destroy_primary_port,
            info.memslot_add_port,
            info.memslot_delete_port,
            info.destroy_all_surfaces_port,
            info.monitors_config_port,
        ] {
            if port.addr() > u16::MAX as usize {
                return Err(Error::InvalidPort);
            }
        }
        let secondary_slot = info
            .main_slot_id
            .checked_add(1)
            .filter(|id| *id < info.slot_count);
        if info.create_non_primary_surfaces != 0 && secondary_slot.is_none() {
            return Err(Error::InvalidSlot);
        }
        Ok(Self {
            main_slot,
            primary,
            pages,
            secondary_slot,
            header,
            rom,
            framebuffer_physical: info.framebuffer_physical,
        })
    }

    pub fn main_slot(self) -> MemorySlot {
        self.main_slot
    }
    pub fn primary(self) -> Region {
        self.primary
    }
    pub fn pages(self) -> Region {
        self.pages
    }
    pub fn secondary_slot(self) -> Option<u8> {
        self.secondary_slot
    }
    pub fn framebuffer(self, memory: &VideoMemoryInformation) -> Result<Region, Error> {
        if memory.video_base != memory.framebuffer_base
            || memory.video_length != memory.framebuffer_length
        {
            return Err(Error::InvalidResponse);
        }
        let framebuffer = Region::new(
            memory.framebuffer_base.cast(),
            memory.framebuffer_length as usize,
            8,
        )?;
        let (start, end) = self.main_slot.virtual_range();
        let (physical_start, _) = self.main_slot.physical_range();
        let main_start = start as usize;
        let main_end = (end as usize).max(self.header.end());
        let ram_length = main_end
            .checked_sub(main_start)
            .ok_or(Error::InvalidRange)?;
        let ram_physical_end = physical_start
            .checked_add(ram_length as u64)
            .ok_or(Error::InvalidRange)?;
        let physical_end = self
            .framebuffer_physical
            .checked_add(framebuffer.length as u64)
            .ok_or(Error::InvalidRange)?;
        if (framebuffer.base.as_ptr().addr() < main_end && main_start < framebuffer.end())
            || (framebuffer.base.as_ptr().addr() < self.rom.end()
                && self.rom.base.as_ptr().addr() < framebuffer.end())
            || (self.framebuffer_physical < ram_physical_end && physical_start < physical_end)
        {
            return Err(Error::InvalidRange);
        }
        Ok(framebuffer)
    }
}

fn validate_header(info: &DriverInfo, allocations_end: usize) -> Result<Region, Error> {
    let header = info
        .command_ring
        .addr()
        .checked_sub(COMMAND_RING_OFFSET as usize)
        .ok_or(Error::InvalidHeader)?;
    if header < allocations_end
        || header.checked_add(RAM_HEADER_BYTES as usize).is_none()
        || !header.is_multiple_of(4)
    {
        return Err(Error::InvalidHeader);
    }
    for (pointer, offset) in [
        (info.cursor_ring.cast::<u8>(), CURSOR_RING_OFFSET),
        (info.release_ring.cast(), RELEASE_RING_OFFSET),
        (info.update_area.cast(), UPDATE_AREA_OFFSET),
        (info.update_surface.cast(), UPDATE_SURFACE_OFFSET),
        (info.ram_slot_start.cast(), SLOT_START_OFFSET),
        (info.ram_slot_end.cast(), SLOT_END_OFFSET),
        (info.primary_surface_create.cast(), PRIMARY_CREATE_OFFSET),
        (info.monitors_config.cast(), MONITORS_CONFIG_OFFSET),
        (info.log_buffer, 12),
    ] {
        if pointer.addr() != header + offset as usize {
            return Err(Error::InvalidHeader);
        }
    }
    Region::new(
        info.command_ring.cast::<u8>().with_addr(header),
        RAM_HEADER_BYTES as usize,
        4,
    )
    .map_err(|_| Error::InvalidHeader)
}

fn validate_rom(info: &DriverInfo) -> Result<Region, Error> {
    let base = info
        .update_id
        .addr()
        .checked_sub(8)
        .ok_or(Error::InvalidRom)?;
    if base == 0 || !base.is_multiple_of(4) || base.checked_add(ROM_PREFIX_BYTES).is_none() {
        return Err(Error::InvalidRom);
    }
    for (pointer, offset) in [
        (info.compression_level.cast::<u8>(), 12),
        (info.log_level.cast(), 16),
        (info.clock.cast(), 48),
        (info.slot_generation, 68),
    ] {
        if pointer.addr() != base + offset {
            return Err(Error::InvalidRom);
        }
    }
    Region::new(
        info.update_id.cast::<u8>().with_addr(base),
        ROM_PREFIX_BYTES,
        4,
    )
    .map_err(|_| Error::InvalidRom)
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
mod native {
    use super::*;
    use crate::engine::EngDeviceIoControl;
    use crate::ioctl::{GET_INFO, MAP_VIDEO_MEMORY, SET_CURRENT_MODE, UNMAP_VIDEO_MEMORY};

    #[derive(Debug)]
    struct VideoMapping {
        driver: NonNull<c_void>,
        address: NonNull<c_void>,
    }

    impl Drop for VideoMapping {
        fn drop(&mut self) {
            let mut requested = self.address.as_ptr();
            let mut returned = 0;
            // SAFETY: successful mapping retains the live miniport handle. All
            // display and host users must have stopped before this owner drops.
            unsafe {
                EngDeviceIoControl(
                    self.driver.as_ptr(),
                    UNMAP_VIDEO_MEMORY,
                    (&mut requested as *mut *mut c_void).cast(),
                    size_of::<usize>() as u32,
                    core::ptr::null_mut(),
                    0,
                    &mut returned,
                )
            };
        }
    }

    #[derive(Debug)]
    pub struct MappedDevice {
        info: DriverInfo,
        interface: Interface,
        framebuffer: Region,
        mapping: VideoMapping,
    }

    impl MappedDevice {
        /// # Safety
        /// Call at PASSIVE_LEVEL as the serialised display hardware-enable path
        /// with a live miniport handle. Its returned kernel mappings, ROM and
        /// event pointers must remain live until this owner and all dependent
        /// objects are destroyed. Stop all GDI and host access before unmap;
        /// mapping or geometry validation alone does not stop asynchronous users.
        pub unsafe fn new(
            driver: NonNull<c_void>,
            mode: &crate::pdev_config::Configuration,
        ) -> Result<Self, Error> {
            let mut index = mode.mode.mode_index;
            let mut returned = 0;
            // SAFETY: the handle/context are live, input is a complete mode ID.
            let status = unsafe {
                EngDeviceIoControl(
                    driver.as_ptr(),
                    SET_CURRENT_MODE,
                    (&mut index as *mut u32).cast(),
                    4,
                    core::ptr::null_mut(),
                    0,
                    &mut returned,
                )
            };
            if status != 0 {
                return Err(Error::Ioctl(status));
            }
            let mut info = DriverInfo::default();
            // SAFETY: every field admits arbitrary integer/raw-pointer bits;
            // this exclusive initialized output has the complete native extent.
            let status = unsafe {
                EngDeviceIoControl(
                    driver.as_ptr(),
                    GET_INFO,
                    core::ptr::null_mut(),
                    0,
                    (&mut info as *mut DriverInfo).cast(),
                    size_of::<DriverInfo>() as u32,
                    &mut returned,
                )
            };
            if status != 0 {
                return Err(Error::Ioctl(status));
            }
            if returned as usize != size_of::<DriverInfo>() {
                return Err(Error::InvalidResponse);
            }
            let interface = Interface::validate(&info, mode.layout)?;
            let mut requested: *mut c_void = core::ptr::null_mut();
            let mut memory = VideoMemoryInformation::default();
            // SAFETY: a null requested base asks the miniport for a mapping;
            // both native records have their full, initialized writable extents.
            let status = unsafe {
                EngDeviceIoControl(
                    driver.as_ptr(),
                    MAP_VIDEO_MEMORY,
                    (&mut requested as *mut *mut c_void).cast(),
                    size_of::<usize>() as u32,
                    (&mut memory as *mut VideoMemoryInformation).cast(),
                    size_of::<VideoMemoryInformation>() as u32,
                    &mut returned,
                )
            };
            if status != 0 {
                return Err(Error::Ioctl(status));
            }
            let address = NonNull::new(memory.video_base)
                .or_else(|| NonNull::new(memory.framebuffer_base))
                .ok_or(Error::InvalidResponse)?;
            let mapping = VideoMapping { driver, address };
            if returned as usize != size_of::<VideoMemoryInformation>()
                || memory.video_base != memory.framebuffer_base
                || memory.video_length != memory.framebuffer_length
            {
                return Err(Error::InvalidResponse);
            }
            let framebuffer = interface.framebuffer(&memory)?;
            Ok(Self {
                info,
                interface,
                framebuffer,
                mapping,
            })
        }

        pub fn info(&self) -> &DriverInfo {
            &self.info
        }
        pub fn interface(&self) -> Interface {
            self.interface
        }
        pub fn framebuffer(&self) -> Region {
            self.framebuffer
        }
        pub fn driver(&self) -> NonNull<c_void> {
            self.mapping.driver
        }
    }
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub use native::MappedDevice;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver_info::MemSlot;
    use crate::surface::Format;

    fn pointer<T>(address: usize) -> *mut T {
        core::ptr::without_provenance_mut(address)
    }

    fn info() -> DriverInfo {
        let mut info = DriverInfo {
            version: 3,
            pci_revision: 5,
            surface_count: 256,
            create_non_primary_surfaces: 1,
            main_slot_id: 1,
            slot_count: 3,
            slot_id_bits: 8,
            slot_generation_bits: 8,
            main_slot: MemSlot {
                generation: 4,
                physical_start: 0xe0000000,
                physical_end: 0xe0003000,
                virtual_start: 0x10000000,
                virtual_end: 0x10003000,
            },
            primary_area: pointer(0x10000000),
            primary_area_size: 4096,
            io_pages_virtual: pointer(0x10001000),
            io_pages_physical: 0xe0001000,
            page_count: 2,
            display_event: pointer(0x30000000),
            cursor_event: pointer(0x30000010),
            sleep_event: pointer(0x30000020),
            io_command_event: pointer(0x30000030),
            update_id: pointer(0x20000008),
            compression_level: pointer(0x2000000c),
            log_level: pointer(0x20000010),
            clock: pointer(0x20000030),
            slot_generation: pointer(0x20000044),
            framebuffer_physical: 0xf0000000,
            ..DriverInfo::default()
        };
        let header = 0x10004000;
        info.command_ring = pointer(header + COMMAND_RING_OFFSET as usize);
        info.cursor_ring = pointer(header + CURSOR_RING_OFFSET as usize);
        info.release_ring = pointer(header + RELEASE_RING_OFFSET as usize);
        info.update_area = pointer(header + UPDATE_AREA_OFFSET as usize);
        info.update_surface = pointer(header + UPDATE_SURFACE_OFFSET as usize);
        info.ram_slot_start = pointer(header + SLOT_START_OFFSET as usize);
        info.ram_slot_end = pointer(header + SLOT_END_OFFSET as usize);
        info.primary_surface_create = pointer(header + PRIMARY_CREATE_OFFSET as usize);
        info.monitors_config = pointer(header + MONITORS_CONFIG_OFFSET as usize);
        info.log_buffer = pointer(header + 12);
        info.notify_command_port = pointer(0xc000);
        info.notify_cursor_port = pointer(0xc001);
        info.notify_oom_port = pointer(0xc004);
        info.update_area_port = pointer(0xc002);
        info.log_port = pointer(0xc007);
        info.memslot_add_port = pointer(0xc008);
        info.memslot_delete_port = pointer(0xc009);
        info.create_primary_port = pointer(0xc00c);
        info.destroy_primary_port = pointer(0xc00d);
        info.destroy_surface_wait_port = pointer(0xc00e);
        info.destroy_all_surfaces_port = pointer(0xc00f);
        info.update_area_async_port = pointer(0xc010);
        info.memslot_add_async_port = pointer(0xc011);
        info.create_primary_async_port = pointer(0xc012);
        info.destroy_primary_async_port = pointer(0xc013);
        info.destroy_surface_async_port = pointer(0xc014);
        info.destroy_all_surfaces_async_port = pointer(0xc015);
        info.flush_surfaces_async_port = pointer(0xc016);
        info.flush_release_port = pointer(0xc017);
        info.monitors_config_port = pointer(0xc018);
        info
    }

    fn layout() -> Layout {
        Layout::new(8, 8, Format::Xrgb8888).unwrap()
    }

    #[test]
    fn valid_interface_keeps_control_header_outside_the_allocation_slot() {
        let info = info();
        let interface = Interface::validate(&info, layout()).unwrap();
        assert_eq!(interface.primary().len(), 4096);
        assert_eq!(interface.pages().len(), 8192);
        assert_eq!(interface.secondary_slot(), Some(2));
        assert_eq!(
            interface.main_slot().virtual_range(),
            (0x10000000, 0x10003000)
        );
        assert_eq!(
            interface
                .main_slot()
                .encode(interface.pages().base().as_ptr().addr() as u64, 8192),
            Ok(0x0104000000001000)
        );
        assert!(
            interface
                .main_slot()
                .encode(info.command_ring.addr() as u64, CommandRing::BYTES as u64)
                .is_err()
        );
        assert_eq!(info.ram_slot_start.addr() % 8, 4);
    }

    #[test]
    fn allocation_ranges_and_physical_offsets_must_agree() {
        for altered in [
            DriverInfo {
                primary_area_size: 128,
                ..info()
            },
            DriverInfo {
                io_pages_physical: 0xe0001001,
                ..info()
            },
            DriverInfo {
                page_count: u32::MAX,
                ..info()
            },
            DriverInfo {
                io_pages_virtual: pointer(0x10000000),
                io_pages_physical: 0xe0000000,
                ..info()
            },
            DriverInfo {
                primary_area: pointer(0x10002ff0),
                ..info()
            },
        ] {
            assert!(matches!(
                Interface::validate(&altered, layout()),
                Err(Error::InvalidRange)
            ));
        }
    }

    #[test]
    fn malformed_control_rom_events_and_ports_are_rejected() {
        let mut overlapping = info();
        overlapping.main_slot.virtual_end += 8192;
        overlapping.main_slot.physical_end += 8192;
        assert!(matches!(
            Interface::validate(&overlapping, layout()),
            Err(Error::InvalidHeader)
        ));
        assert!(matches!(
            Interface::validate(
                &DriverInfo {
                    cursor_ring: pointer(0x10004000),
                    ..info()
                },
                layout()
            ),
            Err(Error::InvalidHeader)
        ));
        assert!(matches!(
            Interface::validate(
                &DriverInfo {
                    clock: pointer(0x20000034),
                    ..info()
                },
                layout()
            ),
            Err(Error::InvalidRom)
        ));
        assert!(matches!(
            Interface::validate(
                &DriverInfo {
                    display_event: core::ptr::null_mut(),
                    ..info()
                },
                layout()
            ),
            Err(Error::InvalidEvent)
        ));
        for port in [pointer(0x10000), core::ptr::null_mut()] {
            assert!(matches!(
                Interface::validate(
                    &DriverInfo {
                        notify_command_port: port,
                        ..info()
                    },
                    layout()
                ),
                Err(Error::InvalidPort)
            ));
        }
        assert!(matches!(
            Interface::validate(
                &DriverInfo {
                    create_primary_async_port: core::ptr::null_mut(),
                    ..info()
                },
                layout()
            ),
            Err(Error::InvalidPort)
        ));
    }

    #[test]
    fn slot_generation_and_optional_secondary_capacity_are_validated() {
        let mut invalid = info();
        invalid.main_slot.physical_end += 1;
        assert!(matches!(
            Interface::validate(&invalid, layout()),
            Err(Error::InvalidSlot)
        ));
        assert!(matches!(
            Interface::validate(
                &DriverInfo {
                    slot_generation_bits: 2,
                    ..info()
                },
                layout()
            ),
            Err(Error::InvalidSlot)
        ));
        assert!(matches!(
            Interface::validate(
                &DriverInfo {
                    slot_count: 2,
                    ..info()
                },
                layout()
            ),
            Err(Error::InvalidSlot)
        ));
        let primary_only = DriverInfo {
            slot_count: 2,
            create_non_primary_surfaces: 0,
            ..info()
        };
        assert_eq!(
            Interface::validate(&primary_only, layout())
                .unwrap()
                .secondary_slot(),
            None
        );
        assert!(matches!(
            Interface::validate(
                &DriverInfo {
                    version: 2,
                    ..info()
                },
                layout()
            ),
            Err(Error::InvalidResponse)
        ));
    }

    #[test]
    fn framebuffer_mapping_rejects_aliases_to_allocations_header_rom_and_physical_ram() {
        let info = info();
        let interface = Interface::validate(&info, layout()).unwrap();
        let mapping = VideoMemoryInformation {
            video_base: pointer(0x40000000),
            video_length: 4096,
            framebuffer_base: pointer(0x40000000),
            framebuffer_length: 4096,
        };
        assert_eq!(interface.framebuffer(&mapping).unwrap().len(), 4096);
        for address in [0x10000000, 0x10004000, 0x20000000] {
            assert!(matches!(
                interface.framebuffer(&VideoMemoryInformation {
                    video_base: pointer(address),
                    framebuffer_base: pointer(address),
                    ..mapping
                }),
                Err(Error::InvalidRange)
            ));
        }
        assert!(matches!(
            interface.framebuffer(&VideoMemoryInformation {
                framebuffer_length: 4095,
                ..mapping
            }),
            Err(Error::InvalidResponse)
        ));
        let alias = DriverInfo {
            framebuffer_physical: 0xe0004000,
            ..info
        };
        let interface = Interface::validate(&alias, layout()).unwrap();
        assert!(matches!(
            interface.framebuffer(&mapping),
            Err(Error::InvalidRange)
        ));
    }
}
