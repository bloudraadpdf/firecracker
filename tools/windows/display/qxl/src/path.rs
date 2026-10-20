// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Cosmetic path resources and stroke commands from display/driver.c and res.c.

use crate::cosmetic::LineStyle;
use crate::display_resources::{Command, Error, Pool, Region};
use crate::ring::CommandRing;
use crate::rop::Rop2;
use crate::slots::MemorySlot;

#[derive(Clone, Copy, Debug)]
pub struct Segments<'a>(&'a [u8]);

impl<'a> Segments<'a> {
    pub fn new(data: &'a [u8]) -> Option<Self> {
        if data.len() > 1 << 20 {
            return None;
        }
        let mut offset = 0usize;
        let mut points = 0u32;
        let mut open = false;
        while offset < data.len() {
            let header = data.get(offset..offset.checked_add(8)?)?;
            let flags = u32::from_le_bytes(header[..4].try_into().ok()?);
            let count = u32::from_le_bytes(header[4..8].try_into().ok()?);
            points = points.checked_add(count)?;
            if flags & !15 != 0
                || count == 0
                || points > 65536
                || flags & 8 != 0 && flags & 2 == 0
                || flags & 4 != 0 && flags & 1 == 0
            {
                return None;
            }
            if flags & 1 != 0 {
                if open {
                    return None;
                }
                open = true;
            }
            if !open {
                return None;
            }
            offset = offset.checked_add(8 + count as usize * 8)?;
            if flags & 2 != 0 {
                open = false;
            }
        }
        (offset == data.len() && !open).then_some(Self(data))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Stroke<'a> {
    bounds: [i32; 4],
    paint: crate::paint::Paint<'a>,
    foreground: Rop2,
    background: Rop2,
}

impl<'a> Stroke<'a> {
    pub fn new(bounds: [i32; 4], paint: crate::paint::Paint<'a>, mix: u32) -> Option<Self> {
        let [left, top, right, bottom] = bounds;
        if left < 0 || top < 0 || left >= right || top >= bottom {
            return None;
        }
        Some(Self {
            bounds,
            paint,
            foreground: Rop2::new(mix as u8)?,
            background: Rop2::new((mix >> 8) as u8)?,
        })
    }

    pub fn bounds(self) -> [i32; 4] {
        self.bounds
    }

    fn encode(
        self,
        path: u64,
        style_address: u64,
        style: &LineStyle,
        brush: &[u8; 20],
        clock: u32,
        bytes: &mut [u8],
    ) -> Result<(), Error> {
        let bytes = bytes.get_mut(..175).ok_or(Error::InvalidSize)?;
        crate::draw::header(bytes, self.bounds, clock, 0, 10);
        bytes[123..131].copy_from_slice(&path.to_le_bytes());
        bytes[131] = style.flags();
        bytes[134] = style.lengths().len() as u8;
        bytes[135..139].copy_from_slice(&16i32.to_le_bytes());
        bytes[143..151].copy_from_slice(&style_address.to_le_bytes());
        bytes[151..171].copy_from_slice(brush);
        bytes[171..173].copy_from_slice(&self.foreground.descriptor().to_le_bytes());
        bytes[173..175].copy_from_slice(&self.background.descriptor().to_le_bytes());
        Ok(())
    }

    /// # Safety
    /// Retain the active primary, exclusive ring and pool generation through
    /// completion. Exclude CPU rendering while this command can access pixels.
    pub unsafe fn submit(
        self,
        path: Segments<'_>,
        style: &LineStyle,
        pool: &mut Pool<'_>,
        slot: MemorySlot,
        ring: &mut CommandRing,
        clock: u32,
    ) -> Result<bool, Error> {
        let style_offset = 24 + path.0.len();
        let paint_offset = style_offset + style.lengths().len() * 4;
        let length = paint_offset + self.paint.storage_bytes();
        let memory = pool.resource(Region::Main, length)?;
        let physical = match slot.encode(memory.as_ptr().addr() as u64, length as u64) {
            Ok(address) => address,
            Err(error) => {
                pool.release(memory).map_err(|(error, _)| error)?;
                return Err(Error::Address(error));
            }
        };
        // SAFETY: this unpublished resource owns the full initialised output.
        let bytes = unsafe { core::slice::from_raw_parts_mut(memory.as_ptr(), length) };
        bytes[..24].fill(0);
        bytes[..4].copy_from_slice(&(path.0.len() as u32).to_le_bytes());
        bytes[4..8].copy_from_slice(&(path.0.len() as u32).to_le_bytes());
        bytes[24..style_offset].copy_from_slice(path.0);
        for (bytes, value) in bytes[style_offset..paint_offset]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(style.lengths())
        {
            *bytes = value.to_le_bytes();
        }
        let result = (|| {
            let mut brush = [0; 20];
            self.paint.encode(
                physical + paint_offset as u64,
                &mut bytes[paint_offset..],
                &mut brush,
            )?;
            let mut output = pool.output(Command::Draw, 175)?;
            let style_address = if style.lengths().is_empty() {
                0
            } else {
                physical + style_offset as u64
            };
            if let Err(error) = pool
                .payload(&mut output)
                .and_then(|bytes| self.encode(physical, style_address, style, &brush, clock, bytes))
            {
                pool.cancel(output).map_err(|(error, _)| error)?;
                return Err(error);
            }
            // SAFETY: the drawable retains the complete path and style allocation.
            unsafe { pool.submit_retained(output, ring, &[&memory]) }
        })();
        pool.release(memory).map_err(|(error, _)| error)?;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cosmetic::CosmeticStyle;

    #[test]
    fn segments_require_bounded_complete_figures() {
        let mut data = [0; 24];
        data[..4].copy_from_slice(&15u32.to_le_bytes());
        data[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert!(Segments::new(&data).is_some());
        assert!(Segments::new(&data[..23]).is_none());
        data[0] = 1;
        assert!(Segments::new(&data).is_none());
        data[0] = 18;
        assert!(Segments::new(&data).is_none());
        data[0] = 2;
        assert!(Segments::new(&data).is_none());
    }

    #[test]
    fn stroke_preserves_style_density_and_rop_descriptors() {
        let style = LineStyle::new(
            CosmeticStyle::Custom {
                lengths: &[6, 2],
                start_gap: true,
            },
            3,
        )
        .unwrap();
        let stroke =
            Stroke::new([3, 5, 17, 19], crate::paint::Paint::Solid(0x123456), 0x0d0d).unwrap();
        let mut bytes = [0xa5; 180];
        let mut brush = [0; 20];
        stroke.paint.encode(0, &mut [], &mut brush).unwrap();
        stroke
            .encode(0x1000, 0x2000, &style, &brush, 7, &mut bytes)
            .unwrap();
        assert_eq!(&bytes[131..139], &[12, 0, 0, 2, 16, 0, 0, 0]);
        assert_eq!(&bytes[151..159], &[1, 0, 0, 0, 0x56, 0x34, 0x12, 0]);
        assert_eq!(&bytes[171..175], &[8, 0, 8, 0]);
        assert!(bytes[175..].iter().all(|byte| *byte == 0xa5));
    }
}
