// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Development-only XP x86 PnP miniport for Firecracker's linear framebuffer.
#![cfg_attr(not(test), no_std)]

pub mod abi;

use abi::*;
use core::ffi::c_void;
use core::mem::{MaybeUninit, size_of, size_of_val};
use core::ptr::null_mut;

#[cfg(all(not(test), not(all(target_arch = "x86", target_os = "windows"))))]
compile_error!("The kernel prototype supports only XP x86; use host unit tests for local checks.");

const INVALID: u32 = 87;
const NO_MEMORY: u32 = 8;
const IO_ERROR: u32 = 1117;
const REINITIALIZE: u32 = 1164;
const REGISTER_BYTES: u32 = 4096;
const ADAPTER_ID: u32 = u32::MAX;
const MONITOR_ID: u32 = 1;
const POWER_ON: u32 = 1;
const POWER_OFF: u32 = 4;
const POWER_SHUTDOWN: u32 = 6;

#[cfg(not(test))]
mod port {
    use super::*;

    #[link(name = "videoprt")]
    unsafe extern "system" {
        #[link_name = "VideoPortInitialize"]
        pub fn initialize(
            a: *mut c_void,
            b: *mut c_void,
            data: *mut InitializationData,
            context: *mut c_void,
        ) -> u32;
        #[link_name = "VideoPortGetAccessRanges"]
        pub fn ranges(
            extension: *mut c_void,
            requested_count: u32,
            requested: *mut c_void,
            count: u32,
            ranges: *mut AccessRange,
            vendor: *mut c_void,
            device: *mut c_void,
            slot: *mut u32,
        ) -> u32;
        #[link_name = "VideoPortVerifyAccessRanges"]
        pub fn release(extension: *mut c_void, count: u32, ranges: *mut AccessRange) -> u32;
        #[link_name = "VideoPortGetBusData"]
        pub fn pci(
            extension: *mut c_void,
            bus_type: u32,
            slot: u32,
            output: *mut c_void,
            offset: u32,
            length: u32,
        ) -> u32;
        #[link_name = "VideoPortGetDeviceBase"]
        pub fn device_base(
            extension: *mut c_void,
            physical: i64,
            length: u32,
            space: u8,
        ) -> *mut c_void;
        #[link_name = "VideoPortFreeDeviceBase"]
        pub fn free_base(extension: *mut c_void, address: *mut c_void);
        #[link_name = "VideoPortReadRegisterUshort"]
        pub fn read(register: *mut u16) -> u16;
        #[link_name = "VideoPortWriteRegisterUshort"]
        pub fn write(register: *mut u16, value: u16);
        #[link_name = "VideoPortReadRegisterUchar"]
        pub fn read_byte(register: *mut u8) -> u8;
        #[link_name = "VideoPortWriteRegisterUchar"]
        pub fn write_byte(register: *mut u8, value: u8);
        #[link_name = "VideoPortMapMemory"]
        pub fn map(
            extension: *mut c_void,
            physical: i64,
            length: *mut u32,
            space: *mut u32,
            address: *mut *mut c_void,
        ) -> u32;
        #[link_name = "VideoPortUnmapMemory"]
        pub fn unmap(extension: *mut c_void, address: *mut c_void, process: *mut c_void) -> u32;
        #[link_name = "VideoPortMoveMemory"]
        pub fn copy(destination: *mut c_void, source: *const c_void, length: u32);
        #[link_name = "VideoPortZeroMemory"]
        pub fn zero(destination: *mut c_void, length: u32);
        #[link_name = "VideoPortSetRegistryParameters"]
        pub fn set_registry(
            extension: *mut c_void,
            name: *mut u16,
            data: *mut c_void,
            length: u32,
        ) -> u32;
        #[link_name = "VideoPortLogError"]
        pub fn log(extension: *mut c_void, packet: *mut RequestPacket, status: u32, unique_id: u32);
        #[link_name = "KeBugCheckEx"]
        pub fn bugcheck(code: u32, a: usize, b: usize, c: usize, d: usize) -> !;
    }
}

#[repr(C)]
struct Adapter {
    registers: *mut u8,
    framebuffer: u32,
    mapped: *mut c_void,
    mapping_valid: u8,
    mapped_length: u32,
    references: u32,
    initial: [u16; 11],
    clock: u8,
    tentative_claim: u8,
    port_resources: u8,
    ready: u8,
    mode: Option<ModeIndex>,
    poisoned: u8,
    cleanup_pending: u8,
    power_error: u32,
    power: u32,
}

#[derive(Debug, PartialEq, Eq)]
struct Resources {
    framebuffer: u32,
    registers: u32,
}

fn resources(header: &[u8; 64], ranges: &[AccessRange]) -> Result<Resources, u32> {
    if header[..4] != [0x34, 0x12, 0x11, 0x11]
        || header[4] & 2 == 0
        || header[8..12] != [1, 0, 0, 3]
        || header[14] != 0
        || header[44..48] != [0x34, 0x12, 0x11, 0x11]
    {
        return Err(INVALID);
    }
    let framebuffer = u32::from_le_bytes([header[16], header[17], header[18], header[19]]);
    let registers = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    if framebuffer & 15 != 8 || registers & 15 != 0 {
        return Err(INVALID);
    }
    let framebuffer = framebuffer & !15;
    let registers = registers & !15;
    let framebuffer_end = u64::from(framebuffer) + u64::from(FRAMEBUFFER_BYTES);
    let register_end = u64::from(registers) + u64::from(REGISTER_BYTES);
    if framebuffer == 0
        || registers == 0
        || !framebuffer.is_multiple_of(FRAMEBUFFER_BYTES)
        || !registers.is_multiple_of(REGISTER_BYTES)
        || framebuffer_end > 1 << 32
        || register_end > 1 << 32
        || u64::from(framebuffer) < register_end && u64::from(registers) < framebuffer_end
    {
        return Err(INVALID);
    }
    for (base, length) in [
        (framebuffer, FRAMEBUFFER_BYTES),
        (registers, REGISTER_BYTES),
    ] {
        let mut matches = ranges.iter().filter(|range| range.start == i64::from(base));
        let range = matches.next().ok_or(INVALID)?;
        if matches.next().is_some() || range.length != length || range.in_io_space != 0 {
            return Err(INVALID);
        }
    }
    Ok(Resources {
        framebuffer,
        registers,
    })
}

fn aligned<T>(pointer: *mut T) -> bool {
    !pointer.is_null() && pointer.is_aligned()
}

// The video port owns and serialises these callback buffers and the adapter extension.
unsafe fn adapter<'a>(extension: *mut c_void) -> Result<&'a mut Adapter, u32> {
    let pointer = extension.cast::<Adapter>();
    if !aligned(pointer) {
        return Err(INVALID);
    }
    Ok(unsafe { &mut *pointer })
}

unsafe fn read(adapter: &Adapter, index: usize) -> u16 {
    unsafe { port::read(adapter.registers.add(0x500 + index * 2).cast()) }
}

unsafe fn write(adapter: &Adapter, index: usize, value: u16) {
    unsafe { port::write(adapter.registers.add(0x500 + index * 2).cast(), value) }
}

unsafe fn disable(adapter: &Adapter) -> Result<(), u32> {
    unsafe { write(adapter, 4, 0) };
    if unsafe { read(adapter, 4) } != 0 {
        return Err(IO_ERROR);
    }
    Ok(())
}

unsafe fn clock(adapter: &Adapter, value: Option<u8>) -> u8 {
    let index = unsafe { adapter.registers.add(0x404) };
    let data = unsafe { adapter.registers.add(0x405) };
    let previous = unsafe { port::read_byte(index) };
    unsafe { port::write_byte(index, 1) };
    if let Some(value) = value {
        unsafe { port::write_byte(data, value) };
    }
    let observed = unsafe { port::read_byte(data) };
    unsafe { port::write_byte(index, previous) };
    observed
}

unsafe fn blank(adapter: &Adapter, value: bool) -> Result<(), u32> {
    let desired = if value {
        adapter.clock | 0x20
    } else {
        adapter.clock
    };
    if unsafe { clock(adapter, Some(desired)) } != desired {
        return Err(IO_ERROR);
    }
    Ok(())
}

unsafe fn program(adapter: &mut Adapter, mode: ModeIndex, clear: bool) -> Result<(), u32> {
    if let Err(error) = unsafe { disable(adapter) } {
        adapter.poisoned = 1;
        adapter.mode = None;
        return Err(error);
    }
    let information = mode.information();
    let (width, height) = (information.width as u16, information.height as u16);
    let enable = if clear { 0x41 } else { 0xc1 };
    for (index, value) in [
        (1, width),
        (2, height),
        (3, 32),
        (5, 0),
        (4, enable),
        (6, width),
        (8, 0),
        (9, 0),
    ] {
        unsafe { write(adapter, index, value) };
    }
    for (index, expected) in [
        (1, width),
        (2, height),
        (3, 32),
        (4, enable),
        (5, 0),
        (6, width),
        (8, 0),
        (9, 0),
    ] {
        if unsafe { read(adapter, index) } != expected {
            adapter.poisoned = 1;
            adapter.mode = None;
            return Err(IO_ERROR);
        }
    }
    adapter.mode = Some(mode);
    adapter.poisoned = 0;
    Ok(())
}

unsafe fn reset(adapter: &mut Adapter) -> Result<(), u32> {
    adapter.mode = None;
    adapter.poisoned = 1;
    unsafe { disable(adapter)? };
    unsafe { blank(adapter, false)? };
    if adapter.initial[4] & 1 != 0 {
        for index in [1, 2, 3, 5] {
            unsafe { write(adapter, index, adapter.initial[index]) };
        }
        unsafe { write(adapter, 4, adapter.initial[4] | 0x80) };
        for index in [6, 8, 9] {
            unsafe { write(adapter, index, adapter.initial[index]) };
        }
        for index in [1, 2, 3, 4, 6, 8, 9] {
            let expected = if index == 4 {
                adapter.initial[index] | 0x80
            } else {
                adapter.initial[index]
            };
            if unsafe { read(adapter, index) } != expected {
                adapter.poisoned = 1;
                return Err(IO_ERROR);
            }
        }
    }
    adapter.mode = None;
    adapter.poisoned = 0;
    Ok(())
}

unsafe fn release_mapping(extension: *mut c_void, adapter: &mut Adapter) -> Result<(), u32> {
    if !adapter.mapped.is_null() {
        let status = unsafe { port::unmap(extension, adapter.mapped, null_mut()) };
        if status != 0 {
            return Err(status);
        }
        adapter.mapped = null_mut();
        adapter.mapping_valid = 0;
        adapter.mapped_length = 0;
        adapter.references = 0;
    }
    Ok(())
}

unsafe fn cleanup(extension: *mut c_void, adapter: &mut Adapter) -> Result<(), u32> {
    let result = unsafe { release_mapping(extension, adapter) };
    if !adapter.registers.is_null() {
        unsafe { port::free_base(extension, adapter.registers.cast()) };
        adapter.registers = null_mut();
    }
    adapter.ready = 0;
    adapter.mode = None;
    adapter.cleanup_pending = u8::from(!adapter.mapped.is_null());
    result
}

// Claim release is valid only while executing HwFindAdapter.
unsafe fn abandon_discovery(extension: *mut c_void, adapter: &mut Adapter) -> Result<(), u32> {
    let mut result = unsafe { cleanup(extension, adapter) };
    if adapter.tentative_claim != 0 && adapter.mapped.is_null() {
        let status = unsafe { port::release(extension, 0, null_mut()) };
        if status == 0 {
            adapter.tentative_claim = 0;
        } else if result.is_ok() {
            result = Err(status);
        }
    }
    result
}

unsafe fn hardware_information(extension: *mut c_void) {
    let mut adapter_name = b"HardwareInformation.AdapterString\0".map(u16::from);
    let mut adapter = b"Firecracker linear framebuffer\0".map(u16::from);
    let mut memory_name = b"HardwareInformation.MemorySize\0".map(u16::from);
    let mut memory = FRAMEBUFFER_BYTES;
    for (name, data, length, id) in [
        (
            adapter_name.as_mut_ptr(),
            adapter.as_mut_ptr().cast(),
            size_of_val(&adapter) as u32,
            0x4656_4241,
        ),
        (
            memory_name.as_mut_ptr(),
            (&mut memory as *mut u32).cast(),
            size_of_val(&memory) as u32,
            0x4656_424d,
        ),
    ] {
        let status = unsafe { port::set_registry(extension, name, data, length) };
        if status != 0 {
            unsafe { port::log(extension, null_mut(), status, id) };
        }
    }
}

unsafe extern "system" fn find_adapter(
    extension: *mut c_void,
    _context: *mut c_void,
    _arguments: *mut u16,
    config: *mut c_void,
    again: *mut u8,
) -> u32 {
    let Ok(adapter) = (unsafe { adapter(extension) }) else {
        return INVALID;
    };
    let config = config.cast::<PortConfiguration>();
    if !aligned(config)
        || !aligned(again)
        || unsafe { (*config).length } < size_of::<PortConfiguration>() as u32
        || unsafe { (*config).interface_type } != 5
    {
        return INVALID;
    }
    unsafe { *again = 0 };
    if adapter.ready != 0 || adapter.port_resources != 0 || !adapter.mapped.is_null() {
        return INVALID;
    }
    if (adapter.tentative_claim != 0 || !adapter.registers.is_null())
        && let Err(error) = unsafe { abandon_discovery(extension, adapter) }
    {
        return error;
    }
    let mut ranges = MaybeUninit::<[AccessRange; 8]>::uninit();
    unsafe {
        port::zero(
            ranges.as_mut_ptr().cast(),
            size_of::<[AccessRange; 8]>() as u32,
        )
    };
    let mut slot = 0;
    let status = unsafe {
        port::ranges(
            extension,
            0,
            null_mut(),
            8,
            ranges.as_mut_ptr().cast(),
            null_mut(),
            null_mut(),
            &mut slot,
        )
    };
    if status != 0 {
        return status;
    }
    adapter.tentative_claim = 1;
    let outcome = (|| {
        let mut header = MaybeUninit::<[u8; 64]>::uninit();
        if unsafe { port::pci(extension, 4, slot, header.as_mut_ptr().cast(), 0, 64) } != 64 {
            return Err(INVALID);
        }
        let resources = resources(unsafe { header.assume_init_ref() }, unsafe {
            ranges.assume_init_ref()
        })?;
        adapter.framebuffer = resources.framebuffer;
        adapter.registers = unsafe {
            port::device_base(extension, i64::from(resources.registers), REGISTER_BYTES, 0)
        }
        .cast();
        if adapter.registers.is_null() {
            return Err(NO_MEMORY);
        }
        if adapter.registers.addr() & 1 != 0 {
            return Err(INVALID);
        }
        if !(0xb0c0..=0xb0c5).contains(&unsafe { read(adapter, 0) })
            || unsafe { read(adapter, 10) } != 256
        {
            return Err(INVALID);
        }
        for index in 0..11 {
            adapter.initial[index] = unsafe { read(adapter, index) };
        }
        adapter.clock = unsafe { clock(adapter, None) };
        unsafe {
            (*config).emulator_count = 0;
            (*config).emulator_entries = null_mut();
            (*config).emulator_context = 0;
            (*config).vdm_address = 0;
            (*config).vdm_length = 0;
            (*config).hardware_state_size = 0;
            (*config).interrupt_level = 0;
            (*config).interrupt_vector = 0;
        }
        adapter.tentative_claim = 0;
        adapter.port_resources = 1;
        adapter.ready = 1;
        adapter.power = POWER_ON;
        unsafe { hardware_information(extension) };
        Ok(())
    })();
    match outcome {
        Ok(()) => 0,
        Err(error) => {
            let _ = unsafe { abandon_discovery(extension, adapter) };
            error
        }
    }
}

unsafe extern "system" fn initialize(extension: *mut c_void) -> u8 {
    u8::from(
        unsafe { adapter(extension) }
            .is_ok_and(|adapter| adapter.ready != 0 && adapter.poisoned == 0),
    )
}

unsafe fn request(extension: *mut c_void, packet: &RequestPacket) -> Result<usize, u32> {
    let adapter = unsafe { adapter(extension)? };
    for (pointer, length) in [
        (packet.input, packet.input_length),
        (packet.output, packet.output_length),
    ] {
        if length != 0
            && (pointer.is_null()
                || length as usize > isize::MAX as usize
                || pointer.addr().checked_add(length as usize).is_none())
        {
            return Err(INVALID);
        }
    }
    let input = if packet.input_length == 0 {
        &[]
    } else {
        unsafe {
            core::slice::from_raw_parts(
                packet.input.cast::<u8>(),
                packet.input_length.min(4) as usize,
            )
        }
    };
    let operation = decode_request(packet.control_code, input, packet.output_length as usize)
        .map_err(|error| error as u32)?;
    if !matches!(operation, Operation::Unmap { .. })
        && (adapter.ready == 0 || adapter.poisoned != 0 || adapter.power != POWER_ON)
    {
        return Err(IO_ERROR);
    }
    match operation {
        Operation::QueryModes | Operation::QueryCurrentMode => {
            let (source, length) = if operation == Operation::QueryModes {
                (MODES.as_ptr(), MODE_BYTES)
            } else {
                (
                    adapter.mode.ok_or(INVALID)?.information() as *const ModeInformation,
                    size_of::<ModeInformation>(),
                )
            };
            unsafe { port::copy(packet.output, source.cast(), length as u32) };
            Ok(length)
        }
        Operation::QueryModeCount => {
            static COUNT: NumberOfModes = NumberOfModes {
                count: MODES.len() as u32,
                information_length: size_of::<ModeInformation>() as u32,
            };
            unsafe {
                port::copy(
                    packet.output,
                    (&COUNT as *const NumberOfModes).cast(),
                    size_of::<NumberOfModes>() as u32,
                )
            };
            Ok(size_of::<NumberOfModes>())
        }
        Operation::SetMode { mode, clear_memory } => {
            unsafe { program(adapter, mode, clear_memory)? };
            Ok(0)
        }
        Operation::Reset => {
            unsafe { reset(adapter)? };
            Ok(0)
        }
        Operation::Map { requested_address } => {
            if requested_address != 0 {
                return Err(INVALID);
            }
            let visible_bytes = adapter.mode.ok_or(INVALID)?.visible_bytes();
            if packet.output_length < size_of::<MemoryInformation>() as u32 {
                return Err(RequestError::InsufficientBuffer as u32);
            }
            let references = adapter.references.checked_add(1).ok_or(NO_MEMORY)?;
            if adapter.mapped.is_null() {
                let mut length = FRAMEBUFFER_BYTES;
                let mut space = 0;
                let mut address = null_mut();
                let status = unsafe {
                    port::map(
                        extension,
                        i64::from(adapter.framebuffer),
                        &mut length,
                        &mut space,
                        &mut address,
                    )
                };
                if status != 0 {
                    return Err(status);
                }
                if address.is_null() {
                    return Err(IO_ERROR);
                }
                adapter.mapped = address;
                adapter.mapped_length = length;
                adapter.mapping_valid = 0;
                adapter.references = 1;
                if length != FRAMEBUFFER_BYTES || space != 0 || address.addr() & 4095 != 0 {
                    adapter.poisoned = 1;
                    let _ = unsafe { release_mapping(extension, adapter) };
                    return Err(IO_ERROR);
                }
                adapter.mapping_valid = 1;
            }
            if adapter.mapping_valid == 0 {
                return Err(IO_ERROR);
            }
            adapter.references = references;
            let mut information = MaybeUninit::<MemoryInformation>::uninit();
            let information = information.as_mut_ptr();
            unsafe {
                port::zero(information.cast(), size_of::<MemoryInformation>() as u32);
                (*information).video_base = adapter.mapped;
                (*information).video_length = adapter.mapped_length;
                (*information).framebuffer_base = adapter.mapped;
                (*information).framebuffer_length = visible_bytes;
                port::copy(
                    packet.output,
                    information.cast(),
                    size_of::<MemoryInformation>() as u32,
                );
            }
            Ok(size_of::<MemoryInformation>())
        }
        Operation::Unmap { address } => {
            if adapter.mapped.is_null()
                || adapter.mapped.addr() != address as usize
                || adapter.references == 0
            {
                return Err(INVALID);
            }
            if adapter.references > 1 {
                adapter.references -= 1;
            } else {
                unsafe { release_mapping(extension, adapter)? };
            }
            Ok(0)
        }
    }
}

unsafe extern "system" fn start_io(extension: *mut c_void, packet: *mut RequestPacket) -> u8 {
    if !aligned(packet) {
        return 0;
    }
    let packet = unsafe { &*packet };
    if !aligned(packet.status) {
        return 0;
    }
    unsafe {
        (*packet.status).information = 0;
        (*packet.status).value.status = INVALID;
    }
    match unsafe { request(extension, packet) } {
        Ok(length) => unsafe {
            (*packet.status).value.status = 0;
            (*packet.status).information = length;
        },
        Err(status) => unsafe {
            (*packet.status).value.status = status;
        },
    }
    1
}

unsafe extern "system" fn reset_hardware(extension: *mut c_void, _columns: u32, _rows: u32) -> u8 {
    let pointer = extension.cast::<Adapter>();
    if !aligned(pointer) || unsafe { (*pointer).registers.is_null() } {
        return 0;
    }
    // Bugcheck callbacks cannot call mapping or other passive-level services.
    unsafe {
        let registers = (*pointer).registers;
        let register = registers.add(0x508).cast();
        port::write(register, 0);
        let index = registers.add(0x404);
        let data = registers.add(0x405);
        let previous = port::read_byte(index);
        port::write_byte(index, 1);
        port::write_byte(data, port::read_byte(data) & !0x20);
        port::write_byte(index, previous);
    }
    0
}

unsafe extern "system" fn get_power(
    extension: *mut c_void,
    id: u32,
    power: *mut PowerManagement,
) -> u32 {
    if !aligned(power) || unsafe { (*power).length } < size_of::<PowerManagement>() as u32 {
        return REINITIALIZE;
    }
    let state = unsafe { (*power).power_state };
    let shutdown = state == POWER_SHUTDOWN && id == ADAPTER_ID;
    if ![ADAPTER_ID, MONITOR_ID].contains(&id)
        || !matches!(state, POWER_ON | POWER_OFF) && !shutdown
    {
        return REINITIALIZE;
    }
    if !unsafe { adapter(extension) }
        .is_ok_and(|adapter| adapter.ready != 0 || shutdown && adapter.port_resources != 0)
    {
        return REINITIALIZE;
    }
    0
}

unsafe fn transition_power(extension: *mut c_void, id: u32, power: *mut PowerManagement) -> u32 {
    if unsafe { get_power(extension, id, power) } != 0 {
        return INVALID;
    }
    let adapter = match unsafe { adapter(extension) } {
        Ok(adapter) => adapter,
        Err(error) => return error,
    };
    let state = unsafe { (*power).power_state };
    if adapter.ready == 0 && state == POWER_SHUTDOWN {
        return match unsafe { cleanup(extension, adapter) } {
            Ok(()) => 0,
            Err(error) => error,
        };
    }
    if adapter.ready == 0 {
        return INVALID;
    }
    let operation = if state == POWER_ON {
        if !adapter.mapped.is_null()
            && adapter.mapping_valid == 0
            && let Err(error) = unsafe { release_mapping(extension, adapter) }
        {
            return error;
        }
        if let Err(error) = unsafe { blank(adapter, false) } {
            return error;
        }
        if let Some(mode) = adapter.mode {
            unsafe { program(adapter, mode, false) }
        } else {
            unsafe { reset(adapter) }
        }
    } else {
        unsafe { disable(adapter) }.and_then(|()| unsafe { blank(adapter, true) })
    };
    if let Err(error) = operation {
        adapter.poisoned = 1;
        return error;
    }
    adapter.power = state;
    if state == POWER_SHUTDOWN && id == ADAPTER_ID {
        return match unsafe { cleanup(extension, adapter) } {
            Ok(()) => 0,
            Err(error) => error,
        };
    }
    0
}

unsafe extern "system" fn set_power(
    extension: *mut c_void,
    id: u32,
    power: *mut PowerManagement,
) -> u32 {
    let error = unsafe { transition_power(extension, id, power) };
    if let Ok(adapter) = unsafe { adapter(extension) } {
        adapter.power_error = error;
        if error != 0 {
            adapter.poisoned = 1;
            unsafe { port::log(extension, null_mut(), error, 0x4656_4250) };
        }
    }
    0
}

unsafe extern "system" fn child(
    extension: *mut c_void,
    info: *mut ChildEnumeration,
    child_type: *mut u32,
    _descriptor: *mut u8,
    id: *mut u32,
    unused: *mut u32,
) -> u32 {
    if !unsafe { adapter(extension) }.is_ok_and(|adapter| adapter.ready != 0)
        || !aligned(info)
        || !aligned(child_type)
        || !aligned(id)
        || !aligned(unused)
        || unsafe { (*info).size } < size_of::<ChildEnumeration>() as u32
    {
        return 123;
    }
    unsafe {
        *unused = 0;
    }
    let index = unsafe { (*info).child_index };
    if index == ADAPTER_ID {
        unsafe {
            *child_type = 3;
            *id = ADAPTER_ID;
        }
    } else if index == 1 || index == 0 && unsafe { (*info).acpi_hardware_id } == MONITOR_ID {
        unsafe {
            *child_type = 1;
            *id = MONITOR_ID;
        }
    } else {
        return 1248;
    }
    1246
}

#[cfg(not(test))]
#[unsafe(no_mangle)]
/// # Safety
/// Called once by the XP kernel loader with its driver object and registry path.
pub unsafe extern "system" fn DriverEntry(driver: *mut c_void, registry: *mut c_void) -> u32 {
    let mut initialization = MaybeUninit::<InitializationData>::uninit();
    let data = initialization.as_mut_ptr();
    unsafe {
        port::zero(data.cast(), size_of::<InitializationData>() as u32);
        (*data).size = size_of::<InitializationData>() as u32;
        (*data).interface_type = 5;
        (*data).find_adapter = Some(find_adapter);
        (*data).initialize = Some(initialize);
        (*data).start_io = Some(start_io);
        (*data).extension_size = size_of::<Adapter>() as u32;
        (*data).reset_hardware = Some(reset_hardware);
        (*data).set_power = Some(set_power);
        (*data).get_power = Some(get_power);
        (*data).child_descriptor = Some(child);
        port::initialize(driver, registry, data, null_mut())
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    unsafe { port::bugcheck(0xe2, 0x4643_5647, 0, 0, 0) }
}

#[cfg(test)]
mod port {
    use super::*;
    use std::cell::RefCell;

    pub(super) struct Mock {
        pub header: [u8; 64],
        pub ranges: [AccessRange; 8],
        pub registers: Box<[u16; 2048]>,
        pub range_status: u32,
        pub pci_length: u32,
        pub base_fails: bool,
        pub ignored_write: Option<usize>,
        pub ignored_value: Option<(usize, u16)>,
        pub ignore_clock_write: bool,
        pub map_status: u32,
        pub map_length: u32,
        pub unmap_status: u32,
        pub release_status: u32,
        pub clock: u8,
        pub sequencer_index: u8,
        pub calls: Vec<&'static str>,
        pub writes: Vec<(usize, u16)>,
        pub logged_errors: Vec<(u32, u32)>,
        pub registry_writes: Vec<(Vec<u16>, Vec<u8>)>,
        pub registry_status: [u32; 2],
    }

    impl Mock {
        fn new() -> Self {
            let mut header = [0; 64];
            header[..4].copy_from_slice(&[0x34, 0x12, 0x11, 0x11]);
            header[4] = 2;
            header[8..12].copy_from_slice(&[1, 0, 0, 3]);
            header[16..20].copy_from_slice(&0xc100_0008u32.to_le_bytes());
            header[24..28].copy_from_slice(&0xc000_1000u32.to_le_bytes());
            header[44..48].copy_from_slice(&[0x34, 0x12, 0x11, 0x11]);
            let mut ranges = core::array::from_fn(|_| AccessRange {
                start: 0,
                length: 0,
                in_io_space: 0,
                visible: 0,
                shareable: 0,
                passive: 0,
            });
            ranges[0].start = 0xc100_0000;
            ranges[0].length = FRAMEBUFFER_BYTES;
            ranges[1].start = 0xc000_1000;
            ranges[1].length = REGISTER_BYTES;
            let mut registers = Box::new([0; 2048]);
            registers[0x500 / 2] = 0xb0c5;
            registers[0x514 / 2] = 256;
            Self {
                header,
                ranges,
                registers,
                range_status: 0,
                pci_length: 64,
                base_fails: false,
                ignored_write: None,
                ignored_value: None,
                ignore_clock_write: false,
                map_status: 0,
                map_length: FRAMEBUFFER_BYTES,
                unmap_status: 0,
                release_status: 0,
                clock: 1,
                sequencer_index: 3,
                calls: Vec::new(),
                writes: Vec::new(),
                logged_errors: Vec::new(),
                registry_writes: Vec::new(),
                registry_status: [0; 2],
            }
        }
    }

    thread_local! { static MOCK: RefCell<Mock> = RefCell::new(Mock::new()); }
    pub(super) fn with<R>(call: impl FnOnce(&mut Mock) -> R) -> R {
        MOCK.with(|mock| call(&mut mock.borrow_mut()))
    }
    pub(super) fn fresh() {
        with(|mock| *mock = Mock::new());
    }

    pub(super) unsafe extern "system" fn ranges(
        _extension: *mut c_void,
        requested_count: u32,
        requested: *mut c_void,
        count: u32,
        output: *mut AccessRange,
        vendor: *mut c_void,
        device: *mut c_void,
        slot: *mut u32,
    ) -> u32 {
        assert_eq!((requested_count, count), (0, 8));
        assert!(requested.is_null() && vendor.is_null() && device.is_null());
        with(|mock| {
            mock.calls.push("ranges");
            if mock.range_status == 0 {
                unsafe {
                    core::ptr::copy_nonoverlapping(mock.ranges.as_ptr(), output, 8);
                    *slot = 0x10;
                }
            }
            mock.range_status
        })
    }
    pub(super) unsafe extern "system" fn release(
        _extension: *mut c_void,
        count: u32,
        ranges: *mut AccessRange,
    ) -> u32 {
        assert_eq!(count, 0);
        assert!(ranges.is_null());
        with(|mock| {
            mock.calls.push("release");
            mock.release_status
        })
    }
    pub(super) unsafe extern "system" fn pci(
        _extension: *mut c_void,
        bus_type: u32,
        slot: u32,
        output: *mut c_void,
        offset: u32,
        length: u32,
    ) -> u32 {
        assert_eq!((bus_type, slot, offset, length), (4, 0x10, 0, 64));
        with(|mock| {
            mock.calls.push("pci");
            unsafe {
                core::ptr::copy_nonoverlapping(
                    mock.header.as_ptr(),
                    output.cast(),
                    mock.pci_length.min(64) as usize,
                );
            }
            mock.pci_length
        })
    }
    pub(super) unsafe extern "system" fn device_base(
        _extension: *mut c_void,
        physical: i64,
        length: u32,
        space: u8,
    ) -> *mut c_void {
        assert_eq!((physical, length, space), (0xc000_1000, REGISTER_BYTES, 0));
        with(|mock| {
            mock.calls.push("base");
            if mock.base_fails {
                null_mut()
            } else {
                mock.registers.as_mut_ptr().cast()
            }
        })
    }
    pub(super) unsafe extern "system" fn free_base(_extension: *mut c_void, address: *mut c_void) {
        with(|mock| {
            assert_eq!(address, mock.registers.as_mut_ptr().cast());
            mock.calls.push("free");
        });
    }
    fn register_offset(mock: &Mock, register: *mut u16) -> usize {
        let offset = register.addr() - mock.registers.as_ptr().addr();
        assert_eq!(offset & 1, 0);
        assert!((0x500..=0x514).contains(&offset));
        offset
    }
    pub(super) unsafe extern "system" fn read(register: *mut u16) -> u16 {
        with(|mock| {
            let offset = register_offset(mock, register);
            mock.registers[offset / 2]
        })
    }
    pub(super) unsafe extern "system" fn write(register: *mut u16, value: u16) {
        with(|mock| {
            let offset = register_offset(mock, register);
            let index = (offset - 0x500) / 2;
            mock.writes.push((index, value));
            if mock.ignored_write != Some(index) && mock.ignored_value != Some((index, value)) {
                mock.registers[offset / 2] = value;
            }
        });
    }
    pub(super) unsafe extern "system" fn read_byte(register: *mut u8) -> u8 {
        with(
            |mock| match register.addr() - mock.registers.as_ptr().addr() {
                0x404 => mock.sequencer_index,
                0x405 => {
                    assert_eq!(mock.sequencer_index, 1);
                    mock.clock
                }
                _ => panic!("unexpected byte register"),
            },
        )
    }
    pub(super) unsafe extern "system" fn write_byte(register: *mut u8, value: u8) {
        with(
            |mock| match register.addr() - mock.registers.as_ptr().addr() {
                0x404 => mock.sequencer_index = value,
                0x405 => {
                    assert_eq!(mock.sequencer_index, 1);
                    if !mock.ignore_clock_write {
                        mock.clock = value;
                    }
                }
                _ => panic!("unexpected byte register"),
            },
        );
    }
    pub(super) unsafe extern "system" fn map(
        _extension: *mut c_void,
        physical: i64,
        length: *mut u32,
        space: *mut u32,
        address: *mut *mut c_void,
    ) -> u32 {
        assert_eq!(physical, 0xc100_0000);
        assert_eq!(
            unsafe { (*length, *space, *address) },
            (FRAMEBUFFER_BYTES, 0, null_mut())
        );
        with(|mock| {
            mock.calls.push("map");
            if mock.map_status == 0 {
                unsafe {
                    *length = mock.map_length;
                    *address = core::ptr::without_provenance_mut(0x9000);
                }
            }
            mock.map_status
        })
    }
    pub(super) unsafe extern "system" fn unmap(
        _extension: *mut c_void,
        address: *mut c_void,
        process: *mut c_void,
    ) -> u32 {
        assert_eq!(address.addr(), 0x9000);
        assert!(process.is_null());
        with(|mock| {
            mock.calls.push("unmap");
            mock.unmap_status
        })
    }
    pub(super) unsafe extern "system" fn copy(
        destination: *mut c_void,
        source: *const c_void,
        length: u32,
    ) {
        unsafe {
            core::ptr::copy(source.cast::<u8>(), destination.cast(), length as usize);
        }
    }
    pub(super) unsafe extern "system" fn zero(destination: *mut c_void, length: u32) {
        unsafe {
            core::ptr::write_bytes(destination.cast::<u8>(), 0, length as usize);
        }
    }
    pub(super) unsafe extern "system" fn set_registry(
        extension: *mut c_void,
        name: *mut u16,
        data: *mut c_void,
        length: u32,
    ) -> u32 {
        assert!(!extension.is_null());
        assert_eq!(name.addr() % align_of::<u16>(), 0);
        assert!(!data.is_null());
        let mut key = Vec::new();
        for index in 0..64 {
            let value = unsafe { *name.add(index) };
            key.push(value);
            if value == 0 {
                break;
            }
        }
        assert_eq!(key.last(), Some(&0));
        let bytes = unsafe { core::slice::from_raw_parts(data.cast::<u8>(), length as usize) };
        if key
            == "HardwareInformation.MemorySize"
                .encode_utf16()
                .chain([0])
                .collect::<Vec<_>>()
        {
            assert_eq!(data.addr() % align_of::<u32>(), 0);
        }
        with(|mock| {
            let index = mock.registry_writes.len();
            assert!(index < 2);
            mock.registry_writes.push((key, bytes.to_vec()));
            mock.registry_status[index]
        })
    }
    pub(super) unsafe extern "system" fn log(
        _extension: *mut c_void,
        packet: *mut RequestPacket,
        status: u32,
        unique_id: u32,
    ) {
        assert!(packet.is_null());
        with(|mock| {
            mock.calls.push("log");
            mock.logged_errors.push((status, unique_id));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discover(adapter: &mut Adapter) -> u32 {
        let mut config: PortConfiguration = unsafe { core::mem::zeroed() };
        config.length = size_of::<PortConfiguration>() as u32;
        config.interface_type = 5;
        let mut again = 1;
        let status = unsafe {
            find_adapter(
                (adapter as *mut Adapter).cast(),
                null_mut(),
                null_mut(),
                (&mut config as *mut PortConfiguration).cast(),
                &mut again,
            )
        };
        assert_eq!(again, 0);
        status
    }
    fn fixture() -> Box<Adapter> {
        port::fresh();
        let mut adapter = Box::new(unsafe { core::mem::zeroed() });
        assert_eq!(discover(&mut adapter), 0);
        assert_eq!(
            unsafe { initialize((&mut *adapter as *mut Adapter).cast()) },
            1
        );
        adapter
    }
    fn ioctl(
        adapter: &mut Adapter,
        code: u32,
        input: &mut [u8],
        output: &mut [u8],
    ) -> (u32, usize) {
        let mut status = StatusBlock {
            value: StatusValue { status: u32::MAX },
            information: usize::MAX,
        };
        let mut packet = RequestPacket {
            control_code: code,
            status: &mut status,
            input: input.as_mut_ptr().cast(),
            input_length: input.len() as u32,
            output: output.as_mut_ptr().cast(),
            output_length: output.len() as u32,
        };
        assert_eq!(
            unsafe { start_io((adapter as *mut Adapter).cast(), &mut packet) },
            1
        );
        (unsafe { status.value.status }, status.information)
    }
    fn set_index(adapter: &mut Adapter, index: u32, flags: u32) {
        assert_eq!(
            ioctl(
                adapter,
                SET_MODE,
                &mut (index | flags).to_le_bytes(),
                &mut []
            ),
            (0, 0)
        );
    }

    fn set_mode(adapter: &mut Adapter) {
        set_index(adapter, 0, 0);
    }

    fn current_mode(adapter: &mut Adapter) -> ModeInformation {
        let mut output = [0; 80];
        assert_eq!(
            ioctl(adapter, QUERY_CURRENT_MODE, &mut [], &mut output),
            (0, 80)
        );
        unsafe { core::ptr::read_unaligned(output.as_ptr().cast()) }
    }

    fn map_framebuffer(adapter: &mut Adapter) -> MemoryInformation {
        let mut output = [0xcc; size_of::<MemoryInformation>()];
        assert_eq!(
            ioctl(adapter, MAP_MEMORY, &mut 0u32.to_le_bytes(), &mut output),
            (0, size_of::<MemoryInformation>())
        );
        unsafe { core::ptr::read_unaligned(output.as_ptr().cast()) }
    }

    fn rejected_current(adapter: &mut Adapter, status: u32) {
        let mut output = [0xcc; 80];
        let actual = ioctl(adapter, QUERY_CURRENT_MODE, &mut [], &mut output);
        assert_eq!(actual, (status, 0));
        assert!(output.iter().all(|byte| *byte == 0xcc));
    }

    fn rejected_map(adapter: &mut Adapter) -> [u8; size_of::<MemoryInformation>()] {
        let mut output = [0xcc; size_of::<MemoryInformation>()];
        let actual = ioctl(adapter, MAP_MEMORY, &mut 0u32.to_le_bytes(), &mut output);
        assert_eq!(actual, (IO_ERROR, 0));
        assert!(output.iter().all(|byte| *byte == 0xcc));
        output
    }

    fn mapped_fixture() -> Box<Adapter> {
        let mut adapter = fixture();
        set_mode(&mut adapter);
        let mut output = [0; size_of::<MemoryInformation>()];
        assert_eq!(
            ioctl(
                &mut adapter,
                MAP_MEMORY,
                &mut 0u32.to_le_bytes(),
                &mut output
            ),
            (0, size_of::<MemoryInformation>())
        );
        adapter
    }

    fn invalid_mapping_fixture() -> (Box<Adapter>, [u8; size_of::<MemoryInformation>()]) {
        let mut adapter = fixture();
        set_mode(&mut adapter);
        port::with(|mock| {
            mock.map_length = FRAMEBUFFER_BYTES - 1;
            mock.unmap_status = 17;
        });
        let output = rejected_map(&mut adapter);
        (adapter, output)
    }

    fn finish_unmap(adapter: &mut Adapter) {
        port::with(|mock| mock.unmap_status = 0);
        assert_eq!(
            ioctl(adapter, UNMAP_MEMORY, &mut 0x9000u32.to_le_bytes(), &mut []),
            (0, 0)
        );
        assert!(adapter.mapped.is_null());
    }

    #[test]
    fn discovery_registers_only_truthful_adapter_information() {
        let mut adapter = fixture();
        assert_eq!(
            (
                adapter.ready,
                adapter.port_resources,
                adapter.tentative_claim
            ),
            (1, 1, 0)
        );
        port::with(|mock| {
            let names = [
                "HardwareInformation.AdapterString",
                "HardwareInformation.MemorySize",
            ];
            assert_eq!(mock.registry_writes.len(), names.len());
            for ((name, _), expected) in mock.registry_writes.iter().zip(names) {
                assert_eq!(
                    *name,
                    expected.encode_utf16().chain([0]).collect::<Vec<_>>()
                );
            }
            assert_eq!(
                mock.registry_writes[0].1,
                "Firecracker linear framebuffer"
                    .encode_utf16()
                    .chain([0])
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>()
            );
            assert_eq!(mock.registry_writes[1].1, 16_777_216u32.to_le_bytes());
            assert!(mock.logged_errors.is_empty());
        });
        set_mode(&mut adapter);
    }

    #[test]
    fn rejected_hardware_does_not_publish_adapter_information() {
        for failure in 0..3 {
            port::fresh();
            port::with(|mock| match failure {
                0 => mock.header[0] = 0,
                1 => mock.registers[0x500 / 2] = 0,
                _ => mock.registers[0x514 / 2] = 0,
            });
            let mut adapter: Adapter = unsafe { core::mem::zeroed() };
            assert_eq!(discover(&mut adapter), INVALID);
            assert_eq!(
                (
                    adapter.ready,
                    adapter.port_resources,
                    adapter.tentative_claim
                ),
                (0, 0, 0)
            );
            port::with(|mock| assert!(mock.registry_writes.is_empty()));
        }
    }

    #[test]
    fn information_failure_is_logged_without_rejecting_the_adapter() {
        for statuses in [[7, 0], [0, 87], [7, 87]] {
            port::fresh();
            port::with(|mock| mock.registry_status = statuses);
            let mut adapter: Adapter = unsafe { core::mem::zeroed() };
            assert_eq!(discover(&mut adapter), 0);
            assert_eq!(
                (
                    adapter.ready,
                    adapter.port_resources,
                    adapter.tentative_claim
                ),
                (1, 1, 0)
            );
            port::with(|mock| {
                assert_eq!(mock.registry_writes.len(), 2);
                let expected: Vec<_> = statuses
                    .into_iter()
                    .zip([0x4656_4241, 0x4656_424d])
                    .filter(|(status, _)| *status != 0)
                    .collect();
                assert_eq!(mock.logged_errors, expected);
                assert!(!mock.calls.contains(&"release"));
            });
            assert_eq!(
                unsafe { initialize((&mut adapter as *mut Adapter).cast()) },
                1
            );
            set_mode(&mut adapter);
        }
    }

    #[test]
    fn peer_reset_hardware_requires_hal_character_mode_fallback() {
        let mut adapter = fixture();
        set_mode(&mut adapter);
        assert_eq!(
            unsafe { reset_hardware((&mut *adapter as *mut Adapter).cast(), 80, 25) },
            0
        );
    }

    #[test]
    fn peer_power_shutdown_must_not_release_accepted_pnp_assignments() {
        let mut adapter = fixture();
        set_mode(&mut adapter);
        port::with(|mock| mock.calls.clear());
        let mut power = PowerManagement {
            length: size_of::<PowerManagement>() as u32,
            dpms_version: 0,
            power_state: POWER_SHUTDOWN,
        };
        unsafe {
            set_power(
                (&mut *adapter as *mut Adapter).cast(),
                ADAPTER_ID,
                &mut power,
            );
        }
        port::with(|mock| assert!(!mock.calls.contains(&"release")));
    }

    #[test]
    fn peer_reset_clock_failure_must_invalidate_the_current_mode() {
        let mut adapter = fixture();
        set_mode(&mut adapter);
        port::with(|mock| {
            mock.clock = 0x21;
            mock.ignore_clock_write = true;
        });
        assert_eq!(
            ioctl(&mut adapter, RESET_DEVICE, &mut [], &mut []),
            (IO_ERROR, 0)
        );
        let mut output = [0xcc; 80];
        assert_eq!(
            ioctl(&mut adapter, QUERY_CURRENT_MODE, &mut [], &mut output),
            (IO_ERROR, 0)
        );
        assert_eq!(output, [0xcc; 80]);
    }

    #[test]
    fn peer_power_acknowledgement_does_not_erase_failed_cleanup() {
        let mut adapter = mapped_fixture();
        port::with(|mock| mock.unmap_status = 7);
        let mut power = PowerManagement {
            length: size_of::<PowerManagement>() as u32,
            dpms_version: 0,
            power_state: POWER_SHUTDOWN,
        };
        assert_eq!(
            unsafe {
                set_power(
                    (&mut *adapter as *mut Adapter).cast(),
                    ADAPTER_ID,
                    &mut power,
                )
            },
            0
        );
        assert_eq!(
            (
                adapter.power_error,
                adapter.cleanup_pending,
                adapter.references
            ),
            (7, 1, 1)
        );
    }

    #[test]
    fn peer_power_on_must_not_reuse_an_invalid_retained_mapping() {
        let (mut adapter, mut output) = invalid_mapping_fixture();
        let mut power = PowerManagement {
            length: size_of::<PowerManagement>() as u32,
            dpms_version: 0,
            power_state: POWER_ON,
        };
        let extension = (&mut *adapter as *mut Adapter).cast();
        assert_eq!(unsafe { set_power(extension, ADAPTER_ID, &mut power) }, 0);
        assert_eq!(adapter.power_error, 17);
        assert_eq!(
            ioctl(
                &mut adapter,
                MAP_MEMORY,
                &mut 0u32.to_le_bytes(),
                &mut output
            ),
            (IO_ERROR, 0)
        );
        port::with(|mock| mock.unmap_status = 0);
        assert_eq!(unsafe { set_power(extension, ADAPTER_ID, &mut power) }, 0);
        assert_eq!(adapter.power_error, 0);
        assert!(adapter.mapped.is_null());
    }

    #[test]
    fn discovery_validates_and_releases_each_partial_acquisition() {
        for case in 0..6 {
            port::fresh();
            port::with(|mock| match case {
                0 => mock.range_status = 5,
                1 => mock.pci_length = 63,
                2 => mock.header[0] = 0,
                3 => mock.base_fails = true,
                4 => mock.registers[0x500 / 2] = 0,
                _ => mock.registers[0x514 / 2] = 255,
            });
            let mut adapter: Adapter = unsafe { core::mem::zeroed() };
            assert_eq!(
                discover(&mut adapter),
                if case == 0 {
                    5
                } else if case == 3 {
                    NO_MEMORY
                } else {
                    INVALID
                }
            );
            assert_eq!((adapter.ready, adapter.tentative_claim), (0, 0));
            assert!(adapter.registers.is_null());
            port::with(|mock| {
                assert_eq!(mock.calls.contains(&"release"), case != 0);
                assert_eq!(mock.calls.contains(&"free"), case >= 4);
            });
        }
    }

    #[test]
    fn pci_resources_reject_decode_identity_range_and_overflow_errors() {
        port::fresh();
        port::with(|mock| {
            assert_eq!(
                resources(&mock.header, &mock.ranges),
                Ok(Resources {
                    framebuffer: 0xc100_0000,
                    registers: 0xc000_1000
                })
            );
            for offset in [0, 4, 8, 9, 11, 14, 44] {
                let saved = mock.header[offset];
                mock.header[offset] ^= 0x80;
                if offset == 4 {
                    mock.header[offset] = 0;
                }
                assert_eq!(resources(&mock.header, &mock.ranges), Err(INVALID));
                mock.header[offset] = saved;
            }
            mock.ranges[0].length -= 1;
            assert_eq!(resources(&mock.header, &mock.ranges), Err(INVALID));
            mock.ranges[0].length += 1;
            mock.ranges[0].in_io_space = 1;
            assert_eq!(resources(&mock.header, &mock.ranges), Err(INVALID));
            mock.ranges[0].in_io_space = 0;
            mock.header[16..20].copy_from_slice(&0xff00_0008u32.to_le_bytes());
            mock.ranges[0].start = 0xff00_0000;
            assert!(resources(&mock.header, &mock.ranges).is_ok());
            mock.header[16..20].copy_from_slice(&0xfff0_0008u32.to_le_bytes());
            mock.ranges[0].start = 0xfff0_0000;
            assert_eq!(resources(&mock.header, &mock.ranges), Err(INVALID));
        });
    }

    #[test]
    fn mode_programming_checks_readback_and_never_claims_an_unknown_mode() {
        let mut adapter = fixture();
        let mut output = [0xcc; 80];
        assert_eq!(
            ioctl(&mut adapter, QUERY_CURRENT_MODE, &mut [], &mut output),
            (INVALID, 0)
        );
        assert_eq!(output, [0xcc; 80]);
        set_mode(&mut adapter);
        port::with(|mock| {
            assert_eq!(
                mock.writes,
                [
                    (4, 0),
                    (1, 640),
                    (2, 480),
                    (3, 32),
                    (5, 0),
                    (4, 0x41),
                    (6, 640),
                    (8, 0),
                    (9, 0)
                ]
            )
        });
        assert_eq!(
            ioctl(&mut adapter, QUERY_CURRENT_MODE, &mut [], &mut output),
            (0, 80)
        );
        let information =
            unsafe { core::ptr::read_unaligned(output.as_ptr().cast::<ModeInformation>()) };
        assert_eq!(information, MODES[0]);
        port::with(|mock| {
            mock.ignored_write = Some(1);
            mock.registers[0x502 / 2] = 0;
        });
        assert_eq!(
            ioctl(&mut adapter, SET_MODE, &mut 0u32.to_le_bytes(), &mut []),
            (IO_ERROR, 0)
        );
        assert_eq!(
            ioctl(&mut adapter, QUERY_CURRENT_MODE, &mut [], &mut output),
            (IO_ERROR, 0)
        );
        assert_eq!((adapter.poisoned, adapter.mode), (1, None));
    }

    #[test]
    fn actual_packet_boundary_rejects_nulls_wrap_and_short_output() {
        let mut adapter = fixture();
        let extension = (&mut *adapter as *mut Adapter).cast();
        assert_eq!(unsafe { start_io(extension, null_mut()) }, 0);
        let mut status = StatusBlock {
            value: StatusValue { status: 0 },
            information: 42,
        };
        let mut packet = RequestPacket {
            control_code: QUERY_MODES,
            status: null_mut(),
            input: null_mut(),
            input_length: 0,
            output: null_mut(),
            output_length: 80,
        };
        assert_eq!(unsafe { start_io(extension, &mut packet) }, 0);
        packet.status = &mut status;
        assert_eq!(unsafe { start_io(extension, &mut packet) }, 1);
        assert_eq!(
            (unsafe { status.value.status }, status.information),
            (INVALID, 0)
        );
        packet.input = core::ptr::without_provenance_mut(usize::MAX - 1);
        packet.input_length = 4;
        assert_eq!(unsafe { start_io(extension, &mut packet) }, 1);
        assert_eq!(
            (unsafe { status.value.status }, status.information),
            (INVALID, 0)
        );
        for length in 0..MODE_BYTES {
            let mut output = [0xcc; MODE_BYTES];
            assert_eq!(
                ioctl(&mut adapter, QUERY_MODES, &mut [], &mut output[..length]),
                (122, 0)
            );
            assert_eq!(output, [0xcc; MODE_BYTES]);
        }
    }

    #[test]
    fn maps_are_bounded_reference_counted_and_unmap_failures_retain_ownership() {
        let mut adapter = fixture();
        set_mode(&mut adapter);
        let mut output = [0xcc; size_of::<MemoryInformation>()];
        assert_eq!(
            ioctl(
                &mut adapter,
                MAP_MEMORY,
                &mut 1u32.to_le_bytes(),
                &mut output
            ),
            (INVALID, 0)
        );
        for expected in [1, 2] {
            assert_eq!(
                ioctl(
                    &mut adapter,
                    MAP_MEMORY,
                    &mut 0u32.to_le_bytes(),
                    &mut output
                ),
                (0, size_of::<MemoryInformation>())
            );
            assert_eq!(adapter.references, expected);
        }
        let memory =
            unsafe { core::ptr::read_unaligned(output.as_ptr().cast::<MemoryInformation>()) };
        assert_eq!(
            (
                memory.video_base.addr(),
                memory.framebuffer_base.addr(),
                memory.video_length,
                memory.framebuffer_length
            ),
            (0x9000, 0x9000, FRAMEBUFFER_BYTES, 1_228_800)
        );
        assert_eq!(
            ioctl(
                &mut adapter,
                UNMAP_MEMORY,
                &mut 0x9000u32.to_le_bytes(),
                &mut []
            ),
            (0, 0)
        );
        assert_eq!(adapter.references, 1);
        port::with(|mock| mock.unmap_status = 5);
        assert_eq!(
            ioctl(
                &mut adapter,
                UNMAP_MEMORY,
                &mut 0x9000u32.to_le_bytes(),
                &mut []
            ),
            (5, 0)
        );
        assert_eq!((adapter.references, adapter.mapped.addr()), (1, 0x9000));
        finish_unmap(&mut adapter);
        port::with(|mock| assert_eq!(mock.calls.iter().filter(|call| **call == "map").count(), 1));
    }

    #[test]
    fn invalid_mapping_metadata_preserves_a_failed_rollback_for_retry() {
        let (mut adapter, output) = invalid_mapping_fixture();
        assert_eq!(output, [0xcc; size_of::<MemoryInformation>()]);
        assert_eq!(
            (adapter.poisoned, adapter.references, adapter.mapped.addr()),
            (1, 1, 0x9000)
        );
        finish_unmap(&mut adapter);
    }

    #[test]
    fn cleanup_keeps_accepted_assignments_with_videoport() {
        let mut adapter = mapped_fixture();
        port::with(|mock| {
            mock.calls.clear();
            mock.unmap_status = 7;
            mock.release_status = 9;
        });
        let extension = (&mut *adapter as *mut Adapter).cast();
        assert_eq!(unsafe { cleanup(extension, &mut adapter) }, Err(7));
        assert_eq!(
            (
                adapter.ready,
                adapter.tentative_claim,
                adapter.port_resources,
                adapter.references
            ),
            (0, 0, 1, 1)
        );
        assert!(adapter.registers.is_null());
        assert!(!adapter.mapped.is_null());
        port::with(|mock| {
            assert_eq!(mock.calls, ["unmap", "free"]);
            mock.calls.clear();
            mock.unmap_status = 0;
        });
        assert_eq!(unsafe { cleanup(extension, &mut adapter) }, Ok(()));
        assert_eq!(
            (
                adapter.cleanup_pending,
                adapter.port_resources,
                adapter.references
            ),
            (0, 1, 0)
        );
        assert!(adapter.mapped.is_null());
        port::with(|mock| assert_eq!(mock.calls, ["unmap"]));
    }

    #[test]
    fn shutdown_retries_failed_owners_through_the_real_power_callback() {
        let mut adapter = mapped_fixture();
        let extension = (&mut *adapter as *mut Adapter).cast();
        let mut power = PowerManagement {
            length: size_of::<PowerManagement>() as u32,
            dpms_version: 0,
            power_state: POWER_SHUTDOWN,
        };
        port::with(|mock| {
            mock.calls.clear();
            mock.unmap_status = 7;
            mock.release_status = 9;
        });
        assert_eq!(unsafe { set_power(extension, ADAPTER_ID, &mut power) }, 0);
        assert_eq!(
            (
                adapter.power_error,
                adapter.ready,
                adapter.cleanup_pending,
                adapter.port_resources,
                adapter.references
            ),
            (7, 0, 1, 1, 1)
        );
        assert_eq!(unsafe { get_power(extension, ADAPTER_ID, &mut power) }, 0);
        port::with(|mock| {
            assert_eq!(mock.calls, ["unmap", "free", "log"]);
            mock.calls.clear();
            mock.unmap_status = 9;
        });
        assert_eq!(unsafe { set_power(extension, ADAPTER_ID, &mut power) }, 0);
        assert_eq!(
            (
                adapter.power_error,
                adapter.cleanup_pending,
                adapter.references
            ),
            (9, 1, 1)
        );
        port::with(|mock| {
            assert_eq!(mock.calls, ["unmap", "log"]);
            mock.calls.clear();
            mock.unmap_status = 0;
        });
        assert_eq!(unsafe { set_power(extension, ADAPTER_ID, &mut power) }, 0);
        assert_eq!(
            (
                adapter.power_error,
                adapter.cleanup_pending,
                adapter.port_resources,
                adapter.references
            ),
            (0, 0, 1, 0)
        );
        assert!(adapter.mapped.is_null());
        port::with(|mock| {
            assert_eq!(mock.calls, ["unmap"]);
            assert_eq!(mock.logged_errors, [(7, 0x4656_4250), (9, 0x4656_4250)]);
        });
    }

    #[test]
    fn tentative_claim_release_is_retried_only_from_find_adapter() {
        port::fresh();
        port::with(|mock| {
            mock.header[0] = 0;
            mock.release_status = 9;
        });
        let mut adapter: Adapter = unsafe { core::mem::zeroed() };
        assert_eq!(discover(&mut adapter), INVALID);
        assert_eq!(
            (
                adapter.tentative_claim,
                adapter.port_resources,
                adapter.ready
            ),
            (1, 0, 0)
        );
        port::with(|mock| mock.calls.clear());
        assert_eq!(discover(&mut adapter), 9);
        port::with(|mock| {
            assert_eq!(mock.calls, ["release"]);
            mock.calls.clear();
            mock.release_status = 0;
            mock.header[0] = 0x34;
        });
        assert_eq!(discover(&mut adapter), 0);
        assert_eq!(
            (
                adapter.tentative_claim,
                adapter.port_resources,
                adapter.ready
            ),
            (0, 1, 1)
        );
        port::with(|mock| assert_eq!(mock.calls, ["release", "ranges", "pci", "base"]));
    }

    #[test]
    fn child_and_power_callbacks_do_not_fabricate_edid_or_ignore_transitions() {
        let mut adapter = fixture();
        set_mode(&mut adapter);
        let extension = (&mut *adapter as *mut Adapter).cast();
        let mut info = ChildEnumeration {
            size: size_of::<ChildEnumeration>() as u32,
            descriptor_size: 128,
            child_index: 1,
            acpi_hardware_id: 0,
            child_extension: null_mut(),
        };
        let (mut kind, mut id, mut unused) = (0, 0, u32::MAX);
        let mut edid = [0xcc; 128];
        assert_eq!(
            unsafe {
                child(
                    extension,
                    &mut info,
                    &mut kind,
                    edid.as_mut_ptr(),
                    &mut id,
                    &mut unused,
                )
            },
            1246
        );
        assert_eq!((kind, id, unused), (1, MONITOR_ID, 0));
        assert_eq!(edid, [0xcc; 128]);
        info.child_index = 2;
        assert_eq!(
            unsafe {
                child(
                    extension,
                    &mut info,
                    &mut kind,
                    edid.as_mut_ptr(),
                    &mut id,
                    &mut unused,
                )
            },
            1248
        );
        let mut power = PowerManagement {
            length: size_of::<PowerManagement>() as u32,
            dpms_version: 0,
            power_state: 4,
        };
        for unsupported in [0, 2, 3, 5, 7] {
            power.power_state = unsupported;
            assert_eq!(
                unsafe { get_power(extension, ADAPTER_ID, &mut power) },
                REINITIALIZE
            );
        }
        power.power_state = POWER_OFF;
        assert_eq!(unsafe { get_power(extension, ADAPTER_ID, &mut power) }, 0);
        assert_eq!(unsafe { set_power(extension, ADAPTER_ID, &mut power) }, 0);
        port::with(|mock| {
            assert_eq!(mock.registers[0x508 / 2], 0);
            assert_eq!(mock.clock & 0x20, 0x20);
            assert_eq!(mock.sequencer_index, 3);
        });
        power.power_state = POWER_ON;
        assert_eq!(unsafe { set_power(extension, ADAPTER_ID, &mut power) }, 0);
        port::with(|mock| {
            assert_eq!(mock.registers[0x508 / 2], 0xc1);
            assert_eq!(mock.clock, 1);
        });
        assert_eq!(unsafe { reset_hardware(extension, 80, 25) }, 0);
        power.power_state = POWER_SHUTDOWN;
        assert_eq!(unsafe { set_power(extension, ADAPTER_ID, &mut power) }, 0);
        assert_eq!((adapter.ready, adapter.tentative_claim), (0, 0));
        assert!(adapter.registers.is_null());
    }

    #[test]
    fn claim_and_reference_boundaries_do_not_leak_or_wrap() {
        port::fresh();
        port::with(|mock| {
            mock.ranges[2].start = mock.ranges[0].start;
            mock.ranges[2].length = FRAMEBUFFER_BYTES;
            assert_eq!(resources(&mock.header, &mock.ranges), Err(INVALID));
            mock.ranges[2].start = 0;
            mock.header[24..28].copy_from_slice(&0xc100_1000u32.to_le_bytes());
            mock.ranges[1].start = 0xc100_1000;
            assert_eq!(resources(&mock.header, &mock.ranges), Err(INVALID));
        });
        let mut adapter = fixture();
        set_mode(&mut adapter);
        let mut output = [0xcc; size_of::<MemoryInformation>()];
        assert_eq!(
            ioctl(
                &mut adapter,
                MAP_MEMORY,
                &mut 0u32.to_le_bytes(),
                &mut output
            )
            .0,
            0
        );
        port::with(|mock| mock.calls.clear());
        adapter.references = u32::MAX;
        assert_eq!(
            ioctl(
                &mut adapter,
                MAP_MEMORY,
                &mut 0u32.to_le_bytes(),
                &mut output
            ),
            (NO_MEMORY, 0)
        );
        assert_eq!(
            (adapter.references, adapter.mapped.addr()),
            (u32::MAX, 0x9000)
        );
        port::with(|mock| assert!(!mock.calls.contains(&"map")));
    }

    #[test]
    fn reset_restores_the_prior_mode_and_bugcheck_uses_only_register_access() {
        let mut adapter = fixture();
        adapter.initial[1] = 800;
        adapter.initial[2] = 600;
        adapter.initial[3] = 32;
        adapter.initial[4] = 0x41;
        adapter.initial[6] = 800;
        set_mode(&mut adapter);
        assert_eq!(ioctl(&mut adapter, RESET_DEVICE, &mut [], &mut []), (0, 0));
        assert_eq!(adapter.mode, None);
        port::with(|mock| {
            assert_eq!(
                (
                    mock.registers[0x502 / 2],
                    mock.registers[0x504 / 2],
                    mock.registers[0x508 / 2]
                ),
                (800, 600, 0xc1)
            );
            mock.calls.clear();
            mock.clock |= 0x20;
        });
        assert_eq!(
            unsafe { reset_hardware((&mut *adapter as *mut Adapter).cast(), 80, 25) },
            0
        );
        port::with(|mock| {
            assert!(mock.calls.is_empty());
            assert_eq!(mock.registers[0x508 / 2], 0);
            assert_eq!(mock.clock & 0x20, 0);
        });
    }

    #[test]
    fn mode_table_queries_have_exact_count_length_and_output_bounds() {
        let mut adapter = fixture();
        let mut count = [0xcc; 12];
        assert_eq!(
            ioctl(&mut adapter, QUERY_MODE_COUNT, &mut [], &mut count),
            (0, 8)
        );
        assert_eq!(&count[..8], &[16, 0, 0, 0, 80, 0, 0, 0]);
        assert_eq!(&count[8..], &[0xcc; 4]);
        let mut output = [0xcc; MODE_BYTES + 16];
        assert_eq!(
            ioctl(
                &mut adapter,
                QUERY_MODES,
                &mut [],
                &mut output[..MODE_BYTES - 1]
            ),
            (122, 0)
        );
        assert_eq!(output, [0xcc; MODE_BYTES + 16]);
        assert_eq!(
            ioctl(&mut adapter, QUERY_MODES, &mut [], &mut output),
            (0, MODE_BYTES)
        );
        assert_eq!(&output[MODE_BYTES..], &[0xcc; 16]);
        for (record, expected) in output[..MODE_BYTES]
            .as_chunks::<80>()
            .0
            .iter()
            .zip(MODES.iter())
        {
            assert_eq!(
                unsafe { core::ptr::read_unaligned(record.as_ptr().cast::<ModeInformation>()) },
                *expected
            );
        }
    }

    #[test]
    fn every_mode_programs_and_commits_only_after_complete_readback() {
        let mut adapter = fixture();
        for information in MODES {
            for flags in [0, LINEAR_MODE, NO_ZERO_MEMORY, LINEAR_MODE | NO_ZERO_MEMORY] {
                port::with(|mock| mock.writes.clear());
                set_index(&mut adapter, information.index, flags);
                assert_eq!(current_mode(&mut adapter), information);
                port::with(|mock| {
                    let enable = if flags & NO_ZERO_MEMORY == 0 {
                        0x41
                    } else {
                        0xc1
                    };
                    assert_eq!(
                        mock.writes,
                        [
                            (4, 0),
                            (1, information.width as u16),
                            (2, information.height as u16),
                            (3, 32),
                            (5, 0),
                            (4, enable),
                            (6, information.width as u16),
                            (8, 0),
                            (9, 0)
                        ]
                    );
                    for (index, expected) in [
                        (1, information.width as u16),
                        (2, information.height as u16),
                        (3, 32),
                        (4, enable),
                        (5, 0),
                        (6, information.width as u16),
                        (8, 0),
                        (9, 0),
                    ] {
                        assert_eq!(mock.registers[0x500 / 2 + index], expected);
                    }
                });
            }
        }
    }

    #[test]
    fn invalid_mode_requests_leave_committed_state_maps_and_registers_untouched() {
        let mut adapter = mapped_fixture();
        let before = (
            adapter.mode,
            adapter.mapped,
            adapter.mapped_length,
            adapter.references,
            adapter.poisoned,
        );
        port::with(|mock| {
            mock.calls.clear();
            mock.writes.clear();
        });
        for index in [16, 17, 255, 256, 0x2000_0000, u32::MAX] {
            for flags in [0, LINEAR_MODE, NO_ZERO_MEMORY, LINEAR_MODE | NO_ZERO_MEMORY] {
                assert_eq!(
                    ioctl(
                        &mut adapter,
                        SET_MODE,
                        &mut (index | flags).to_le_bytes(),
                        &mut []
                    ),
                    (INVALID, 0)
                );
                assert_eq!(
                    (
                        adapter.mode,
                        adapter.mapped,
                        adapter.mapped_length,
                        adapter.references,
                        adapter.poisoned
                    ),
                    before
                );
            }
        }
        port::with(|mock| {
            assert!(mock.calls.is_empty());
            assert!(mock.writes.is_empty());
        });
    }

    #[test]
    fn geometry_enable_and_stride_failures_invalidate_the_mode_without_losing_maps() {
        for register in [1, 2, 3, 4, 5, 6, 8, 9] {
            let mut adapter = mapped_fixture();
            port::with(|mock| {
                mock.ignored_write = (register != 4).then_some(register);
                mock.ignored_value = (register == 4).then_some((4, 0x41));
                if register != 4 {
                    mock.registers[0x500 / 2 + register] = u16::MAX;
                }
            });
            assert_eq!(
                ioctl(&mut adapter, SET_MODE, &mut 12u32.to_le_bytes(), &mut []),
                (IO_ERROR, 0)
            );
            assert_eq!(
                (
                    adapter.mode,
                    adapter.poisoned,
                    adapter.mapped.addr(),
                    adapter.mapped_length,
                    adapter.references
                ),
                (None, 1, 0x9000, FRAMEBUFFER_BYTES, 1)
            );
            rejected_current(&mut adapter, IO_ERROR);
            finish_unmap(&mut adapter);
        }
    }

    #[test]
    fn small_large_small_modes_reuse_full_aperture_and_retain_failed_unmaps() {
        let mut adapter = fixture();
        for (reference, index) in [0, 12, 0].into_iter().enumerate() {
            set_index(&mut adapter, index, NO_ZERO_MEMORY);
            let information = map_framebuffer(&mut adapter);
            assert_eq!(
                (
                    information.video_base.addr(),
                    information.framebuffer_base.addr(),
                    information.video_length,
                    information.framebuffer_length
                ),
                (
                    0x9000,
                    0x9000,
                    FRAMEBUFFER_BYTES,
                    ModeIndex::new(index).unwrap().visible_bytes()
                )
            );
            assert_eq!(
                (adapter.references, adapter.mapped_length),
                (reference as u32 + 1, FRAMEBUFFER_BYTES)
            );
        }
        port::with(|mock| {
            assert_eq!(mock.calls.iter().filter(|call| **call == "map").count(), 1);
            mock.unmap_status = 9;
        });
        for _ in 0..2 {
            assert_eq!(
                ioctl(
                    &mut adapter,
                    UNMAP_MEMORY,
                    &mut 0x9000u32.to_le_bytes(),
                    &mut []
                ),
                (0, 0)
            );
        }
        assert_eq!(
            ioctl(
                &mut adapter,
                UNMAP_MEMORY,
                &mut 0x9000u32.to_le_bytes(),
                &mut []
            ),
            (9, 0)
        );
        assert_eq!(
            (
                adapter.references,
                adapter.mapped.addr(),
                adapter.mapped_length
            ),
            (1, 0x9000, FRAMEBUFFER_BYTES)
        );
        finish_unmap(&mut adapter);
        assert_eq!(adapter.mapped_length, 0);
    }

    #[test]
    fn full_aperture_short_or_oversized_maps_are_never_reported_as_valid() {
        for length in [0, FRAMEBUFFER_BYTES - 1, FRAMEBUFFER_BYTES + 4096] {
            let mut adapter = fixture();
            set_index(&mut adapter, 12, 0);
            port::with(|mock| {
                mock.map_length = length;
                mock.unmap_status = 13;
            });
            let _ = rejected_map(&mut adapter);
            assert_eq!(
                (
                    adapter.mapping_valid,
                    adapter.mapped_length,
                    adapter.references
                ),
                (0, length, 1)
            );
            finish_unmap(&mut adapter);
        }
    }

    #[test]
    fn nonzero_mode_survives_off_on_but_reset_and_shutdown_clear_the_claim() {
        let mut adapter = fixture();
        set_index(&mut adapter, 12, 0);
        let _ = map_framebuffer(&mut adapter);
        let extension = (&mut *adapter as *mut Adapter).cast();
        let mut power = PowerManagement {
            length: size_of::<PowerManagement>() as u32,
            dpms_version: 0,
            power_state: POWER_OFF,
        };
        assert_eq!(unsafe { set_power(extension, ADAPTER_ID, &mut power) }, 0);
        assert_eq!(adapter.mode, ModeIndex::new(12));
        let mut output = [0xcc; 80];
        assert_eq!(
            ioctl(&mut adapter, QUERY_CURRENT_MODE, &mut [], &mut output),
            (IO_ERROR, 0)
        );
        power.power_state = POWER_ON;
        assert_eq!(unsafe { set_power(extension, ADAPTER_ID, &mut power) }, 0);
        assert_eq!(current_mode(&mut adapter), MODES[12]);
        assert_eq!(
            (adapter.references, adapter.mapped_length),
            (1, FRAMEBUFFER_BYTES)
        );
        port::with(|mock| assert_eq!(mock.registers[0x508 / 2], 0xc1));
        assert_eq!(ioctl(&mut adapter, RESET_DEVICE, &mut [], &mut []), (0, 0));
        assert_eq!(adapter.mode, None);
        set_index(&mut adapter, 15, 0);
        power.power_state = POWER_SHUTDOWN;
        assert_eq!(unsafe { set_power(extension, ADAPTER_ID, &mut power) }, 0);
        assert_eq!(
            (
                adapter.mode,
                adapter.ready,
                adapter.port_resources,
                adapter.mapped_length
            ),
            (None, 0, 1, 0)
        );
    }
}
