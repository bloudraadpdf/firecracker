// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Masked and ternary drawables from display/rop.c.

use crate::display_resources::{Command, Error, Pool, Region};
use crate::paint::Paint;
use crate::ring::CommandRing;
use crate::rop::Rop3;
use crate::slots::MemorySlot;

#[derive(Clone, Copy, Debug)]
pub struct Bitmap<'a> {
    width: u32,
    height: u32,
    pixels: &'a [u8],
}

impl<'a> Bitmap<'a> {
    pub fn new(width: u32, height: u32, pixels: &'a [u8]) -> Option<Self> {
        let length = width.checked_mul(4)?.checked_mul(height)? as usize;
        (width != 0 && height != 0 && width <= 4096 && height <= 4096 && pixels.len() == length)
            .then_some(Self {
                width,
                height,
                pixels,
            })
    }

    fn bytes(self) -> usize {
        48 + self.pixels.len()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Mask<'a> {
    width: u32,
    height: u32,
    pixels: &'a [u8],
    origin: [i32; 2],
    inverted: bool,
}

impl<'a> Mask<'a> {
    pub fn new(
        width: u32,
        height: u32,
        pixels: &'a [u8],
        origin: [i32; 2],
        inverted: bool,
    ) -> Option<Self> {
        let length = width.div_ceil(8).checked_mul(height)? as usize;
        (width != 0
            && height != 0
            && width <= 4096
            && height <= 4096
            && pixels.len() == length
            && origin[0] >= 0
            && origin[1] >= 0)
            .then_some(Self {
                width,
                height,
                pixels,
                origin,
                inverted,
            })
    }

    fn bytes(self) -> usize {
        48 + self.pixels.len()
    }

    fn encode(self, physical: u64, bytes: &mut [u8]) -> Result<(), Error> {
        let bytes = bytes.get_mut(..self.bytes()).ok_or(Error::InvalidSize)?;
        bytes[..48].fill(0);
        bytes[..8].copy_from_slice(&3u64.to_le_bytes());
        bytes[10..14].copy_from_slice(&self.width.to_le_bytes());
        bytes[14..18].copy_from_slice(&self.height.to_le_bytes());
        bytes[18..20].copy_from_slice(&[2, 5]);
        bytes[20..24].copy_from_slice(&self.width.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.height.to_le_bytes());
        bytes[28..32].copy_from_slice(&self.width.div_ceil(8).to_le_bytes());
        bytes[40..48].copy_from_slice(
            &physical
                .checked_add(48)
                .ok_or(Error::InvalidSize)?
                .to_le_bytes(),
        );
        bytes[48..].copy_from_slice(self.pixels);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Raster<'a> {
    bounds: [i32; 4],
    truth: u8,
    source: Option<Bitmap<'a>>,
    paint: Paint<'a>,
    mask: Option<Mask<'a>>,
    clip: Option<[i32; 4]>,
}

impl<'a> Raster<'a> {
    pub fn new(
        bounds: [i32; 4],
        truth: u8,
        source: Option<Bitmap<'a>>,
        paint: Paint<'a>,
        mask: Option<Mask<'a>>,
    ) -> Option<Self> {
        let [left, top, right, bottom] = bounds;
        if left < 0 || top < 0 || left >= right || top >= bottom {
            return None;
        }
        let operation = Rop3::new(truth);
        let source = if operation.inputs().source {
            Some(source?)
        } else {
            None
        };
        let paint = if operation.inputs().brush {
            paint
        } else {
            Paint::Solid(0)
        };
        if let Some(mask) = mask
            && (i64::from(mask.origin[0]) + i64::from(right - left) > i64::from(mask.width)
                || i64::from(mask.origin[1]) + i64::from(bottom - top) > i64::from(mask.height))
        {
            return None;
        }
        Some(Self {
            bounds,
            truth,
            source,
            paint,
            mask,
            clip: None,
        })
    }

    pub fn bounds(self) -> [i32; 4] {
        self.bounds
    }

    pub fn with_clip(mut self, clip: [i32; 4]) -> Option<Self> {
        let [left, top, right, bottom] = clip;
        if left < self.bounds[0]
            || top < self.bounds[1]
            || left >= right
            || top >= bottom
            || right > self.bounds[2]
            || bottom > self.bounds[3]
        {
            return None;
        }
        if clip != self.bounds {
            self.clip = Some(clip);
        }
        Some(self)
    }

    fn encode(
        self,
        addresses: [u64; 3],
        brush: &[u8; 20],
        clock: u32,
        bytes: &mut [u8],
    ) -> Result<(), Error> {
        let [source, mask, clip] = addresses;
        let bytes = bytes.get_mut(..186).ok_or(Error::InvalidSize)?;
        crate::draw::header(bytes, self.bounds, clock, 0, 9);
        if self.clip.is_some() {
            bytes[47..51].copy_from_slice(&1u32.to_le_bytes());
            bytes[51..59].copy_from_slice(&clip.to_le_bytes());
        }
        if let Some(bitmap) = self.source {
            bytes[123..131].copy_from_slice(&source.to_le_bytes());
            crate::draw::rect(
                bytes,
                131,
                [0, 0, bitmap.width as i32, bitmap.height as i32],
            );
        }
        bytes[147..167].copy_from_slice(brush);
        bytes[167] = self.truth;
        bytes[168] = 1;
        if let Some(bitmap) = self.mask {
            bytes[169] = u8::from(bitmap.inverted);
            bytes[170..174].copy_from_slice(&bitmap.origin[0].to_le_bytes());
            bytes[174..178].copy_from_slice(&bitmap.origin[1].to_le_bytes());
            bytes[178..186].copy_from_slice(&mask.to_le_bytes());
        }
        Ok(())
    }

    /// # Safety
    /// Retain the active primary, exclusive queue and pool generation through
    /// host completion. All CPU access must be serialised with submitted draws.
    pub unsafe fn submit(
        self,
        pool: &mut Pool<'_>,
        slot: MemorySlot,
        ring: &mut CommandRing,
        clock: u32,
    ) -> Result<bool, Error> {
        let paint_offset = self.source.map_or(0, Bitmap::bytes);
        let mask_offset = paint_offset + self.paint.storage_bytes();
        let clip_offset = mask_offset + self.mask.map_or(0, Mask::bytes);
        let length = (clip_offset + if self.clip.is_some() { 40 } else { 0 }).max(1);
        let memory = pool.resource(Region::Main, length)?;
        let physical = match slot.encode(memory.as_ptr().addr() as u64, length as u64) {
            Ok(physical) => physical,
            Err(error) => {
                pool.release(memory).map_err(|(error, _)| error)?;
                return Err(Error::Address(error));
            }
        };
        // SAFETY: this resource owns the complete unpublished output allocation.
        let bytes = unsafe { core::slice::from_raw_parts_mut(memory.as_ptr(), length) };
        let result = (|| {
            if let Some(source) = self.source {
                crate::image::encode(
                    source.width,
                    source.height,
                    source.pixels,
                    false,
                    physical,
                    &mut bytes[..paint_offset],
                )?;
            }
            let mut brush = [0; 20];
            self.paint.encode(
                physical + paint_offset as u64,
                &mut bytes[paint_offset..mask_offset],
                &mut brush,
            )?;
            if let Some(mask) = self.mask {
                mask.encode(
                    physical + mask_offset as u64,
                    &mut bytes[mask_offset..clip_offset],
                )?;
            }
            if let Some(clip) = self.clip {
                let record = &mut bytes[clip_offset..clip_offset + 40];
                record.fill(0);
                record[..4].copy_from_slice(&1u32.to_le_bytes());
                record[4..8].copy_from_slice(&16u32.to_le_bytes());
                crate::draw::rect(record, 24, clip);
            }
            let mut output = pool.output(Command::Draw, 186)?;
            if let Err(error) = pool.payload(&mut output).and_then(|bytes| {
                self.encode(
                    [
                        physical,
                        physical + mask_offset as u64,
                        physical + clip_offset as u64,
                    ],
                    &brush,
                    clock,
                    bytes,
                )
            }) {
                pool.cancel(output).map_err(|(error, _)| error)?;
                return Err(error);
            }
            // SAFETY: one immutable allocation retains all source, pattern and mask data.
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
    fn combined_source_pattern_mask_and_clip_live_until_release_and_cancel_cleanly() {
        use crate::display_resources::test_utils::{Bytes, HostRing, empty, pool};
        use crate::paint::Pattern;
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
        // SAFETY: the fake host retains each independent, exclusively owned endpoint.
        let (mut commands, mut releases) = unsafe {
            (
                CommandRing::new(host.base, 532).unwrap(),
                ReleaseRing::new(release_host.base, 84).unwrap(),
            )
        };
        let source = Bitmap::new(2, 2, &[1; 16]).unwrap();
        let paint = Paint::Pattern(Pattern::new(2, 1, &[2; 8], [-3, 5]).unwrap());
        let mask = Mask::new(2, 2, &[0x80, 0x40], [0, 0], true).unwrap();
        let raster = Raster::new([3, 5, 5, 7], 0xb8, Some(source), paint, Some(mask))
            .unwrap()
            .with_clip([4, 5, 5, 6])
            .unwrap();
        // SAFETY: the simulated destination and mapped endpoints remain retained.
        unsafe { raster.submit(&mut pool, slot, &mut commands, 7) }.unwrap();
        let output = host.command(&pool, 0);
        // SAFETY: the pending drawable owns its complete 186-byte payload.
        let command = unsafe { output.snapshot::<186>() };
        let source = u64::from_le_bytes(command[123..131].try_into().unwrap());
        for (field, offset) in [(151, 64), (178, 120), (51, 170)] {
            assert_eq!(
                u64::from_le_bytes(command[field..field + 8].try_into().unwrap()),
                source + offset
            );
        }
        assert_eq!(&command[47..51], &[1, 0, 0, 0]);
        let address = slot.decode(source, 210).unwrap();
        let offset = usize::try_from(address - base.addr() as u64).unwrap();
        // SAFETY: one retained allocation owns all four complete source records.
        let data = unsafe { core::slice::from_raw_parts(base.add(offset), 210) };
        assert_eq!(&data[48..64], &[1; 16]);
        assert_eq!(&data[112..120], &[2; 8]);
        assert_eq!(&data[138..140], &[2, 5]);
        assert_eq!(&data[168..170], &[0x80, 0x40]);
        assert_eq!(&data[170..178], &[1, 0, 0, 0, 16, 0, 0, 0]);
        output.complete(0);
        release_host.release(output.id);
        // SAFETY: the host has ended every draw, source, mask and clip access.
        assert_eq!(unsafe { pool.reap(&mut releases, 1) }.unwrap().outputs, 1);
        empty(&mut pool);
        host.set(4, 32);
        host.set(12, 0);
        // SAFETY: this full ring cannot publish or retain a partial command.
        let result = unsafe { raster.submit(&mut pool, slot, &mut commands, 7) };
        assert_eq!(result, Err(Error::RingFull));
        empty(&mut pool);
    }

    #[test]
    fn raster_records_preserve_mask_bit_order_origin_and_rop_truth() {
        let source = Bitmap::new(2, 1, &[0; 8]).unwrap();
        let mask = Mask::new(9, 1, &[0x81, 0x80], [1, 0], true).unwrap();
        let raster = Raster::new(
            [3, 5, 5, 6],
            0xb8,
            Some(source),
            Paint::Solid(0x123456),
            Some(mask),
        )
        .unwrap();
        let mut bytes = [0xa5; 190];
        let mut brush = [0; 20];
        raster.paint.encode(0, &mut [], &mut brush).unwrap();
        raster
            .encode([0x1000, 0x2000, 0], &brush, 7, &mut bytes)
            .unwrap();
        assert_eq!(bytes[13], 9);
        assert_eq!(&bytes[167..178], &[0xb8, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            u64::from_le_bytes(bytes[178..186].try_into().unwrap()),
            0x2000
        );
        assert_eq!(&bytes[186..], &[0xa5; 4]);
        let mut encoded = [0xa5; 50];
        mask.encode(0x2000, &mut encoded).unwrap();
        assert_eq!(&encoded[18..20], &[2, 5]);
        assert_eq!(&encoded[48..], &[0x81, 0x80]);
        assert!(
            Raster::new(
                [3, 5, 12, 6],
                0xb8,
                Some(source),
                Paint::Solid(0),
                Some(mask)
            )
            .is_none()
        );
        assert!(Raster::new([3, 5, 5, 6], 0xcc, None, Paint::Solid(0), None).is_none());
        assert!(Raster::new([3, 5, 5, 6], 0xf0, None, Paint::Solid(0), None).is_some());
    }
}
