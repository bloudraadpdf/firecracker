// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::bitmap::{Bitmap, ScaledBitmap, chunks};
use super::memory::Memory;
use super::protocol::{DecodeError, Rect, bytes};
use super::raster::{Operand, RasterOp};
use super::surface::Surface;
use super::transport::{AddressSpace, TransportError};

fn read<const N: usize>(
    memory: &Memory,
    addresses: &AddressSpace,
    address: u64,
    offset: usize,
) -> Result<[u8; N], TransportError> {
    let length = offset.checked_add(N).ok_or(DecodeError::Address)?;
    let (region, base) = addresses.resolve(address, length)?;
    let mut output = [0; N];
    memory.read(region, base + offset, &mut output)?;
    Ok(output)
}

fn point(data: &[u8], offset: usize) -> Result<(i32, i32), DecodeError> {
    Ok((
        i32::from_le_bytes(bytes(data, offset)?),
        i32::from_le_bytes(bytes(data, offset + 4)?),
    ))
}

#[derive(Clone, Copy, Debug)]
pub enum Layout {
    Modern,
    Compat,
}

#[derive(Debug)]
enum Brush {
    Solid(u32),
    Pattern { bitmap: Bitmap, origin: (i32, i32) },
}

impl Brush {
    fn decode(
        memory: &Memory,
        addresses: &AddressSpace,
        data: &[u8],
    ) -> Result<Self, TransportError> {
        match u32::from_le_bytes(bytes(data, 0)?) {
            1 => Ok(Self::Solid(u32::from_le_bytes(bytes(data, 4)?))),
            2 => Ok(Self::Pattern {
                bitmap: Bitmap::decode(memory, addresses, u64::from_le_bytes(bytes(data, 4)?))?,
                origin: point(data, 12)?,
            }),
            _ => Err(DecodeError::Parameter.into()),
        }
    }

    fn sample(&self, x: u32, y: u32) -> u32 {
        match self {
            Self::Solid(color) => *color,
            Self::Pattern { bitmap, origin } => {
                let (width, height) = bitmap.dimensions();
                let x = (i64::from(x) - i64::from(origin.0)).rem_euclid(i64::from(width));
                let y = (i64::from(y) - i64::from(origin.1)).rem_euclid(i64::from(height));
                bitmap.pixels()
                    [usize::try_from(y).unwrap() * width as usize + usize::try_from(x).unwrap()]
            }
        }
    }
}

#[derive(Debug)]
struct Mask {
    bitmap: Bitmap,
    origin: (i32, i32),
    inverted: bool,
}

impl Mask {
    fn decode(
        memory: &Memory,
        addresses: &AddressSpace,
        data: &[u8],
        bounds: Rect,
    ) -> Result<Option<Self>, TransportError> {
        let address = u64::from_le_bytes(bytes(data, 9)?);
        if address == 0 {
            return Ok(None);
        }
        if data[0] & !1 != 0 {
            return Err(DecodeError::Parameter.into());
        }
        let bitmap = Bitmap::decode_mask(memory, addresses, address)?;
        let origin = point(data, 1)?;
        let [left, top, right, bottom] = bounds.bounds().map(i64::from);
        let (width, height) = bitmap.dimensions();
        if origin.0 < 0
            || origin.1 < 0
            || i64::from(origin.0) + right - left > i64::from(width)
            || i64::from(origin.1) + bottom - top > i64::from(height)
        {
            return Err(DecodeError::Parameter.into());
        }
        Ok(Some(Self {
            bitmap,
            origin,
            inverted: data[0] & 1 != 0,
        }))
    }

    fn covers(&self, x: u32, y: u32, bounds: Rect) -> bool {
        let [left, top, _, _] = bounds.bounds();
        let x = i64::from(x) - i64::from(left) + i64::from(self.origin.0);
        let y = i64::from(y) - i64::from(top) + i64::from(self.origin.1);
        let index = usize::try_from(y).unwrap() * self.bitmap.dimensions().0 as usize
            + usize::try_from(x).unwrap();
        (self.bitmap.pixels()[index] != 0) ^ self.inverted
    }
}

#[derive(Debug)]
enum Operation {
    Nop,
    Fill {
        brush: Brush,
        op: RasterOp,
        mask: Option<Mask>,
    },
    Copy {
        bitmap: Bitmap,
        source: Rect,
        op: RasterOp,
        mask: Option<Mask>,
    },
    CopyBits((i32, i32)),
}

#[derive(Debug)]
pub struct Drawable {
    surface_id: u32,
    bounds: Rect,
    clips: Option<Vec<Rect>>,
    operation: Operation,
}

impl Drawable {
    pub fn decode(
        memory: &Memory,
        addresses: &AddressSpace,
        address: u64,
        layout: Layout,
    ) -> Result<Self, TransportError> {
        let (surface_id, bounds, clip, payload, opcode) = match layout {
            Layout::Modern => {
                let header = read::<123>(memory, addresses, address, 0)?;
                if header[12] > 7 || header[14] != 0 {
                    return Err(DecodeError::Parameter.into());
                }
                (
                    u32::from_le_bytes(bytes(&header, 8)?),
                    Rect::decode(&header[31..47])?,
                    bytes::<12>(&header, 47)?,
                    123,
                    header[13],
                )
            }
            Layout::Compat => {
                let header = read::<60>(memory, addresses, address, 0)?;
                if header[8] > 7 || u16::from_le_bytes(bytes(&header, 10)?) != 0 {
                    return Err(DecodeError::Parameter.into());
                }
                (
                    0,
                    Rect::decode(&header[28..44])?,
                    bytes::<12>(&header, 44)?,
                    60,
                    header[9],
                )
            }
        };
        let clips = match u32::from_le_bytes(bytes(&clip, 0)?) {
            0 => None,
            1 => {
                let pointer = u64::from_le_bytes(bytes(&clip, 4)?);
                let count = u32::from_le_bytes(read::<4>(memory, addresses, pointer, 0)?);
                if count > 4096 {
                    return Err(DecodeError::Parameter.into());
                }
                addresses.resolve(pointer, 24)?;
                let data = chunks(
                    memory,
                    addresses,
                    pointer.checked_add(4).ok_or(DecodeError::Address)?,
                    count as usize * 16,
                )?;
                Some(
                    data.as_chunks::<16>()
                        .0
                        .iter()
                        .map(|rect| Rect::decode(rect))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
            _ => return Err(DecodeError::Parameter.into()),
        };
        let operation = match opcode {
            0 => Operation::Nop,
            1 => {
                let data = read::<39>(memory, addresses, address, payload)?;
                Operation::Fill {
                    brush: Brush::decode(memory, addresses, &data[..20])?,
                    op: RasterOp::decode(u16::from_le_bytes(bytes(&data, 20)?), Operand::Brush)?,
                    mask: Mask::decode(memory, addresses, &data[22..], bounds)?,
                }
            }
            3 => {
                let data = read::<44>(memory, addresses, address, payload)?;
                if data[26] != 1 {
                    return Err(DecodeError::Parameter.into());
                }
                let bitmap =
                    Bitmap::decode(memory, addresses, u64::from_le_bytes(bytes(&data, 0)?))?;
                let source = Rect::decode(&data[8..24])?;
                bitmap.validate_rect(source)?;
                Operation::Copy {
                    bitmap,
                    source,
                    op: RasterOp::decode(u16::from_le_bytes(bytes(&data, 24)?), Operand::Source)?,
                    mask: Mask::decode(memory, addresses, &data[27..], bounds)?,
                }
            }
            4 => Operation::CopyBits(point(&read::<8>(memory, addresses, address, payload)?, 0)?),
            6..=8 => {
                let data = read::<17>(memory, addresses, address, payload)?;
                Operation::Fill {
                    brush: Brush::Solid(0),
                    op: RasterOp::decode(1 << (opcode + 1), Operand::Brush)?,
                    mask: Mask::decode(memory, addresses, &data, bounds)?,
                }
            }
            _ => return Err(DecodeError::Command(u32::from(opcode)).into()),
        };
        Ok(Self {
            surface_id,
            bounds,
            clips,
            operation,
        })
    }

    pub fn surface_id(&self) -> u32 {
        self.surface_id
    }

    pub fn render(&self, surface: &Surface<'_>) -> Result<(), TransportError> {
        if matches!(self.operation, Operation::Nop) {
            return Ok(());
        }
        let (width, height) = surface.dimensions();
        let [left, top, right, bottom] = self.bounds.clipped(width, height)?;
        if left == right || top == bottom {
            return Ok(());
        }
        let copied = if matches!(self.operation, Operation::CopyBits(_)) {
            Some(surface.snapshot()?)
        } else {
            None
        };
        let sampler = match &self.operation {
            Operation::Copy { bitmap, source, .. } => {
                ScaledBitmap::new(bitmap, *source, self.bounds)?
            }
            Operation::CopyBits(source) => {
                let bounds = self.bounds.bounds();
                let x = i64::from(source.0) + i64::from(left) - i64::from(bounds[0]);
                let y = i64::from(source.1) + i64::from(top) - i64::from(bounds[1]);
                let coordinate = |value| i32::try_from(value).map_err(|_| DecodeError::Parameter);
                let source = Rect::new(
                    coordinate(x)?,
                    coordinate(y)?,
                    coordinate(x + i64::from(right - left))?,
                    coordinate(y + i64::from(bottom - top))?,
                )?;
                let target = Rect::new(
                    i32::try_from(left).unwrap(),
                    i32::try_from(top).unwrap(),
                    i32::try_from(right).unwrap(),
                    i32::try_from(bottom).unwrap(),
                )?;
                ScaledBitmap::new(copied.as_ref().unwrap(), source, target)?
            }
            _ => None,
        };
        let clips = self
            .clips
            .as_ref()
            .map(|rects| {
                rects
                    .iter()
                    .map(|rect| rect.clipped(width, height))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        let mut spans = Vec::new();
        for y in top..bottom {
            spans.clear();
            if let Some(clips) = &clips {
                for &[clip_left, clip_top, clip_right, clip_bottom] in clips {
                    let start = left.max(clip_left);
                    let end = right.min(clip_right);
                    if y >= clip_top && y < clip_bottom && start < end {
                        spans.push((start, end));
                    }
                }
            } else {
                spans.push((left, right));
            }
            spans.sort_unstable();
            let mut index = 0;
            while index < spans.len() {
                let (start, mut end) = spans[index];
                index += 1;
                while index < spans.len() && spans[index].0 <= end {
                    end = end.max(spans[index].1);
                    index += 1;
                }
                let rect = Rect::new(
                    i32::try_from(start).unwrap(),
                    i32::try_from(y).unwrap(),
                    i32::try_from(end).unwrap(),
                    i32::try_from(y + 1).unwrap(),
                )?;
                surface.paint(rect, |x, y, destination| match &self.operation {
                    Operation::Fill { brush, op, mask } => {
                        if mask
                            .as_ref()
                            .is_none_or(|mask| mask.covers(x, y, self.bounds))
                        {
                            op.apply(0, brush.sample(x, y), destination)
                        } else {
                            destination
                        }
                    }
                    Operation::Copy { op, mask, .. } => {
                        if mask
                            .as_ref()
                            .is_none_or(|mask| mask.covers(x, y, self.bounds))
                        {
                            op.apply(sampler.as_ref().unwrap().sample(x, y), 0, destination)
                        } else {
                            destination
                        }
                    }
                    Operation::CopyBits(_) => sampler.as_ref().unwrap().sample(x, y),
                    Operation::Nop => destination,
                })?;
            }
        }
        Ok(())
    }
}
