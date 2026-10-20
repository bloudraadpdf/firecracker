// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! XDDM mode enumeration and the Windows VIDEO_MODE_INFORMATION ABI.

use core::mem::{offset_of, size_of};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeError {
    InvalidTable,
    InvalidDimensions,
    UnsupportedDepth,
    InvalidStride,
    InvalidId,
    InsufficientMemory,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoModeInformation {
    pub length: u32,
    pub mode_index: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub planes: u32,
    pub bits_per_plane: u32,
    pub frequency: u32,
    pub width_mm: u32,
    pub height_mm: u32,
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

const _: () = {
    assert!(size_of::<VideoModeInformation>() == 80);
    assert!(offset_of!(VideoModeInformation, mode_index) == 4);
    assert!(offset_of!(VideoModeInformation, stride) == 16);
    assert!(offset_of!(VideoModeInformation, attributes) == 64);
    assert!(offset_of!(VideoModeInformation, driver_attributes) == 76);
};

impl VideoModeInformation {
    pub fn encode(&self, output: &mut [u8]) -> Option<usize> {
        let output = output.get_mut(..80)?;
        let fields = [
            self.length,
            self.mode_index,
            self.width,
            self.height,
            self.stride,
            self.planes,
            self.bits_per_plane,
            self.frequency,
            self.width_mm,
            self.height_mm,
            self.red_bits,
            self.green_bits,
            self.blue_bits,
            self.red_mask,
            self.green_mask,
            self.blue_mask,
            self.attributes,
            self.bitmap_width,
            self.bitmap_height,
            self.driver_attributes,
        ];
        for (bytes, value) in output.as_chunks_mut::<4>().0.iter_mut().zip(fields) {
            *bytes = value.to_le_bytes();
        }
        Some(80)
    }

    /// Port of FillVidModeInfo for custom resolutions; 16-bit modes use RGB555.
    pub fn custom(index: u32, width: u32, height: u32, depth: u32) -> Result<Self, ModeError> {
        let bytes_per_pixel = match depth {
            16 => 2,
            24 => 3,
            32 => 4,
            _ => return Err(ModeError::UnsupportedDepth),
        };
        let stride = width
            .checked_mul(bytes_per_pixel)
            .and_then(|bytes| bytes.checked_add(3))
            .map(|bytes| bytes & !3)
            .ok_or(ModeError::InvalidStride)?;
        let mut result = Self::from_fields([index, width, height, depth, stride, 320, 240, 0])?;
        result.frequency = 60;
        Ok(result)
    }

    fn from_fields(fields: [u32; 8]) -> Result<Self, ModeError> {
        let [
            index,
            width,
            height,
            depth,
            stride,
            width_mm,
            height_mm,
            orientation,
        ] = fields;
        if width == 0 || height == 0 {
            return Err(ModeError::InvalidDimensions);
        }
        let (bytes_per_pixel, color_bits) = match depth {
            16 => (2, 5),
            24 => (3, 8),
            32 => (4, 8),
            _ => return Err(ModeError::UnsupportedDepth),
        };
        if width
            .checked_mul(bytes_per_pixel)
            .is_none_or(|minimum| stride < minimum)
            || stride.checked_mul(height).is_none()
        {
            return Err(ModeError::InvalidStride);
        }
        let mask = (1 << color_bits) - 1;
        Ok(Self {
            length: 80,
            mode_index: index,
            width,
            height,
            stride,
            planes: 1,
            bits_per_plane: depth,
            frequency: 100,
            width_mm,
            height_mm,
            red_bits: color_bits,
            green_bits: color_bits,
            blue_bits: color_bits,
            red_mask: mask << (color_bits * 2),
            green_mask: mask << color_bits,
            blue_mask: mask,
            attributes: 3,
            bitmap_width: width,
            bitmap_height: height,
            driver_attributes: orientation,
        })
    }

    pub fn framebuffer_bytes(&self) -> Option<u32> {
        self.stride.checked_mul(self.height)
    }
}

/// A bounded view of the packed, little-endian QXLModes ROM table.
#[derive(Clone, Copy, Debug)]
pub struct ModeTable<'a> {
    records: &'a [u8],
}

/// Miniport mode state, including the two alternating custom resolution IDs.
#[derive(Debug)]
pub struct ModeSet<'a> {
    table: ModeTable<'a>,
    custom: [VideoModeInformation; 2],
    selected_custom: usize,
    current: u32,
    surface_bytes: u32,
}

impl<'a> ModeSet<'a> {
    pub fn new(table: ModeTable<'a>, surface_bytes: u32) -> Result<Self, ModeError> {
        let mut modes = table.modes();
        let first = modes.next().ok_or(ModeError::InvalidTable)??;
        let mut last = first.mode_index;
        for (index, entry) in core::iter::once(Ok(first)).chain(modes).enumerate() {
            let mode = entry?;
            if mode
                .framebuffer_bytes()
                .is_none_or(|bytes| bytes > surface_bytes)
            {
                return Err(ModeError::InsufficientMemory);
            }
            if index != 0 && mode.mode_index <= last {
                return Err(ModeError::InvalidId);
            }
            last = mode.mode_index;
        }
        let mut custom = [first; 2];
        custom[0].mode_index = last.checked_add(1).ok_or(ModeError::InvalidId)?;
        custom[1].mode_index = last.checked_add(2).ok_or(ModeError::InvalidId)?;
        Ok(Self {
            table,
            custom,
            selected_custom: 0,
            current: first.mode_index,
            surface_bytes,
        })
    }

    pub fn count(&self) -> usize {
        self.table.records.len() / 32 + 2
    }

    pub fn entries(&self) -> impl Iterator<Item = Result<VideoModeInformation, ModeError>> + '_ {
        self.table.modes().chain(self.custom.into_iter().map(Ok))
    }

    pub fn find(&self, id: u32) -> Result<VideoModeInformation, ModeError> {
        for mode in self.entries() {
            let mode = mode?;
            if mode.mode_index == id {
                return Ok(mode);
            }
        }
        Err(ModeError::InvalidId)
    }

    pub fn current(&self) -> Result<VideoModeInformation, ModeError> {
        self.find(self.current)
    }

    pub fn set_current(&mut self, id: u32) -> Result<(), ModeError> {
        self.find(id)?;
        self.current = id;
        Ok(())
    }

    pub fn set_custom(&mut self, width: u32, height: u32, depth: u32) -> Result<u32, ModeError> {
        let next = self.selected_custom ^ 1;
        let id = self.custom[next].mode_index;
        let mode = VideoModeInformation::custom(id, width, height, depth)?;
        if mode
            .framebuffer_bytes()
            .is_none_or(|bytes| bytes > self.surface_bytes)
        {
            return Err(ModeError::InsufficientMemory);
        }
        self.custom[next] = mode;
        self.selected_custom = next;
        Ok(id)
    }
}

impl<'a> ModeTable<'a> {
    pub fn parse(rom: &'a [u8], offset: u32) -> Result<Self, ModeError> {
        let offset = usize::try_from(offset).map_err(|_| ModeError::InvalidTable)?;
        let data = rom.get(offset..).ok_or(ModeError::InvalidTable)?;
        let count = data
            .get(..4)
            .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
            .map(u32::from_le_bytes)
            .filter(|&count| count != 0)
            .ok_or(ModeError::InvalidTable)?;
        let length = usize::try_from(count)
            .ok()
            .and_then(|count| count.checked_mul(32))
            .and_then(|length| length.checked_add(4))
            .ok_or(ModeError::InvalidTable)?;
        let records = data.get(4..length).ok_or(ModeError::InvalidTable)?;
        Ok(Self { records })
    }

    pub fn modes(
        &self,
    ) -> impl ExactSizeIterator<Item = Result<VideoModeInformation, ModeError>> + '_ {
        self.records.as_chunks::<32>().0.iter().map(|record| {
            let mut fields = [0; 8];
            for (field, bytes) in fields.iter_mut().zip(record.as_chunks::<4>().0) {
                *field = u32::from_le_bytes(*bytes);
            }
            VideoModeInformation::from_fields(fields)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_windows_mode_record_and_rejects_short_output() {
        let mode = VideoModeInformation::custom(0x11223344, 641, 480, 24).unwrap();
        let mut output = [0xa5; 81];
        assert_eq!(mode.encode(&mut output[..79]), None);
        assert!(output.iter().all(|&byte| byte == 0xa5));
        assert_eq!(mode.encode(&mut output), Some(80));
        assert_eq!(&output[..8], &[80, 0, 0, 0, 0x44, 0x33, 0x22, 0x11]);
        assert_eq!(&output[16..20], &1924u32.to_le_bytes());
        assert_eq!(&output[52..56], &0x00ff0000u32.to_le_bytes());
        assert_eq!(output[80], 0xa5);
    }

    #[test]
    fn custom_modes_match_xddm_layout_and_colour_masks() {
        let mode = VideoModeInformation::custom(17, 641, 480, 24).unwrap();
        assert_eq!(mode.stride, 1924);
        assert_eq!(mode.framebuffer_bytes(), Some(923520));
        assert_eq!(mode.frequency, 60);
        assert_eq!(mode.attributes, 3);
        assert_eq!(
            (mode.red_mask, mode.green_mask, mode.blue_mask),
            (0xff0000, 0xff00, 0xff)
        );
        let rgb555 = VideoModeInformation::custom(9, 640, 480, 16).unwrap();
        assert_eq!(
            (rgb555.red_mask, rgb555.green_mask, rgb555.blue_mask),
            (0x7c00, 0x3e0, 0x1f)
        );
    }

    #[test]
    fn invalid_dimensions_and_stride_cannot_wrap() {
        assert_eq!(
            VideoModeInformation::custom(0, 0, 480, 32),
            Err(ModeError::InvalidDimensions)
        );
        assert_eq!(
            VideoModeInformation::custom(0, 640, 0, 32),
            Err(ModeError::InvalidDimensions)
        );
        assert_eq!(
            VideoModeInformation::custom(0, 640, 480, 8),
            Err(ModeError::UnsupportedDepth)
        );
        assert_eq!(
            VideoModeInformation::custom(0, u32::MAX, 1, 32),
            Err(ModeError::InvalidStride)
        );
        assert_eq!(
            VideoModeInformation::custom(0, 640, u32::MAX, 32),
            Err(ModeError::InvalidStride)
        );
    }

    #[test]
    fn rom_records_preserve_nonconsecutive_ids_and_orientation() {
        let mut rom = [0; 69];
        rom[1..5].copy_from_slice(&2u32.to_le_bytes());
        for (index, fields) in [
            [4u32, 640, 480, 16, 1280, 300, 200, 0],
            [91, 768, 1024, 32, 3072, 240, 320, 1],
        ]
        .into_iter()
        .enumerate()
        {
            for (field, value) in fields.into_iter().enumerate() {
                let start = 5 + index * 32 + field * 4;
                rom[start..start + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        let table = ModeTable::parse(&rom, 1).unwrap();
        let mut modes = table.modes();
        assert_eq!(modes.len(), 2);
        assert_eq!(modes.next().unwrap().unwrap().mode_index, 4);
        let second = modes.next().unwrap().unwrap();
        assert_eq!(second.mode_index, 91);
        assert_eq!(second.driver_attributes, 1);
        assert_eq!(second.frequency, 100);
        assert!(modes.next().is_none());
        assert!(ModeTable::parse(&rom[..68], 1).is_err());
    }

    #[test]
    fn malformed_rom_tables_are_rejected() {
        for data in [&[][..], &[0; 4][..], &[255; 4][..]] {
            assert!(ModeTable::parse(data, 0).is_err());
        }
        assert!(ModeTable::parse(&[0; 8], u32::MAX).is_err());
        assert_eq!(
            VideoModeInformation::from_fields([0, 640, 480, 32, 1, 0, 0, 0]),
            Err(ModeError::InvalidStride)
        );
    }

    #[test]
    fn custom_mode_ids_alternate_and_failed_requests_preserve_state() {
        let fields = [1u32, 17, 1, 1, 32, 4, 320, 240, 0];
        let mut rom = [0; 36];
        for (bytes, value) in rom.as_chunks_mut::<4>().0.iter_mut().zip(fields) {
            *bytes = value.to_le_bytes();
        }
        let mut modes = ModeSet::new(ModeTable::parse(&rom, 0).unwrap(), 1923).unwrap();
        assert_eq!(modes.count(), 3);
        assert_eq!(modes.current().unwrap().mode_index, 17);
        assert_eq!(modes.set_current(0), Err(ModeError::InvalidId));
        assert_eq!(modes.current().unwrap().mode_index, 17);
        // A 641-pixel RGB24 scanline needs 1924 bytes after alignment.
        assert_eq!(
            modes.set_custom(641, 1, 24),
            Err(ModeError::InsufficientMemory)
        );
        assert_eq!(modes.set_custom(640, 1, 24), Ok(19));
        modes.set_current(19).unwrap();
        assert_eq!(modes.current().unwrap().width, 640);
        assert_eq!(
            modes.set_custom(0, 1, 32),
            Err(ModeError::InvalidDimensions)
        );
        assert_eq!(modes.set_custom(100, 1, 32), Ok(18));
        assert_eq!(modes.set_custom(200, 1, 32), Ok(19));
        assert_eq!(modes.current().unwrap().width, 200);
    }

    #[test]
    fn mode_set_rejects_duplicate_ids_and_custom_id_overflow() {
        let mut rom = [0; 68];
        let fields = [
            2u32, 17, 1, 1, 32, 4, 320, 240, 0, 17, 1, 1, 32, 4, 320, 240, 0,
        ];
        for (bytes, value) in rom.as_chunks_mut::<4>().0.iter_mut().zip(fields) {
            *bytes = value.to_le_bytes();
        }
        assert!(matches!(
            ModeSet::new(ModeTable::parse(&rom, 0).unwrap(), 4),
            Err(ModeError::InvalidId)
        ));
        rom[36..40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            ModeSet::new(ModeTable::parse(&rom, 0).unwrap(), 4),
            Err(ModeError::InvalidId)
        ));
        rom[36..40].copy_from_slice(&18u32.to_le_bytes());
        assert!(matches!(
            ModeSet::new(ModeTable::parse(&rom, 0).unwrap(), 3),
            Err(ModeError::InsufficientMemory)
        ));
    }
}
