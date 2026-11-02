// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Raster glyph strings and text drawables from display/text.c and res.c.

use crate::display_resources::{Command, Error, Pool, Region};
use crate::ring::CommandRing;
use crate::rop::Rop2;
use crate::slots::MemorySlot;

#[derive(Clone, Copy, Debug)]
pub struct Glyphs<'a> {
    data: &'a [u8],
    count: u16,
    flags: u16,
}

impl<'a> Glyphs<'a> {
    pub fn new(data: &'a [u8], count: u16, bits: u8) -> Option<Self> {
        let flags = match bits {
            1 => 9,
            4 => 10,
            8 => 12,
            _ => return None,
        };
        if count > 4096 || data.len() > 32 << 20 {
            return None;
        }
        let mut offset = 0usize;
        for _ in 0..count {
            let header = data.get(offset..offset.checked_add(20)?)?;
            let width = usize::from(u16::from_le_bytes([header[16], header[17]]));
            let height = usize::from(u16::from_le_bytes([header[18], header[19]]));
            if width > 4096 || height > 4096 {
                return None;
            }
            offset = offset.checked_add(20 + (width * usize::from(bits)).div_ceil(8) * height)?;
        }
        (offset == data.len()).then_some(Self { data, count, flags })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Text<'a> {
    bounds: [i32; 4],
    foreground: crate::paint::Paint<'a>,
    background: Option<([i32; 4], crate::paint::Paint<'a>)>,
    operation: Rop2,
}

impl<'a> Text<'a> {
    pub fn new(
        bounds: [i32; 4],
        foreground: crate::paint::Paint<'a>,
        background: Option<([i32; 4], crate::paint::Paint<'a>)>,
        mix: u8,
    ) -> Option<Self> {
        let [left, top, right, bottom] = bounds;
        if left < 0 || top < 0 || left >= right || top >= bottom {
            return None;
        }
        if let Some(([left, top, right, bottom], _)) = background
            && (left > right || top > bottom)
        {
            return None;
        }
        Some(Self {
            bounds,
            foreground,
            background,
            operation: Rop2::new(mix)?,
        })
    }

    pub fn bounds(self) -> [i32; 4] {
        self.bounds
    }

    fn encode(
        self,
        string: u64,
        brushes: &[[u8; 20]; 2],
        clock: u32,
        bytes: &mut [u8],
    ) -> Result<(), Error> {
        let bytes = bytes.get_mut(..191).ok_or(Error::InvalidSize)?;
        crate::draw::header(
            bytes,
            self.bounds,
            clock,
            if self.background.is_some() { 1 } else { 0 },
            11,
        );
        bytes[123..131].copy_from_slice(&string.to_le_bytes());
        bytes[147..167].copy_from_slice(&brushes[0]);
        bytes[187..189].copy_from_slice(&self.operation.descriptor().to_le_bytes());
        if let Some((area, _)) = self.background {
            crate::draw::rect(bytes, 131, area);
            bytes[167..187].copy_from_slice(&brushes[1]);
            bytes[189..191].copy_from_slice(&8u16.to_le_bytes());
        }
        Ok(())
    }

    /// # Safety
    /// Retain the active destination, exclusive ring and pool generation through
    /// host release. Glyph positions and the background must describe this primary.
    pub unsafe fn submit(
        self,
        glyphs: Glyphs<'_>,
        pool: &mut Pool<'_>,
        slot: MemorySlot,
        ring: &mut CommandRing,
        clock: u32,
    ) -> Result<bool, Error> {
        let foreground_offset = 28 + glyphs.data.len();
        let background_offset = foreground_offset + self.foreground.storage_bytes();
        let length = background_offset
            + self
                .background
                .map_or(0, |(_, paint)| paint.storage_bytes());
        let memory = pool.resource(Region::Main, length)?;
        let address = match slot.encode(memory.as_ptr().addr() as u64, length as u64) {
            Ok(address) => address,
            Err(error) => {
                pool.release(memory).map_err(|(error, _)| error)?;
                return Err(Error::Address(error));
            }
        };
        // SAFETY: the resource exclusively owns its entire unpublished extent.
        let bytes = unsafe { core::slice::from_raw_parts_mut(memory.as_ptr(), length) };
        bytes[..28].fill(0);
        bytes[..4].copy_from_slice(&(glyphs.data.len() as u32).to_le_bytes());
        bytes[4..6].copy_from_slice(&glyphs.count.to_le_bytes());
        bytes[6..8].copy_from_slice(&glyphs.flags.to_le_bytes());
        bytes[8..12].copy_from_slice(&(glyphs.data.len() as u32).to_le_bytes());
        bytes[28..foreground_offset].copy_from_slice(glyphs.data);
        let result = (|| {
            let mut brushes = [[0; 20]; 2];
            self.foreground.encode(
                address + foreground_offset as u64,
                &mut bytes[foreground_offset..background_offset],
                &mut brushes[0],
            )?;
            if let Some((_, background)) = self.background {
                background.encode(
                    address + background_offset as u64,
                    &mut bytes[background_offset..],
                    &mut brushes[1],
                )?;
            }
            let mut output = pool.output(Command::Draw, 191)?;
            if let Err(error) = pool
                .payload(&mut output)
                .and_then(|bytes| self.encode(address, &brushes, clock, bytes))
            {
                pool.cancel(output).map_err(|(error, _)| error)?;
                return Err(error);
            }
            // SAFETY: the drawable retains its immutable glyph resource until release.
            unsafe { pool.submit_retained(output, ring, &[&memory]) }
        })();
        pool.release(memory).map_err(|(error, _)| error)?;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterned_text_retains_both_images_and_rolls_back_a_full_ring() {
        use crate::display_resources::test_utils::{Bytes, HostRing, empty, pool};
        use crate::paint::{Paint, Pattern};
        use crate::ring::ReleaseRing;
        use crate::slots::SlotLayout;
        let mut backing = [0u64; 2048];
        let base = backing.as_mut_ptr().cast::<u8>();
        let slot = SlotLayout::new(8, 8)
            .unwrap()
            .slot(
                1,
                4,
                0xe0000000,
                base.addr() as u64,
                core::mem::size_of_val(&backing) as u64,
            )
            .unwrap();
        let mut pool = pool(&mut backing, None);
        let mut command_bytes = Bytes([0; 532]);
        let host = HostRing::new(&mut command_bytes, 32, 16);
        let mut release_bytes = Bytes([0; 84]);
        let mut release_host = HostRing::new(&mut release_bytes, 8, 8);
        // SAFETY: the simulated host retains both independent ring mappings.
        let (mut commands, mut releases) = unsafe {
            (
                CommandRing::new(host.base, 532).unwrap(),
                ReleaseRing::new(release_host.base, 84).unwrap(),
            )
        };
        let pixels = [1, 2, 3, 0, 4, 5, 6, 0];
        let paint = Paint::Pattern(Pattern::new(2, 1, &pixels, [-3, 5]).unwrap());
        let text = Text::new([3, 5, 17, 19], paint, Some(([4, 6, 16, 18], paint)), 13).unwrap();
        let mut glyph = [0; 21];
        glyph[16] = 1;
        glyph[18] = 1;
        glyph[20] = 0x80;
        let glyphs = Glyphs::new(&glyph, 1, 1).unwrap();
        // SAFETY: the simulated destination and retained mappings are exclusive.
        unsafe { text.submit(glyphs, &mut pool, slot, &mut commands, 7) }.unwrap();
        let output = host.command(&pool, 0);
        // SAFETY: the published text command contains its complete 191-byte record.
        let command = unsafe { output.snapshot::<191>() };
        let string = u64::from_le_bytes(command[123..131].try_into().unwrap());
        assert_eq!(&command[147..151], &[2, 0, 0, 0]);
        assert_eq!(&command[167..171], &[2, 0, 0, 0]);
        for (field, offset) in [(151, 49u64), (171, 105u64)] {
            let image = u64::from_le_bytes(command[field..field + 8].try_into().unwrap());
            assert_eq!(image, string + offset);
            let address = slot.decode(image, 56).unwrap();
            let offset = usize::try_from(address - base.addr() as u64).unwrap();
            // SAFETY: the pending output retains the entire validated image extent.
            let image = unsafe { core::slice::from_raw_parts(base.add(offset), 56) };
            assert_eq!(&image[18..20], &[8, 5]);
            assert_eq!(&image[48..], &pixels);
        }
        output.complete(0);
        release_host.release(output.id);
        // SAFETY: the host has ended all accesses to this command and its images.
        assert_eq!(unsafe { pool.reap(&mut releases, 1) }.unwrap().outputs, 1);
        empty(&mut pool);
        host.set(4, 32);
        host.set(12, 0);
        // SAFETY: the full ring publishes nothing and retains the same generation.
        let result = unsafe { text.submit(glyphs, &mut pool, slot, &mut commands, 7) };
        assert_eq!(result, Err(Error::RingFull));
        empty(&mut pool);
    }

    #[test]
    fn glyph_records_validate_dimensions_and_complete_byte_extent() {
        let mut data = [0; 22];
        data[16..18].copy_from_slice(&9u16.to_le_bytes());
        data[18..20].copy_from_slice(&1u16.to_le_bytes());
        let glyphs = Glyphs::new(&data, 1, 1).unwrap();
        assert_eq!(glyphs.flags, 9);
        assert!(Glyphs::new(&data[..21], 1, 1).is_none());
        assert!(Glyphs::new(&data, 2, 1).is_none());
        assert!(Glyphs::new(&data, 1, 4).is_none());
        assert!(Glyphs::new(&[], 4097, 1).is_none());
        assert!(Glyphs::new(&[], 0, 2).is_none());
        assert!(Glyphs::new(&[0; 20], 1, 4).is_some());
    }

    #[test]
    fn text_layout_separates_background_and_foreground_brushes() {
        let draw = Text::new(
            [3, 5, 17, 19],
            crate::paint::Paint::Solid(0x123456),
            Some(([4, 6, 16, 18], crate::paint::Paint::Solid(0xabcdef))),
            13,
        )
        .unwrap();
        let mut bytes = [0xa5; 200];
        let mut brushes = [[0; 20]; 2];
        draw.foreground.encode(0, &mut [], &mut brushes[0]).unwrap();
        draw.background
            .unwrap()
            .1
            .encode(0, &mut [], &mut brushes[1])
            .unwrap();
        draw.encode(0x0104000000001000, &brushes, 7, &mut bytes)
            .unwrap();
        assert_eq!(&bytes[12..15], &[1, 11, 0]);
        assert_eq!(&bytes[147..155], &[1, 0, 0, 0, 0x56, 0x34, 0x12, 0]);
        assert_eq!(&bytes[167..175], &[1, 0, 0, 0, 0xef, 0xcd, 0xab, 0]);
        assert_eq!(&bytes[187..191], &[8, 0, 8, 0]);
        assert!(bytes[191..].iter().all(|byte| *byte == 0xa5));
    }
}
