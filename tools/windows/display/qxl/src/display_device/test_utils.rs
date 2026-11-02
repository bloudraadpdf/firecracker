// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::driver_info::MemSlot;

pub(crate) fn pointer<T>(address: usize) -> *mut T {
    core::ptr::without_provenance_mut(address)
}

pub(crate) fn info() -> DriverInfo {
    mapped_info(pointer(0x10000000), pointer(0x20000000))
}

pub(crate) fn mapped_info(ram: *mut u8, rom: *mut u8) -> DriverInfo {
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
            virtual_start: ram.addr() as u64,
            virtual_end: ram.addr() as u64 + 0x3000,
        },
        primary_area: ram,
        primary_area_size: 4096,
        io_pages_virtual: ram.wrapping_add(0x1000).cast(),
        io_pages_physical: 0xe0001000,
        page_count: 2,
        display_event: pointer(0x30000000),
        cursor_event: pointer(0x30000010),
        sleep_event: pointer(0x30000020),
        io_command_event: pointer(0x30000030),
        update_id: rom.wrapping_add(8).cast(),
        compression_level: rom.wrapping_add(12).cast(),
        log_level: rom.wrapping_add(16).cast(),
        clock: rom.wrapping_add(48).cast(),
        slot_generation: rom.wrapping_add(68).cast(),
        framebuffer_physical: 0xf0000000,
        ..DriverInfo::default()
    };
    let header = ram.wrapping_add(0x4000);
    info.command_ring = header.wrapping_add(COMMAND_RING_OFFSET as usize).cast();
    info.cursor_ring = header.wrapping_add(CURSOR_RING_OFFSET as usize).cast();
    info.release_ring = header.wrapping_add(RELEASE_RING_OFFSET as usize).cast();
    info.update_area = header.wrapping_add(UPDATE_AREA_OFFSET as usize).cast();
    info.update_surface = header.wrapping_add(UPDATE_SURFACE_OFFSET as usize).cast();
    info.ram_slot_start = header.wrapping_add(SLOT_START_OFFSET as usize).cast();
    info.ram_slot_end = header.wrapping_add(SLOT_END_OFFSET as usize).cast();
    info.primary_surface_create = header.wrapping_add(PRIMARY_CREATE_OFFSET as usize).cast();
    info.monitors_config = header.wrapping_add(MONITORS_CONFIG_OFFSET as usize).cast();
    info.log_buffer = header.wrapping_add(12);
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
