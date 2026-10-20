// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Windows VideoPort ABI. See the Microsoft video.h DDI references in README.md.

use core::ffi::c_void;
use core::mem::{offset_of, size_of};

pub type FindAdapter = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    *mut u16,
    *mut ConfigInfoPrefix,
    *mut u8,
) -> u32;
pub type Initialize = unsafe extern "system" fn(*mut c_void) -> u8;
pub type Interrupt = unsafe extern "system" fn(*mut c_void) -> u8;
pub type StartIo = unsafe extern "system" fn(*mut c_void, *mut RequestPacket) -> u8;
pub type Power = unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> u32;
pub type ChildDescriptor = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    *mut i32,
    *mut u8,
    *mut u32,
    *mut u32,
) -> u32;
pub type Dpc = unsafe extern "system" fn(*mut c_void, *mut c_void);
pub type RegistryQuery =
    unsafe extern "system" fn(*mut c_void, *mut c_void, *mut u16, *mut c_void, u32) -> u32;

/// The miniport reads only this stable prefix of VIDEO_PORT_CONFIG_INFO.
#[repr(C)]
#[derive(Debug)]
pub struct ConfigInfoPrefix {
    pub length: u32,
    pub system_io_bus_number: u32,
    pub adapter_interface_type: i32,
    pub bus_interrupt_level: u32,
    pub bus_interrupt_vector: u32,
    pub interrupt_mode: i32,
}

#[repr(C)]
#[derive(Debug)]
pub struct InitializationData {
    pub size: u32,
    pub adapter_interface_type: i32,
    pub find_adapter: Option<FindAdapter>,
    pub initialize: Option<Initialize>,
    pub interrupt: Option<Interrupt>,
    pub start_io: Option<StartIo>,
    pub device_extension_size: u32,
    pub starting_device_number: u32,
    pub reset_hw: *const c_void,
    pub timer: *const c_void,
    pub start_dma: *const c_void,
    pub set_power_state: Option<Power>,
    pub get_power_state: Option<Power>,
    pub get_child_descriptor: Option<ChildDescriptor>,
    pub query_interface: *const c_void,
    pub child_device_extension_size: u32,
    pub legacy_resource_list: *mut AccessRange,
    pub legacy_resource_count: u32,
    pub get_legacy_resources: *const c_void,
    pub allow_early_enumeration: u8,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AccessRange {
    pub start: i64,
    pub length: u32,
    pub in_io_space: u8,
    pub visible: u8,
    pub shareable: u8,
    pub passive: u8,
}

#[repr(C)]
#[derive(Debug)]
pub struct StatusBlock {
    pub status: u32,
    pub information: usize,
}

#[repr(C)]
#[derive(Debug)]
pub struct RequestPacket {
    pub ioctl: u32,
    pub status: *mut StatusBlock,
    pub input: *mut c_void,
    pub input_length: u32,
    pub output: *mut c_void,
    pub output_length: u32,
}

const _: () = {
    assert!(size_of::<ConfigInfoPrefix>() == 24);
    assert!(offset_of!(ConfigInfoPrefix, bus_interrupt_level) == 12);
    assert!(size_of::<AccessRange>() == 16);
    assert!(offset_of!(AccessRange, in_io_space) == 12);
    assert!(offset_of!(InitializationData, find_adapter) == 8);
};

#[cfg(target_arch = "x86")]
const _: () = {
    assert!(size_of::<InitializationData>() == 84);
    assert!(offset_of!(InitializationData, set_power_state) == 44);
    assert!(offset_of!(InitializationData, allow_early_enumeration) == 76);
    assert!(size_of::<RequestPacket>() == 24);
    assert!(offset_of!(RequestPacket, output) == 16);
    assert!(size_of::<StatusBlock>() == 8);
};

#[cfg(target_arch = "x86_64")]
const _: () = {
    assert!(size_of::<InitializationData>() == 144);
    assert!(offset_of!(InitializationData, set_power_state) == 72);
    assert!(offset_of!(InitializationData, allow_early_enumeration) == 136);
    assert!(size_of::<RequestPacket>() == 48);
    assert!(offset_of!(RequestPacket, output) == 32);
    assert!(size_of::<StatusBlock>() == 16);
};

#[cfg_attr(
    target_arch = "x86",
    link(
        name = "videoprt.sys",
        kind = "raw-dylib",
        modifiers = "+verbatim",
        import_name_type = "undecorated"
    )
)]
#[cfg_attr(
    target_arch = "x86_64",
    link(name = "videoprt.sys", kind = "raw-dylib", modifiers = "+verbatim")
)]
unsafe extern "system" {
    pub fn VideoPortInitialize(
        argument1: *mut c_void,
        argument2: *mut c_void,
        initialization: *mut InitializationData,
        context: *mut c_void,
    ) -> u32;
    pub fn VideoPortGetAccessRanges(
        device: *mut c_void,
        requested_count: u32,
        requested_resources: *mut c_void,
        range_count: u32,
        ranges: *mut AccessRange,
        vendor_id: *mut c_void,
        device_id: *mut c_void,
        slot: *mut u32,
    ) -> u32;
    pub fn VideoPortGetBusData(
        device: *mut c_void,
        bus_data_type: i32,
        slot: u32,
        buffer: *mut c_void,
        offset: u32,
        length: u32,
    ) -> u32;
    pub fn VideoPortMapMemory(
        device: *mut c_void,
        physical: i64,
        length: *mut u32,
        io_space: *mut u32,
        virtual_address: *mut *mut c_void,
    ) -> u32;
    pub fn VideoPortUnmapMemory(
        device: *mut c_void,
        virtual_address: *mut c_void,
        process: *mut c_void,
    ) -> u32;
    pub fn VideoPortGetDeviceBase(
        device: *mut c_void,
        physical: i64,
        length: u32,
        io_space: u8,
    ) -> *mut c_void;
    pub fn VideoPortFreeDeviceBase(device: *mut c_void, address: *mut c_void);
    pub fn VideoPortAllocatePool(
        device: *mut c_void,
        pool_type: i32,
        bytes: usize,
        tag: u32,
    ) -> *mut c_void;
    pub fn VideoPortFreePool(device: *mut c_void, allocation: *mut c_void);
    pub fn VideoPortCreateEvent(
        device: *mut c_void,
        flags: u32,
        unused: *mut c_void,
        event: *mut *mut c_void,
    ) -> u32;
    pub fn VideoPortDeleteEvent(device: *mut c_void, event: *mut c_void) -> u32;
    pub fn VideoPortSetEvent(device: *mut c_void, event: *mut c_void) -> i32;
    pub fn VideoPortWaitForSingleObject(
        device: *mut c_void,
        object: *mut c_void,
        timeout: *mut i64,
    ) -> u32;
    pub fn VideoPortQueueDpc(device: *mut c_void, callback: Dpc, context: *mut c_void) -> u8;
    pub fn VideoPortInterlockedExchange(target: *mut i32, value: i32) -> i32;
    pub fn VideoPortWritePortUchar(port: *mut u8, value: u8);
    pub fn VideoPortGetRegistryParameters(
        device: *mut c_void,
        name: *mut u16,
        is_file: u8,
        query: RegistryQuery,
        context: *mut c_void,
    ) -> u32;
    pub fn VideoPortSetRegistryParameters(
        device: *mut c_void,
        name: *mut u16,
        value: *mut c_void,
        length: u32,
    ) -> u32;
    pub fn VideoPortGetVersion(device: *mut c_void, version: *mut c_void) -> u32;
    pub fn VideoPortLogError(
        device: *mut c_void,
        packet: *mut RequestPacket,
        error: u32,
        unique_id: u32,
    );
}
