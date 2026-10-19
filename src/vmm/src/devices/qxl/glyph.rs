// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::bitmap::chunks;
use super::memory::Memory;
use super::protocol::{DecodeError, bytes};
use super::transport::{AddressSpace, TransportError};

#[derive(Clone, Copy, Debug)]
enum Format {
    A1,
    A4,
    A8,
}

impl Format {
    fn bits(self) -> usize {
        match self {
            Self::A1 => 1,
            Self::A4 => 4,
            Self::A8 => 8,
        }
    }

    fn coverage(self, row: &[u8], x: usize) -> u8 {
        match self {
            Self::A1 => {
                if row[x / 8] & (0x80 >> (x % 8)) != 0 {
                    255
                } else {
                    0
                }
            }
            // QXL A4 levels occupy the high nibble of an A8 coverage value.
            Self::A4 => ((row[x / 2] >> (4 * (1 - x % 2))) & 15) << 4,
            Self::A8 => row[x],
        }
    }
}

#[derive(Debug)]
struct Glyph {
    x: i64,
    y: i64,
    width: usize,
    height: usize,
    stride: usize,
    offset: usize,
}

#[derive(Debug)]
pub(super) struct GlyphString {
    glyphs: Vec<Glyph>,
    data: Vec<u8>,
    format: Format,
    top_down: bool,
}

impl GlyphString {
    pub fn decode(
        memory: &Memory,
        addresses: &AddressSpace,
        address: u64,
    ) -> Result<Self, TransportError> {
        let mut header = [0; 8];
        addresses.read(memory, address, &mut header)?;
        let length = u32::from_le_bytes(bytes(&header, 0)?) as usize;
        let count = usize::from(u16::from_le_bytes(bytes(&header, 4)?));
        let flags = u16::from_le_bytes(bytes(&header, 6)?);
        if length > 32 << 20 || count > 4096 || flags & !15 != 0 {
            return Err(DecodeError::Parameter.into());
        }
        let format = match flags & 7 {
            1 => Format::A1,
            2 => Format::A4,
            4 => Format::A8,
            0 if count == 0 => Format::A1,
            _ => return Err(DecodeError::Parameter.into()),
        };
        if length < count * 20 {
            return Err(DecodeError::Truncated.into());
        }
        addresses.resolve(address, 28)?;
        let data = chunks(
            memory,
            addresses,
            address.checked_add(8).ok_or(DecodeError::Address)?,
            length,
        )?;
        let mut glyphs = Vec::with_capacity(count);
        let mut position = 0;
        for _ in 0..count {
            let header = data
                .get(position..position + 20)
                .ok_or(DecodeError::Truncated)?;
            let width = usize::from(u16::from_le_bytes(bytes(header, 16)?));
            let height = usize::from(u16::from_le_bytes(bytes(header, 18)?));
            if width > 4096 || height > 4096 {
                return Err(DecodeError::Parameter.into());
            }
            let stride = (width * format.bits()).div_ceil(8);
            let start = position + 20;
            let end = start + stride * height;
            if end > data.len() {
                return Err(DecodeError::Truncated.into());
            }
            let coordinate = |offset| i32::from_le_bytes(bytes(header, offset).unwrap());
            glyphs.push(Glyph {
                x: i64::from(coordinate(0)) + i64::from(coordinate(8)),
                y: i64::from(coordinate(4)) + i64::from(coordinate(12)),
                width,
                height,
                stride,
                offset: start,
            });
            position = end;
        }
        if position != data.len() {
            return Err(DecodeError::Parameter.into());
        }
        Ok(Self {
            glyphs,
            data,
            format,
            top_down: flags & 8 != 0,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs
            .iter()
            .all(|glyph| glyph.width == 0 || glyph.height == 0)
    }

    pub fn coverage(&self, bounds: [u32; 4]) -> Result<Vec<u8>, DecodeError> {
        let [left, top, right, bottom] = bounds;
        if left > right
            || top > bottom
            || right > 4096
            || bottom > 4096
            || u64::from(right - left) * u64::from(bottom - top) > 8 << 20
        {
            return Err(DecodeError::Parameter);
        }
        let width = (right - left) as usize;
        let mut output = vec![0; width * (bottom - top) as usize];
        for glyph in &self.glyphs {
            let x0 = glyph.x.max(i64::from(left));
            let y0 = glyph.y.max(i64::from(top));
            let x1 = (glyph.x + i64::try_from(glyph.width).unwrap()).min(i64::from(right));
            let y1 = (glyph.y + i64::try_from(glyph.height).unwrap()).min(i64::from(bottom));
            for y in y0..y1 {
                let row = usize::try_from(y - glyph.y).unwrap();
                let row = if self.top_down {
                    row
                } else {
                    glyph.height - 1 - row
                };
                let start = glyph.offset + row * glyph.stride;
                let source = &self.data[start..start + glyph.stride];
                for x in x0..x1 {
                    let index = usize::try_from(y - i64::from(top)).unwrap() * width
                        + usize::try_from(x - i64::from(left)).unwrap();
                    output[index] = output[index].max(
                        self.format
                            .coverage(source, usize::try_from(x - glyph.x).unwrap()),
                    );
                }
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::qxl::memory::Region;
    use crate::devices::qxl::test_utils::{glyph, memory};

    const POINTER: u64 = 0x0101_0000_0000_0100;

    fn decode(flags: u16, count: u16, data: &[u8]) -> Result<GlyphString, TransportError> {
        let (memory, addresses) = memory();
        let mut bytes = Vec::new();
        let length = u32::try_from(data.len()).unwrap();
        bytes.extend(length.to_le_bytes());
        bytes.extend(count.to_le_bytes());
        bytes.extend(flags.to_le_bytes());
        bytes.extend(length.to_le_bytes());
        bytes.extend([0; 16]);
        bytes.extend(data);
        memory.write(Region::Ram, 256, &bytes).unwrap();
        GlyphString::decode(&memory, &addresses, POINTER)
    }

    #[test]
    fn glyph_placement_and_row_order_follow_the_driver_layout() {
        for (flags, data) in [(1, [0x40, 0x80]), (9, [0x80, 0x40])] {
            let string = decode(flags, 1, &glyph([2, 2], [-1, -1], 2, 2, &data)).unwrap();
            assert_eq!(
                string.coverage([0, 0, 4, 4]).unwrap(),
                [0, 0, 0, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0, 0, 0, 0]
            );
            assert_eq!(string.coverage([2, 1, 3, 3]).unwrap(), [0, 255]);
        }
        let string = decode(
            9,
            1,
            &glyph([i32::MAX, i32::MAX], [i32::MAX, i32::MAX], 1, 1, &[0x80]),
        )
        .unwrap();
        assert_eq!(string.coverage([0, 0, 2, 2]).unwrap(), [0; 4]);
    }

    #[test]
    fn packed_alpha_levels_and_overlapping_glyphs_form_one_coverage_mask() {
        let string = decode(2, 1, &glyph([0, 0], [0, 0], 3, 1, &[0xf1, 0x2f])).unwrap();
        assert_eq!(string.coverage([0, 0, 3, 1]).unwrap(), [240, 16, 32]);
        let mut bytes = glyph([1, 1], [0, 0], 1, 1, &[64]);
        bytes.extend(glyph([1, 1], [0, 0], 1, 1, &[128]));
        let string = decode(4, 2, &bytes).unwrap();
        assert_eq!(string.coverage([0, 0, 2, 2]).unwrap(), [0, 0, 0, 128]);
    }

    #[test]
    fn glyph_headers_and_bits_can_cross_chunk_boundaries() {
        let (memory, addresses) = memory();
        let data = glyph([0, 0], [0, 0], 1, 1, &[0x80]);
        let mut first = Vec::new();
        first.extend(21u32.to_le_bytes());
        first.extend(1u16.to_le_bytes());
        first.extend(9u16.to_le_bytes());
        first.extend(5u32.to_le_bytes());
        first.extend(0u64.to_le_bytes());
        first.extend((POINTER + 256).to_le_bytes());
        first.extend(&data[..5]);
        memory.write(Region::Ram, 256, &first).unwrap();
        let mut second = Vec::new();
        second.extend(16u32.to_le_bytes());
        second.extend((POINTER + 8).to_le_bytes());
        second.extend(0u64.to_le_bytes());
        second.extend(&data[5..]);
        memory.write(Region::Ram, 512, &second).unwrap();
        assert_eq!(
            GlyphString::decode(&memory, &addresses, POINTER)
                .unwrap()
                .coverage([0, 0, 1, 1])
                .unwrap(),
            [255]
        );
    }

    #[test]
    fn empty_glyphs_do_not_require_foreground_paint_or_allocate_large_masks() {
        let empty = decode(0, 0, &[]).unwrap();
        assert!(empty.is_empty());
        let string = decode(1, 1, &glyph([0, 0], [0, 0], 2, 0, &[])).unwrap();
        assert!(string.is_empty());
        for bounds in [
            [2, 0, 1, 1],
            [0, 2, 1, 1],
            [0, 0, 4097, 1],
            [0, 0, 4096, 4096],
        ] {
            empty.coverage(bounds).unwrap_err();
        }
    }

    #[test]
    fn invalid_counts_flags_dimensions_and_truncated_glyphs_are_rejected() {
        let bytes = glyph([0, 0], [0, 0], 1, 1, &[0x80]);
        for flags in [0, 3, 5, 7, 16] {
            decode(flags, 1, &bytes).unwrap_err();
        }
        decode(1, 4097, &bytes).unwrap_err();
        decode(1, 1, &bytes[..19]).unwrap_err();
        decode(1, 1, &bytes[..20]).unwrap_err();
        decode(0, 0, &[0]).unwrap_err();
        decode(1, 1, &glyph([0, 0], [0, 0], 4097, 0, &[])).unwrap_err();
        let (memory, addresses) = memory();
        memory
            .write(Region::Ram, 256, &((32u32 << 20) + 1).to_le_bytes())
            .unwrap();
        GlyphString::decode(&memory, &addresses, POINTER).unwrap_err();
    }
}
