// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Solid and realised bitmap brushes from display/brush.c.

use crate::display_resources::Error;

#[derive(Clone, Copy, Debug)]
pub struct Pattern<'a> {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: &'a [u8],
    origin: [i32; 2],
}

impl<'a> Pattern<'a> {
    pub fn new(width: u32, height: u32, pixels: &'a [u8], origin: [i32; 2]) -> Option<Self> {
        let length = width.checked_mul(4)?.checked_mul(height)? as usize;
        (width != 0 && height != 0 && width <= 4096 && height <= 4096 && length == pixels.len())
            .then_some(Self {
                width,
                height,
                pixels,
                origin,
            })
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Paint<'a> {
    Solid(u32),
    Pattern(Pattern<'a>),
}

impl Paint<'_> {
    pub(crate) fn storage_bytes(self) -> usize {
        match self {
            Self::Solid(_) => 0,
            Self::Pattern(pattern) => 48 + pattern.pixels.len(),
        }
    }

    pub(crate) fn encode(
        self,
        address: u64,
        image: &mut [u8],
        brush: &mut [u8; 20],
    ) -> Result<(), Error> {
        brush.fill(0);
        match self {
            Self::Solid(color) => {
                brush[..4].copy_from_slice(&1u32.to_le_bytes());
                brush[4..8].copy_from_slice(&color.to_le_bytes());
            }
            Self::Pattern(pattern) => {
                crate::image::encode(
                    pattern.width,
                    pattern.height,
                    pattern.pixels,
                    false,
                    address,
                    image,
                )?;
                brush[..4].copy_from_slice(&2u32.to_le_bytes());
                brush[4..12].copy_from_slice(&address.to_le_bytes());
                brush[12..16].copy_from_slice(&pattern.origin[0].to_le_bytes());
                brush[16..20].copy_from_slice(&pattern.origin[1].to_le_bytes());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_and_solid_brushes_keep_wire_padding_and_pixel_bytes_defined() {
        let pixels = [1, 2, 3, 0, 4, 5, 6, 0];
        let pattern = Pattern::new(2, 1, &pixels, [-3, 5]).unwrap();
        let paint = Paint::Pattern(pattern);
        let mut image = [0xa5; 56];
        let mut brush = [0xa5; 20];
        paint
            .encode(0x0104000000001000, &mut image, &mut brush)
            .unwrap();
        assert_eq!(paint.storage_bytes(), 56);
        assert_eq!(&image[18..20], &[8, 5]);
        assert_eq!(&image[48..], &pixels);
        assert_eq!(
            u64::from_le_bytes(image[40..48].try_into().unwrap()),
            0x0104000000001030
        );
        assert_eq!(&brush[12..16], &(-3i32).to_le_bytes());
        Paint::Solid(0x123456)
            .encode(0, &mut [], &mut brush)
            .unwrap();
        assert_eq!(&brush[..8], &[1, 0, 0, 0, 0x56, 0x34, 0x12, 0]);
        assert_eq!(&brush[8..], &[0; 12]);
        assert!(Pattern::new(2, 1, &pixels[..7], [0, 0]).is_none());
    }
}
