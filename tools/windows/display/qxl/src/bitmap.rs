// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Validated, tightly packed source bitmaps from display/res.c.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Format {
    Indexed1 = 2,
    Indexed4 = 4,
    Indexed8 = 5,
    Rgb555 = 6,
    Bgr24 = 7,
    Bgrx32 = 8,
    Bgra32 = 9,
}

impl Format {
    pub fn bits(self) -> u32 {
        match self {
            Self::Indexed1 => 1,
            Self::Indexed4 => 4,
            Self::Indexed8 => 8,
            Self::Rgb555 => 16,
            Self::Bgr24 => 24,
            Self::Bgrx32 | Self::Bgra32 => 32,
        }
    }

    pub fn indexed(self) -> bool {
        matches!(self, Self::Indexed1 | Self::Indexed4 | Self::Indexed8)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Bitmap<'a> {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: Format,
    pub(crate) stride: u32,
    pub(crate) pixels: &'a [u8],
}

impl<'a> Bitmap<'a> {
    pub fn new(width: u32, height: u32, format: Format, pixels: &'a [u8]) -> Option<Self> {
        let stride = width.checked_mul(format.bits())?.div_ceil(8);
        let bytes = stride.checked_mul(height)? as usize;
        (width != 0 && height != 0 && width <= 4096 && height <= 4096 && pixels.len() == bytes)
            .then_some(Self {
                width,
                height,
                format,
                stride,
                pixels,
            })
    }

    pub(crate) fn validate_palette(self, count: usize) -> bool {
        if !self.format.indexed() {
            return count == 0;
        }
        if count == 0 || count > (1 << self.format.bits()) {
            return false;
        }
        self.pixels
            .chunks_exact(self.stride as usize)
            .all(|row| (0..self.width as usize).all(|x| usize::from(self.index(row, x)) < count))
    }

    fn index(self, row: &[u8], x: usize) -> u8 {
        match self.format {
            Format::Indexed1 => (row[x / 8] >> (7 - x % 8)) & 1,
            Format::Indexed4 => (row[x / 2] >> ((1 - x % 2) * 4)) & 15,
            _ => row[x],
        }
    }
}

pub fn pack_indexed(
    format: Format,
    source: &[u8],
    skip: usize,
    width: usize,
    output: &mut [u8],
) -> Option<()> {
    if !format.indexed() || width == 0 {
        return None;
    }
    let bits = format.bits() as usize;
    if skip.checked_add(width)?.checked_mul(bits)?.div_ceil(8) > source.len()
        || output.len() != width.checked_mul(bits)?.div_ceil(8)
    {
        return None;
    }
    output.fill(0);
    for x in 0..width {
        let source_bit = (skip + x) * bits;
        let value =
            (source[source_bit / 8] >> (8 - bits - source_bit % 8)) & (((1u16 << bits) - 1) as u8);
        let target = x * bits;
        output[target / 8] |= value << (8 - bits - target % 8);
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_crops_keep_high_bit_first_order_and_clear_tail_bits() {
        let mut mono = [0xff];
        pack_indexed(Format::Indexed1, &[0xb5, 0xc0], 3, 7, &mut mono).unwrap();
        assert_eq!(mono, [0xae]);
        let mut nibble = [0xff; 2];
        pack_indexed(Format::Indexed4, &[0x12, 0x34, 0x56], 1, 3, &mut nibble).unwrap();
        assert_eq!(nibble, [0x23, 0x40]);
        let mut bytes = [0; 2];
        pack_indexed(Format::Indexed8, &[1, 2, 3], 1, 2, &mut bytes).unwrap();
        assert_eq!(bytes, [2, 3]);
        assert!(pack_indexed(Format::Indexed4, &[0x12], 1, 2, &mut nibble).is_none());
        assert!(pack_indexed(Format::Bgrx32, &[0; 4], 0, 1, &mut mono).is_none());
        assert!(
            Bitmap::new(3, 1, Format::Indexed4, &[0x12, 0x3f])
                .unwrap()
                .validate_palette(4)
        );
        assert!(
            !Bitmap::new(3, 1, Format::Indexed4, &[0x12, 0x4f])
                .unwrap()
                .validate_palette(4)
        );
    }
}
