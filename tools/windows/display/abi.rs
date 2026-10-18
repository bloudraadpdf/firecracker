// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! XP x86 video ABI and request validation; no hardware implementation or driver entry.

use core::ffi::c_void;
use core::mem::{offset_of, size_of, size_of_val};

#[repr(C)]
pub union StatusValue {
    pub status: u32,
    pub pointer: *mut c_void,
}

#[repr(C)]
pub struct StatusBlock {
    pub value: StatusValue,
    pub information: usize,
}

#[repr(C)]
pub struct RequestPacket {
    pub control_code: u32,
    pub status: *mut StatusBlock,
    pub input: *mut c_void,
    pub input_length: u32,
    pub output: *mut c_void,
    pub output_length: u32,
}

#[repr(C)]
pub struct AccessRange {
    pub start: i64,
    pub length: u32,
    pub in_io_space: u8,
    pub visible: u8,
    pub shareable: u8,
    pub passive: u8,
}

#[repr(C)]
pub struct PowerManagement {
    pub length: u32,
    pub dpms_version: u32,
    pub power_state: u32,
}

#[repr(C)]
pub struct ChildEnumeration {
    pub size: u32,
    pub descriptor_size: u32,
    pub child_index: u32,
    pub acpi_hardware_id: u32,
    pub child_extension: *mut c_void,
}

#[repr(C)]
pub struct PortConfiguration {
    pub length: u32,
    pub bus_number: u32,
    pub interface_type: i32,
    pub interrupt_level: u32,
    pub interrupt_vector: u32,
    pub interrupt_mode: u32,
    pub emulator_count: u32,
    pub emulator_entries: *mut c_void,
    pub emulator_context: usize,
    pub vdm_address: i64,
    pub vdm_length: u32,
    pub hardware_state_size: u32,
    pub dma_channel: u32,
    pub dma_port: u32,
    pub dma_shareable: u8,
    pub interrupt_shareable: u8,
    pub master: u8,
    pub dma_width: u32,
    pub dma_speed: u32,
    pub map_buffers: u8,
    pub need_physical_addresses: u8,
    pub demand_mode: u8,
    pub maximum_transfer_length: u32,
    pub physical_breaks: u32,
    pub scatter_gather: u8,
    pub scatter_gather_chunk: u32,
    pub get_proc_address: *mut c_void,
    pub registry_path: *mut u16,
    pub system_memory_size: u64,
}

pub type FindAdapter =
    unsafe extern "system" fn(*mut c_void, *mut c_void, *mut u16, *mut c_void, *mut u8) -> u32;
pub type Initialize = unsafe extern "system" fn(*mut c_void) -> u8;
pub type StartIo = unsafe extern "system" fn(*mut c_void, *mut RequestPacket) -> u8;
pub type ResetHardware = unsafe extern "system" fn(*mut c_void, u32, u32) -> u8;
pub type Timer = unsafe extern "system" fn(*mut c_void);
pub type StartDma = unsafe extern "system" fn(*mut c_void, *mut c_void) -> u32;
pub type Power = unsafe extern "system" fn(*mut c_void, u32, *mut PowerManagement) -> u32;
pub type ChildDescriptor = unsafe extern "system" fn(
    *mut c_void,
    *mut ChildEnumeration,
    *mut u32,
    *mut u8,
    *mut u32,
    *mut u32,
) -> u32;
pub type QueryInterface = unsafe extern "system" fn(*mut c_void, *mut c_void) -> u32;
pub type LegacyResources = unsafe extern "system" fn(u32, u32, *mut *mut AccessRange, *mut u32);

#[repr(C)]
pub struct InitializationData {
    pub size: u32,
    pub interface_type: i32,
    pub find_adapter: Option<FindAdapter>,
    pub initialize: Option<Initialize>,
    pub interrupt: Option<Initialize>,
    pub start_io: Option<StartIo>,
    pub extension_size: u32,
    pub starting_device_number: u32,
    pub reset_hardware: Option<ResetHardware>,
    pub timer: Option<Timer>,
    pub start_dma: Option<StartDma>,
    pub set_power: Option<Power>,
    pub get_power: Option<Power>,
    pub child_descriptor: Option<ChildDescriptor>,
    pub query_interface: Option<QueryInterface>,
    pub child_extension_size: u32,
    pub legacy_resource_list: *mut AccessRange,
    pub legacy_resource_count: u32,
    pub get_legacy_resources: Option<LegacyResources>,
    pub allow_early_enumeration: u8,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeInformation {
    pub length: u32,
    pub index: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub planes: u32,
    pub bits_per_plane: u32,
    pub frequency: u32,
    pub x_millimeter: u32,
    pub y_millimeter: u32,
    pub red_bits: u32,
    pub green_bits: u32,
    pub blue_bits: u32,
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
    pub attributes: u32,
    pub bitmap_width: u32,
    pub bitmap_height: u32,
    pub driver_attributes: u32,
}

#[repr(C)]
pub struct NumberOfModes {
    pub count: u32,
    pub information_length: u32,
}

#[repr(C)]
pub struct VideoMemory {
    pub requested_address: *mut c_void,
}

#[repr(C)]
pub struct MemoryInformation {
    pub video_base: *mut c_void,
    pub video_length: u32,
    pub framebuffer_base: *mut c_void,
    pub framebuffer_length: u32,
}

pub const QUERY_MODES: u32 = 0x0023_0400;
pub const QUERY_MODE_COUNT: u32 = 0x0023_0404;
pub const QUERY_CURRENT_MODE: u32 = 0x0023_0408;
pub const SET_MODE: u32 = 0x0023_040c;
pub const RESET_DEVICE: u32 = 0x0023_0410;
pub const MAP_MEMORY: u32 = 0x0023_0458;
pub const UNMAP_MEMORY: u32 = 0x0023_045c;
pub const NO_ZERO_MEMORY: u32 = 0x8000_0000;
pub const LINEAR_MODE: u32 = 0x4000_0000;

pub const FRAMEBUFFER_BYTES: u32 = 16 * 1024 * 1024;

pub static MODES: [ModeInformation; 16] = {
    let dimensions = [
        (640, 480),
        (800, 600),
        (1024, 768),
        (1280, 1024),
        (1600, 1200),
        (1152, 864),
        (1280, 768),
        (1280, 800),
        (1280, 960),
        (1440, 900),
        (1400, 1050),
        (1680, 1050),
        (1920, 1200),
        (1280, 720),
        (1920, 1080),
        (1600, 900),
    ];
    let mut modes = [ModeInformation {
        length: size_of::<ModeInformation>() as u32,
        index: 0,
        width: 0,
        height: 0,
        stride: 0,
        planes: 1,
        bits_per_plane: 32,
        frequency: 60,
        x_millimeter: 0,
        y_millimeter: 0,
        red_bits: 8,
        green_bits: 8,
        blue_bits: 8,
        red_mask: 0x00ff_0000,
        green_mask: 0x0000_ff00,
        blue_mask: 0x0000_00ff,
        attributes: 0x0123,
        bitmap_width: 0,
        bitmap_height: 0,
        driver_attributes: 0,
    }; 16];
    let mut index = 0;
    while index < dimensions.len() {
        let (width, height) = dimensions[index];
        assert!(width > 0 && width <= 2048 && height > 0 && height <= 2048);
        assert!(width * height * 4 <= FRAMEBUFFER_BYTES);
        modes[index].index = index as u32;
        modes[index].width = width;
        modes[index].height = height;
        modes[index].stride = width * 4;
        modes[index].bitmap_width = width;
        modes[index].bitmap_height = height;
        index += 1;
    }
    modes
};

pub const MODE_BYTES: usize = size_of_val(&MODES);

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeIndex(&'static ModeInformation);

impl ModeIndex {
    pub fn new(index: u32) -> Option<Self> {
        MODES.get(index as usize).map(Self)
    }

    pub fn information(self) -> &'static ModeInformation {
        self.0
    }

    pub fn visible_bytes(self) -> u32 {
        let mode = self.information();
        mode.stride * mode.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    QueryModes,
    QueryModeCount,
    QueryCurrentMode,
    SetMode { mode: ModeIndex, clear_memory: bool },
    Reset,
    Map { requested_address: u32 },
    Unmap { address: u32 },
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    Unsupported = 1,
    InvalidParameter = 87,
    InsufficientBuffer = 122,
}

pub fn decode_request(
    code: u32,
    input: &[u8],
    output_length: usize,
) -> Result<Operation, RequestError> {
    let required_output = match code {
        QUERY_MODES => MODE_BYTES,
        QUERY_CURRENT_MODE => size_of::<ModeInformation>(),
        QUERY_MODE_COUNT => size_of::<NumberOfModes>(),
        MAP_MEMORY => 16,
        SET_MODE | RESET_DEVICE | UNMAP_MEMORY => 0,
        _ => return Err(RequestError::Unsupported),
    };
    if output_length < required_output {
        return Err(RequestError::InsufficientBuffer);
    }
    let input_word = || {
        input
            .get(..4)
            .and_then(|word| word.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or(RequestError::InsufficientBuffer)
    };
    Ok(match code {
        QUERY_MODES => Operation::QueryModes,
        QUERY_MODE_COUNT => Operation::QueryModeCount,
        QUERY_CURRENT_MODE => Operation::QueryCurrentMode,
        SET_MODE => {
            let requested = input_word()?;
            let mode = ModeIndex::new(requested & !(NO_ZERO_MEMORY | LINEAR_MODE))
                .ok_or(RequestError::InvalidParameter)?;
            Operation::SetMode {
                mode,
                clear_memory: requested & NO_ZERO_MEMORY == 0,
            }
        }
        RESET_DEVICE => Operation::Reset,
        MAP_MEMORY => Operation::Map {
            requested_address: input_word()?,
        },
        UNMAP_MEMORY => Operation::Unmap {
            address: input_word()?,
        },
        _ => return Err(RequestError::Unsupported),
    })
}

const _: () = {
    assert!(size_of::<ModeInformation>() == 80);
    assert!(size_of::<Option<ModeIndex>>() == size_of::<&ModeInformation>());
    assert!(offset_of!(ModeInformation, attributes) == 64);
    assert!(size_of::<NumberOfModes>() == 8);
    assert!(size_of::<PowerManagement>() == 12);
};

#[cfg(all(target_arch = "x86", target_os = "windows"))]
const _: () = {
    assert!(size_of::<PortConfiguration>() == 112);
    assert!(offset_of!(PortConfiguration, vdm_address) == 40);
    assert!(offset_of!(PortConfiguration, hardware_state_size) == 52);
    assert!(offset_of!(PortConfiguration, system_memory_size) == 104);
    assert!(size_of::<StatusBlock>() == 8);
    assert!(size_of::<RequestPacket>() == 24);
    assert!(offset_of!(RequestPacket, output_length) == 20);
    assert!(size_of::<AccessRange>() == 16);
    assert!(size_of::<ChildEnumeration>() == 20);
    assert!(size_of::<InitializationData>() == 84);
    assert!(offset_of!(InitializationData, set_power) == 44);
    assert!(offset_of!(InitializationData, child_descriptor) == 52);
    assert!(offset_of!(InitializationData, allow_early_enumeration) == 76);
    assert!(offset_of!(InitializationData, reserved) == 80);
    assert!(size_of::<VideoMemory>() == 4);
    assert!(size_of::<MemoryInformation>() == 16);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_ioctl_codes_use_video_buffered_any_access() {
        for (actual, function) in [
            (QUERY_MODES, 0x100),
            (QUERY_MODE_COUNT, 0x101),
            (QUERY_CURRENT_MODE, 0x102),
            (SET_MODE, 0x103),
            (RESET_DEVICE, 0x104),
            (MAP_MEMORY, 0x116),
            (UNMAP_MEMORY, 0x117),
        ] {
            assert_eq!(actual, (0x23 << 16) | (function << 2));
        }
    }

    #[test]
    fn query_buffers_require_the_declared_structure() {
        for (code, required, operation) in [
            (QUERY_MODES, MODE_BYTES, Operation::QueryModes),
            (QUERY_CURRENT_MODE, 80, Operation::QueryCurrentMode),
            (QUERY_MODE_COUNT, 8, Operation::QueryModeCount),
        ] {
            for length in 0..required {
                assert_eq!(
                    decode_request(code, &[], length),
                    Err(RequestError::InsufficientBuffer)
                );
            }
            assert_eq!(decode_request(code, &[], required), Ok(operation));
            assert_eq!(decode_request(code, &[], usize::MAX), Ok(operation));
        }
    }

    #[test]
    fn mode_zero_accepts_only_the_two_documented_flags() {
        for flags in [0, LINEAR_MODE, NO_ZERO_MEMORY, LINEAR_MODE | NO_ZERO_MEMORY] {
            assert_eq!(
                decode_request(SET_MODE, &flags.to_le_bytes(), 0),
                Ok(Operation::SetMode {
                    mode: ModeIndex::new(0).unwrap(),
                    clear_memory: flags & NO_ZERO_MEMORY == 0
                })
            );
        }
        for mode in [16, u32::MAX, LINEAR_MODE | 16, 0x2000_0000] {
            assert_eq!(
                decode_request(SET_MODE, &mode.to_le_bytes(), 0),
                Err(RequestError::InvalidParameter)
            );
        }
    }

    #[test]
    fn short_input_never_reads_a_partial_pointer_or_mode() {
        for code in [SET_MODE, MAP_MEMORY, UNMAP_MEMORY] {
            for length in 0..4 {
                assert_eq!(
                    decode_request(code, &[0; 4][..length], 16),
                    Err(RequestError::InsufficientBuffer)
                );
            }
        }
    }

    #[test]
    fn mapping_decodes_x86_addresses_without_mapping_any_memory() {
        for address in [0_u32, 0x8000_0000, u32::MAX] {
            for length in 0..16 {
                assert_eq!(
                    decode_request(MAP_MEMORY, &address.to_le_bytes(), length),
                    Err(RequestError::InsufficientBuffer)
                );
            }
            assert_eq!(
                decode_request(MAP_MEMORY, &address.to_le_bytes(), 16),
                Ok(Operation::Map {
                    requested_address: address
                })
            );
            assert_eq!(
                decode_request(UNMAP_MEMORY, &address.to_le_bytes(), 0),
                Ok(Operation::Unmap { address })
            );
        }
    }

    #[test]
    fn reset_is_only_decoded_and_unknown_requests_fail() {
        assert_eq!(decode_request(RESET_DEVICE, &[], 0), Ok(Operation::Reset));
        for code in [0, QUERY_MODES - 1, UNMAP_MEMORY + 4, u32::MAX] {
            assert_eq!(
                decode_request(code, &[0xff; 4], usize::MAX),
                Err(RequestError::Unsupported)
            );
        }
    }

    #[test]
    fn prototype_mode_is_linear_xrgb_with_no_offscreen_claim() {
        let mode = MODES[0];
        assert_eq!(
            (mode.width, mode.height, mode.bits_per_plane),
            (640, 480, 32)
        );
        assert_eq!(mode.stride.checked_mul(mode.height), Some(1_228_800));
        assert_eq!(mode.width.checked_mul(4), Some(mode.stride));
        assert_eq!(mode.planes, 1);
        assert_eq!(mode.attributes, 0x100 | 0x20 | 0x2 | 0x1);
        assert_eq!(
            (mode.red_mask | mode.green_mask | mode.blue_mask).count_ones(),
            24
        );
        assert_eq!(mode.red_mask & mode.green_mask, 0);
        assert_eq!(mode.green_mask & mode.blue_mask, 0);
        assert_eq!((mode.x_millimeter, mode.y_millimeter), (0, 0));
    }

    #[test]
    fn coupled_table_matches_all_approved_geometries_and_extents() {
        let widths = [
            640, 800, 1024, 1280, 1600, 1152, 1280, 1280, 1280, 1440, 1400, 1680, 1920, 1280, 1920,
            1600,
        ];
        let heights = [
            480, 600, 768, 1024, 1200, 864, 768, 800, 960, 900, 1050, 1050, 1200, 720, 1080, 900,
        ];
        assert_eq!(MODE_BYTES, 1280);
        for (index, information) in MODES.iter().enumerate() {
            let mode = ModeIndex::new(index as u32).unwrap();
            assert_eq!(mode.information(), information);
            assert_eq!(
                (information.index, information.width, information.height),
                (index as u32, widths[index], heights[index])
            );
            assert_eq!(
                (
                    information.length,
                    information.stride,
                    information.bitmap_width,
                    information.bitmap_height
                ),
                (80, widths[index] * 4, widths[index], heights[index])
            );
            assert_eq!(
                (
                    information.planes,
                    information.bits_per_plane,
                    information.frequency,
                    information.attributes
                ),
                (1, 32, 60, 0x123)
            );
            assert_eq!(
                (
                    information.red_bits,
                    information.green_bits,
                    information.blue_bits
                ),
                (8, 8, 8)
            );
            assert_eq!(
                (
                    information.red_mask,
                    information.green_mask,
                    information.blue_mask
                ),
                (0xff0000, 0xff00, 0xff)
            );
            assert_eq!(
                (
                    information.x_millimeter,
                    information.y_millimeter,
                    information.driver_attributes
                ),
                (0, 0, 0)
            );
            assert_eq!(mode.visible_bytes(), widths[index] * heights[index] * 4);
            assert!(mode.visible_bytes() <= FRAMEBUFFER_BYTES);
        }
        for index in [16, 17, 255, 256, u32::MAX] {
            assert_eq!(ModeIndex::new(index), None);
        }
        let empty: Option<ModeIndex> = unsafe { core::mem::zeroed() };
        assert_eq!(empty, None);
    }

    #[test]
    fn every_bounded_index_accepts_only_the_documented_flags() {
        for index in 0..16 {
            for flags in [0, LINEAR_MODE, NO_ZERO_MEMORY, LINEAR_MODE | NO_ZERO_MEMORY] {
                assert_eq!(
                    decode_request(SET_MODE, &(index | flags).to_le_bytes(), 0),
                    Ok(Operation::SetMode {
                        mode: ModeIndex::new(index).unwrap(),
                        clear_memory: flags & NO_ZERO_MEMORY == 0
                    })
                );
            }
        }
        for flags in [0, LINEAR_MODE, NO_ZERO_MEMORY, LINEAR_MODE | NO_ZERO_MEMORY] {
            assert_eq!(
                decode_request(SET_MODE, &(16 | flags).to_le_bytes(), 0),
                Err(RequestError::InvalidParameter)
            );
        }
    }
}
