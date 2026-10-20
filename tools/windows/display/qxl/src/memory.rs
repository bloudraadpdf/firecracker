// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! QXL miniport ROM and shared RAM layout validation.

use crate::slots::SlotLayout;

pub const ROM_PREFIX_BYTES: usize = 69;
pub const RAM_HEADER_BYTES: u32 = 5348;
pub const REVISION_ASYNC: u8 = 3;
pub const REVISION_MONITORS: u8 = 4;
pub const COMMAND_RING_OFFSET: u32 = 4108;
pub const CURSOR_RING_OFFSET: u32 = 4640;
pub const RELEASE_RING_OFFSET: u32 = 5172;
pub const UPDATE_AREA_OFFSET: u32 = 5256;
pub const UPDATE_SURFACE_OFFSET: u32 = 5272;
pub const SLOT_START_OFFSET: u32 = 5276;
pub const SLOT_END_OFFSET: u32 = 5284;
pub const PRIMARY_CREATE_OFFSET: u32 = 5292;
pub const MONITORS_CONFIG_OFFSET: u32 = 5340;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    InvalidRom,
    InvalidRam,
    InvalidSlots,
}

#[derive(Clone, Copy, Debug)]
pub struct RomInfo {
    pub(crate) device_id: u32,
    pub(crate) modes_offset: u32,
    pub(crate) page_count: u32,
    pub(crate) surface_bytes: u32,
    pub(crate) ram_header: u32,
    pub(crate) surface_count: u32,
    pub(crate) first_slot: u8,
    pub(crate) slot_end: u8,
    pub(crate) slot_id_bits: u8,
    pub(crate) slot_generation_bits: u8,
}

impl RomInfo {
    pub fn device_id(self) -> u32 {
        self.device_id
    }

    /// Parse a snapshot of the immutable ROM prefix, before dereferencing RAM.
    pub fn parse(rom: &[u8], ram_bytes: u32) -> Result<Self, LayoutError> {
        if rom.len() < ROM_PREFIX_BYTES || &rom[..4] != b"QXRO" {
            return Err(LayoutError::InvalidRom);
        }
        let field = |offset| {
            u32::from_le_bytes([
                rom[offset],
                rom[offset + 1],
                rom[offset + 2],
                rom[offset + 3],
            ])
        };
        let info = Self {
            device_id: field(4),
            modes_offset: field(24),
            page_count: field(28),
            surface_bytes: field(40),
            ram_header: field(44),
            surface_count: field(52),
            first_slot: rom[64],
            slot_end: rom[65],
            slot_generation_bits: rom[66],
            slot_id_bits: rom[67],
        };
        if info.modes_offset < ROM_PREFIX_BYTES as u32 || info.surface_count == 0 {
            return Err(LayoutError::InvalidRom);
        }
        let pages_end = info
            .page_count
            .checked_mul(4096)
            .and_then(|bytes| info.surface_bytes.checked_add(bytes));
        if info.surface_bytes == 0
            || info.page_count == 0
            || field(32) != info.surface_bytes
            || pages_end.is_none_or(|end| end > info.ram_header)
            || info
                .ram_header
                .checked_add(RAM_HEADER_BYTES)
                .is_none_or(|end| end > ram_bytes)
            || info.ram_header & 3 != 0
        {
            return Err(LayoutError::InvalidRam);
        }
        SlotLayout::new(info.slot_id_bits, info.slot_generation_bits)
            .map_err(|_| LayoutError::InvalidSlots)?;
        if info.first_slot >= info.slot_end
            || u16::from(info.slot_end) > (1u16 << info.slot_id_bits)
        {
            return Err(LayoutError::InvalidSlots);
        }
        Ok(info)
    }

    pub fn mode_table_offset(self) -> u32 {
        self.modes_offset
    }

    pub fn primary_surface_bytes(self) -> u32 {
        self.surface_bytes
    }

    pub fn header_offset(self) -> u32 {
        self.ram_header
    }

    pub fn main_slot_bytes(self) -> u32 {
        // parse checked both operations and excluded the shared RAM header.
        self.surface_bytes + self.page_count * 4096
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAM_BYTES: u32 = 32 << 20;

    fn rom() -> [u8; ROM_PREFIX_BYTES] {
        let mut rom = [0; ROM_PREFIX_BYTES];
        rom[..4].copy_from_slice(b"QXRO");
        for (offset, value) in [
            (24, 4096u32),
            (28, ((16 << 20) - 8192) / 4096),
            (32, 16 << 20),
            (40, 16 << 20),
            (44, RAM_BYTES - 8192),
            (52, 1),
        ] {
            rom[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        rom[64..68].copy_from_slice(&[1, 3, 8, 8]);
        rom
    }

    #[test]
    fn accepts_firecracker_layout_and_excludes_ring_header_from_slot() {
        let info = RomInfo::parse(&rom(), RAM_BYTES).unwrap();
        assert_eq!(info.mode_table_offset(), 4096);
        assert_eq!(info.device_id(), 0);
        assert_eq!(info.primary_surface_bytes(), 16 << 20);
        assert_eq!(info.header_offset(), RAM_BYTES - 8192);
        assert_eq!(info.main_slot_bytes(), RAM_BYTES - 8192);
        assert_eq!(COMMAND_RING_OFFSET + 532, CURSOR_RING_OFFSET);
        assert_eq!(CURSOR_RING_OFFSET + 532, RELEASE_RING_OFFSET);
    }

    #[test]
    fn rejects_overflow_overlapping_pages_and_short_ram() {
        for (offset, value) in [
            (28, u32::MAX),
            (32, 1),
            (40, u32::MAX),
            (44, u32::MAX - 3),
            (44, (16 << 20) + 4),
        ] {
            let mut rom = rom();
            rom[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(matches!(
                RomInfo::parse(&rom, RAM_BYTES),
                Err(LayoutError::InvalidRam)
            ));
        }
        assert!(RomInfo::parse(&rom(), RAM_BYTES - 8192).is_err());
    }

    #[test]
    fn rejects_bad_magic_and_unrepresentable_slot_layouts() {
        assert!(RomInfo::parse(&rom()[..68], RAM_BYTES).is_err());
        let mut bad = rom();
        bad[0] = 0;
        assert!(matches!(
            RomInfo::parse(&bad, RAM_BYTES),
            Err(LayoutError::InvalidRom)
        ));
        for (offset, value) in [(64, 3), (65, 1), (66, 0), (67, 0), (67, 9)] {
            let mut bad = rom();
            bad[offset] = value;
            assert!(matches!(
                RomInfo::parse(&bad, RAM_BYTES),
                Err(LayoutError::InvalidSlots)
            ));
        }
    }
}
