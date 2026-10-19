// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::bitmap::Bitmap;
use super::memory::{Memory, Region};
use super::protocol::{DecodeError, PrimarySurface, Rect};
use super::raster::{Operand, RasterOp};
use super::transport::{AddressSpace, TransportError};
use crate::devices::display::Frame;

#[derive(Debug)]
pub struct Surface<'a> {
    memory: &'a Memory,
    region: Region,
    offset: usize,
    primary: PrimarySurface,
}

impl<'a> Surface<'a> {
    pub fn open(
        memory: &'a Memory,
        addresses: &AddressSpace,
        primary: PrimarySurface,
    ) -> Result<Self, TransportError> {
        let (region, offset) = addresses.resolve(
            primary.address(),
            usize::try_from(primary.length()).unwrap(),
        )?;
        Ok(Self {
            memory,
            region,
            offset,
            primary,
        })
    }

    fn row(&self, y: u32) -> usize {
        let (_, height) = self.primary.dimensions();
        let row = if self.primary.stride() < 0 {
            height - 1 - y
        } else {
            y
        };
        self.offset
            + usize::try_from(row).unwrap()
                * usize::try_from(self.primary.stride().unsigned_abs()).unwrap()
    }

    pub fn clear(&self) -> Result<(), TransportError> {
        let (width, height) = self.primary.dimensions();
        let row = vec![0; usize::try_from(width).unwrap() * 4];
        for y in 0..height {
            self.memory.write(self.region, self.row(y), &row)?;
        }
        Ok(())
    }

    pub fn frame(&self) -> Result<Frame, TransportError> {
        let (width, height) = self.primary.dimensions();
        let mut pixels = Vec::with_capacity(usize::try_from(width * height).unwrap());
        let mut row = vec![0; usize::try_from(width).unwrap() * 4];
        for y in 0..height {
            self.memory.read(self.region, self.row(y), &mut row)?;
            pixels.extend(
                row.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|pixel| u32::from_le_bytes(*pixel) & 0x00ff_ffff),
            );
        }
        Ok(Frame {
            width: u16::try_from(width).unwrap(),
            height: u16::try_from(height).unwrap(),
            pixels,
        })
    }

    pub fn fill(&self, rect: Rect, color: u32) -> Result<(), TransportError> {
        let (width, height) = self.primary.dimensions();
        let [left, top, right, bottom] = rect.clipped(width, height)?;
        let pixel = color.to_le_bytes();
        let row = pixel.repeat(usize::try_from(right - left).unwrap());
        for y in top..bottom {
            self.memory.write(
                self.region,
                self.row(y) + usize::try_from(left).unwrap() * 4,
                &row,
            )?;
        }
        Ok(())
    }

    pub fn copy_bits(&self, rect: Rect, source: (i32, i32)) -> Result<(), TransportError> {
        let (width, height) = self.primary.dimensions();
        let [left, top, right, bottom] = rect.clipped(width, height)?;
        if left == right || top == bottom {
            return Ok(());
        }
        let bounds = rect.bounds();
        let source_x = i64::from(source.0) + i64::from(left) - i64::from(bounds[0]);
        let source_y = i64::from(source.1) + i64::from(top) - i64::from(bounds[1]);
        let columns = right - left;
        let rows = bottom - top;
        if source_x < 0
            || source_y < 0
            || source_x + i64::from(columns) > i64::from(width)
            || source_y + i64::from(rows) > i64::from(height)
        {
            return Err(DecodeError::Parameter.into());
        }
        let source_y = u32::try_from(source_y).unwrap();
        let source_x = usize::try_from(source_x).unwrap() * 4;
        let target_x = usize::try_from(left).unwrap() * 4;
        let mut row = vec![0; usize::try_from(columns).unwrap() * 4];
        for index in 0..rows {
            let index = if top > source_y {
                rows - 1 - index
            } else {
                index
            };
            self.memory
                .read(self.region, self.row(source_y + index) + source_x, &mut row)?;
            self.memory
                .write(self.region, self.row(top + index) + target_x, &row)?;
        }
        Ok(())
    }

    pub fn raster_fill(
        &self,
        rect: Rect,
        brush: u32,
        descriptor: u16,
    ) -> Result<(), TransportError> {
        let op = RasterOp::decode(descriptor, Operand::Brush)?;
        self.paint(rect, |_, _, destination| op.apply(0, brush, destination))
    }

    pub fn blit(
        &self,
        bitmap: &Bitmap,
        source: Rect,
        target: Rect,
        descriptor: u16,
    ) -> Result<(), TransportError> {
        let op = RasterOp::decode(descriptor, Operand::Source)?;
        let [left, top, right, bottom] = source.bounds().map(i64::from);
        let [target_left, target_top, target_right, target_bottom] = target.bounds().map(i64::from);
        let (width, height) = bitmap.dimensions();
        if left < 0
            || top < 0
            || right > i64::from(width)
            || bottom > i64::from(height)
            || left == right
            || top == bottom
        {
            return Err(DecodeError::Parameter.into());
        }
        if target_left == target_right || target_top == target_bottom {
            return Ok(());
        }
        self.paint(target, |x, y, destination| {
            let x =
                left + (i64::from(x) - target_left) * (right - left) / (target_right - target_left);
            let y =
                top + (i64::from(y) - target_top) * (bottom - top) / (target_bottom - target_top);
            let index = usize::try_from(y).unwrap() * usize::try_from(width).unwrap()
                + usize::try_from(x).unwrap();
            op.apply(bitmap.pixels()[index], 0, destination)
        })
    }

    fn paint(
        &self,
        rect: Rect,
        mut pixel: impl FnMut(u32, u32, u32) -> u32,
    ) -> Result<(), TransportError> {
        let (width, height) = self.primary.dimensions();
        let [left, top, right, bottom] = rect.clipped(width, height)?;
        let mut row = vec![0; usize::try_from(right - left).unwrap() * 4];
        for y in top..bottom {
            let offset = self.row(y) + usize::try_from(left).unwrap() * 4;
            self.memory.read(self.region, offset, &mut row)?;
            for (index, bytes) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let x = left + u32::try_from(index).unwrap();
                *bytes = pixel(x, y, u32::from_le_bytes(*bytes)).to_le_bytes();
            }
            self.memory.write(self.region, offset, &row)?;
        }
        Ok(())
    }
}
