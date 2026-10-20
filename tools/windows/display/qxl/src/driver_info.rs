// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Version 3 display/miniport interface from XDDM qxl_driver.h.

use core::ffi::c_void;
use core::mem::{offset_of, size_of};

pub const GET_INFO: u32 = 0x232000;
pub const SET_CUSTOM_DISPLAY: u32 = 0x232004;

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default)]
pub struct MemSlot {
    pub generation: u8,
    pub physical_start: u64,
    pub physical_end: u64,
    pub virtual_start: u64,
    pub virtual_end: u64,
}

trait Field: Copy {
    fn write(self, output: &mut [u8]);
}

macro_rules! integer_field {
    ($($ty:ty),+) => {$(
        impl Field for $ty {
            fn write(self, output: &mut [u8]) {
                output.copy_from_slice(&self.to_le_bytes());
            }
        }
    )+};
}
integer_field!(u8, u32, u64);

impl<T> Field for *mut T {
    fn write(self, output: &mut [u8]) {
        output.copy_from_slice(&(self as usize).to_le_bytes());
    }
}

impl Field for MemSlot {
    fn write(self, output: &mut [u8]) {
        output.fill(0);
        output[0] = self.generation;
        for (index, value) in [
            self.physical_start,
            self.physical_end,
            self.virtual_start,
            self.virtual_end,
        ]
        .into_iter()
        .enumerate()
        {
            let offset = 8 + index * 8;
            output[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
    }
}

macro_rules! driver_info {
    ($($field:ident: $ty:ty),+ $(,)?) => {
        #[repr(C)]
        #[derive(Clone, Copy, Debug)]
        pub struct DriverInfo { $(pub $field: $ty),+ }

        impl Default for DriverInfo {
            fn default() -> Self {
                // SAFETY: every field is an integer, raw pointer, or MemSlot,
                // each of which admits the all-zero representation.
                unsafe { core::mem::zeroed() }
            }
        }

        impl DriverInfo {
            /// Encode fields individually so kernel padding never reaches the caller.
            pub fn encode(&self, output: &mut [u8]) -> Option<usize> {
                let size = size_of::<Self>();
                let output = output.get_mut(..size)?;
                output.fill(0);
                $(
                    let offset = offset_of!(Self, $field);
                    Field::write(self.$field, &mut output[offset..offset + size_of::<$ty>()]);
                )+
                Some(size)
            }
        }
    };
}

driver_info! {
    version: u32,
    command_ring: *mut c_void,
    cursor_ring: *mut c_void,
    release_ring: *mut c_void,
    notify_command_port: *mut u8,
    notify_cursor_port: *mut u8,
    notify_oom_port: *mut u8,
    update_area_async_port: *mut u8,
    memslot_add_async_port: *mut u8,
    create_primary_async_port: *mut u8,
    destroy_primary_async_port: *mut u8,
    destroy_surface_async_port: *mut u8,
    destroy_all_surfaces_async_port: *mut u8,
    flush_surfaces_async_port: *mut u8,
    flush_release_port: *mut u8,
    display_event: *mut c_void,
    cursor_event: *mut c_void,
    sleep_event: *mut c_void,
    io_command_event: *mut c_void,
    page_count: u32,
    io_pages_virtual: *mut c_void,
    io_pages_physical: u64,
    primary_area: *mut u8,
    primary_area_size: u32,
    update_id: *mut u32,
    compression_level: *mut u32,
    update_area_port: *mut u8,
    update_area: *mut c_void,
    update_surface: *mut u32,
    clock: *mut u32,
    log_port: *mut u8,
    log_buffer: *mut u8,
    log_level: *mut u32,
    slot_count: u8,
    main_slot_id: u8,
    slot_id_bits: u8,
    slot_generation_bits: u8,
    slot_generation: *mut u8,
    ram_slot_start: *mut u64,
    ram_slot_end: *mut u64,
    main_slot: MemSlot,
    destroy_surface_wait_port: *mut u8,
    create_primary_port: *mut u8,
    destroy_primary_port: *mut u8,
    memslot_add_port: *mut u8,
    memslot_delete_port: *mut u8,
    destroy_all_surfaces_port: *mut u8,
    monitors_config_port: *mut u8,
    pci_revision: u8,
    device_id: u32,
    primary_surface_create: *mut c_void,
    surface_count: u32,
    framebuffer_physical: u64,
    create_non_primary_surfaces: u8,
    monitors_config: *mut u64,
}

const _: () = {
    assert!(size_of::<MemSlot>() == 40);
    assert!(offset_of!(MemSlot, physical_start) == 8);
};

#[cfg(all(windows, target_pointer_width = "32"))]
const _: () = {
    assert!(size_of::<DriverInfo>() == 264);
    assert!(offset_of!(DriverInfo, main_slot) == 160);
    assert!(offset_of!(DriverInfo, pci_revision) == 228);
    assert!(offset_of!(DriverInfo, monitors_config) == 260);
};

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<DriverInfo>() == 440);
    assert!(offset_of!(DriverInfo, main_slot) == 296);
    assert!(offset_of!(DriverInfo, pci_revision) == 392);
    assert!(offset_of!(DriverInfo, monitors_config) == 432);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_outputs_are_unchanged() {
        let mut output = [0xa5; size_of::<DriverInfo>() - 1];
        assert_eq!(DriverInfo::default().encode(&mut output), None);
        assert!(output.iter().all(|&byte| byte == 0xa5));
    }

    #[test]
    fn encode_clears_padding_and_preserves_output_tail() {
        let info = DriverInfo {
            version: 3,
            main_slot: MemSlot {
                generation: 7,
                physical_start: 0x1122334455667788,
                ..MemSlot::default()
            },
            ..DriverInfo::default()
        };
        let mut output = [0xa5; size_of::<DriverInfo>() + 1];
        assert_eq!(info.encode(&mut output), Some(size_of::<DriverInfo>()));
        let mut expected = [0; size_of::<DriverInfo>() + 1];
        expected[0] = 3;
        let slot = offset_of!(DriverInfo, main_slot);
        expected[slot] = 7;
        expected[slot + 8..slot + 16].copy_from_slice(&0x1122334455667788u64.to_le_bytes());
        expected[size_of::<DriverInfo>()] = 0xa5;
        assert_eq!(output, expected);
    }
}
