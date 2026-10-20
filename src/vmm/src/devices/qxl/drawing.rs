// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::bitmap::{Bitmap, ScaleMode, ScaledBitmap, chunks};
use super::glyph::GlyphString;
use super::memory::Memory;
use super::path::Path;
use super::protocol::{DecodeError, Rect, bytes};
use super::raster::{Operand, RasterOp, alpha_over, coverage_mix, ternary};
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
struct Paint {
    brush: Brush,
    op: RasterOp,
}

impl Paint {
    fn decode(
        memory: &Memory,
        addresses: &AddressSpace,
        data: &[u8],
        descriptor: u16,
    ) -> Result<Self, TransportError> {
        let op = RasterOp::decode(descriptor, Operand::Brush)?;
        let brush = if op.uses_operand() {
            Brush::decode(memory, addresses, data)?
        } else {
            Brush::Solid(0)
        };
        Ok(Self { brush, op })
    }

    fn apply(&self, x: u32, y: u32, destination: u32) -> u32 {
        self.op.apply(0, self.brush.sample(x, y), destination)
    }
}

#[derive(Debug)]
struct Background {
    bounds: Rect,
    paint: Paint,
}

#[derive(Debug)]
struct Stroke {
    path: Path,
    paint: Option<Paint>,
}

impl Stroke {
    fn decode(
        memory: &Memory,
        addresses: &AddressSpace,
        data: &[u8],
    ) -> Result<Self, TransportError> {
        let path = Path::decode(
            memory,
            addresses,
            u64::from_le_bytes(bytes(data, 0)?),
            &data[8..28],
        )?;
        // QXL backends use on/off dashes. Windows emits background spans as
        // a separate stroke; back_mode does not paint the gaps.
        let descriptor = u16::from_le_bytes(bytes(data, 48)?);
        let paint = (descriptor != 0)
            .then(|| Paint::decode(memory, addresses, &data[28..48], descriptor))
            .transpose()?;
        Ok(Self { path, paint })
    }

    fn apply(&self, foreground: bool, x: u32, y: u32, destination: u32) -> u32 {
        if !foreground {
            return destination;
        }
        self.paint
            .as_ref()
            .map_or(destination, |paint| paint.apply(x, y, destination))
    }
}

#[derive(Debug)]
enum ImageAction {
    Raster(RasterOp),
    Opaque {
        brush: Brush,
        op: RasterOp,
    },
    Ternary {
        brush: Brush,
        opcode: u8,
    },
    Transparent(u32),
    Alpha {
        alpha: u8,
        destination_has_alpha: bool,
        source_has_alpha: bool,
    },
}

impl ImageAction {
    fn uses_source(&self) -> bool {
        match self {
            Self::Raster(op) | Self::Opaque { op, .. } => op.uses_operand(),
            Self::Ternary { opcode, .. } => (opcode ^ (opcode >> 2)) & 0x33 != 0,
            Self::Transparent(_) | Self::Alpha { .. } => true,
        }
    }

    fn apply(&self, source: u32, x: u32, y: u32, destination: u32) -> u32 {
        match self {
            Self::Raster(op) => op.apply(source, 0, destination),
            Self::Opaque { brush, op } => op.apply_opaque(source, brush.sample(x, y)),
            Self::Ternary { brush, opcode } => {
                ternary(*opcode, source, brush.sample(x, y), destination)
            }
            Self::Transparent(color) => {
                if source & 0xffffff == *color & 0xffffff {
                    destination
                } else {
                    source
                }
            }
            Self::Alpha {
                alpha,
                destination_has_alpha,
                source_has_alpha,
            } => alpha_over(
                source,
                destination,
                *alpha,
                *destination_has_alpha,
                *source_has_alpha,
            ),
        }
    }
}

#[derive(Debug)]
enum ImagePixels {
    Bitmap(Bitmap),
    PrimarySurface,
}

#[derive(Debug)]
struct ImageSource {
    pixels: ImagePixels,
    area: Rect,
}

#[derive(Debug)]
enum Operation {
    Nop,
    Fill {
        paint: Paint,
        mask: Option<Mask>,
    },
    Image {
        source: Option<ImageSource>,
        action: ImageAction,
        mode: ScaleMode,
        mask: Option<Mask>,
    },
    CopyBits((i32, i32)),
    Stroke(Stroke),
    Text {
        string: GlyphString,
        foreground: Option<Paint>,
        background: Option<Background>,
    },
}

impl Operation {
    fn image(
        memory: &Memory,
        addresses: &AddressSpace,
        data: &[u8],
        mut action: ImageAction,
        mode: u8,
        mask: Option<&[u8]>,
        bounds: Rect,
    ) -> Result<Self, TransportError> {
        let (source, mode) = if action.uses_source() {
            let address = u64::from_le_bytes(bytes(data, 0)?);
            let header = read::<18>(memory, addresses, address, 0)?;
            let area = Rect::decode(&data[8..24])?;
            let pixels = if header[8] == 104 {
                let id = u32::from_le_bytes(read::<4>(memory, addresses, address, 18)?);
                if id != 0 || header[9] & !3 != 0 {
                    return Err(DecodeError::Parameter.into());
                }
                ImagePixels::PrimarySurface
            } else {
                let bitmap = Bitmap::decode(memory, addresses, address)?;
                bitmap.validate_rect(area)?;
                if let ImageAction::Alpha {
                    source_has_alpha, ..
                } = &mut action
                {
                    *source_has_alpha =
                        matches!(read::<1>(memory, addresses, address, 18)?[0], 9 | 10);
                }
                ImagePixels::Bitmap(bitmap)
            };
            (Some(ImageSource { pixels, area }), ScaleMode::decode(mode)?)
        } else {
            (None, ScaleMode::Nearest)
        };
        let mask = mask
            .map(|data| Mask::decode(memory, addresses, data, bounds))
            .transpose()?
            .flatten();
        Ok(Self::Image {
            source,
            action,
            mode,
            mask,
        })
    }
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
                    paint: Paint::decode(
                        memory,
                        addresses,
                        &data[..20],
                        u16::from_le_bytes(bytes(&data, 20)?),
                    )?,
                    mask: Mask::decode(memory, addresses, &data[22..], bounds)?,
                }
            }
            2 => {
                let data = read::<64>(memory, addresses, address, payload)?;
                let op = RasterOp::decode(u16::from_le_bytes(bytes(&data, 44)?), Operand::Source)?;
                let brush = if op.uses_destination() {
                    Brush::decode(memory, addresses, &data[24..44])?
                } else {
                    Brush::Solid(0)
                };
                Operation::image(
                    memory,
                    addresses,
                    &data,
                    ImageAction::Opaque { brush, op },
                    data[46],
                    Some(&data[47..]),
                    bounds,
                )?
            }
            3 | 5 => {
                let data = read::<44>(memory, addresses, address, payload)?;
                let op = RasterOp::decode(u16::from_le_bytes(bytes(&data, 24)?), Operand::Source)?;
                Operation::image(
                    memory,
                    addresses,
                    &data,
                    ImageAction::Raster(op),
                    data[26],
                    Some(&data[27..]),
                    bounds,
                )?
            }
            4 => Operation::CopyBits(point(&read::<8>(memory, addresses, address, payload)?, 0)?),
            6..=8 => {
                let data = read::<17>(memory, addresses, address, payload)?;
                Operation::Fill {
                    paint: Paint {
                        brush: Brush::Solid(0),
                        op: RasterOp::decode(1 << (opcode + 1), Operand::Brush)?,
                    },
                    mask: Mask::decode(memory, addresses, &data, bounds)?,
                }
            }
            9 => {
                let data = read::<63>(memory, addresses, address, payload)?;
                let opcode = data[44];
                let brush = if (opcode ^ (opcode >> 4)) & 15 != 0 {
                    Brush::decode(memory, addresses, &data[24..44])?
                } else {
                    Brush::Solid(0)
                };
                Operation::image(
                    memory,
                    addresses,
                    &data,
                    ImageAction::Ternary { brush, opcode },
                    data[45],
                    Some(&data[46..]),
                    bounds,
                )?
            }
            10 => Operation::Stroke(Stroke::decode(
                memory,
                addresses,
                &read::<52>(memory, addresses, address, payload)?,
            )?),
            11 => {
                let data = read::<68>(memory, addresses, address, payload)?;
                let string =
                    GlyphString::decode(memory, addresses, u64::from_le_bytes(bytes(&data, 0)?))?;
                let foreground = if string.is_empty() {
                    None
                } else {
                    Some(Paint::decode(
                        memory,
                        addresses,
                        &data[24..44],
                        u16::from_le_bytes(bytes(&data, 64)?),
                    )?)
                };
                let area = Rect::decode(&data[8..24])?;
                let [left, top, right, bottom] = area.bounds();
                let background = if left == right || top == bottom {
                    None
                } else {
                    Some(Background {
                        bounds: area,
                        paint: Paint::decode(
                            memory,
                            addresses,
                            &data[44..64],
                            u16::from_le_bytes(bytes(&data, 66)?),
                        )?,
                    })
                };
                Operation::Text {
                    string,
                    foreground,
                    background,
                }
            }
            12 => {
                let data = read::<32>(memory, addresses, address, payload)?;
                Operation::image(
                    memory,
                    addresses,
                    &data,
                    ImageAction::Transparent(u32::from_le_bytes(bytes(&data, 28)?)),
                    1,
                    None,
                    bounds,
                )?
            }
            13 => {
                let (flags, alpha, data) = match layout {
                    Layout::Modern => {
                        let data = read::<27>(memory, addresses, address, payload)?;
                        (
                            u16::from_le_bytes(bytes(&data, 0)?),
                            data[2],
                            bytes::<24>(&data, 3)?,
                        )
                    }
                    Layout::Compat => {
                        let data = read::<25>(memory, addresses, address, payload)?;
                        (0, data[0], bytes::<24>(&data, 1)?)
                    }
                };
                if flags & !3 != 0 {
                    return Err(DecodeError::Parameter.into());
                }
                Operation::image(
                    memory,
                    addresses,
                    &data,
                    ImageAction::Alpha {
                        alpha,
                        destination_has_alpha: flags & 1 != 0,
                        source_has_alpha: flags & 2 != 0,
                    },
                    0,
                    None,
                    bounds,
                )?
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
        let copied = if matches!(
            self.operation,
            Operation::CopyBits(_)
                | Operation::Image {
                    source: Some(ImageSource {
                        pixels: ImagePixels::PrimarySurface,
                        ..
                    }),
                    ..
                }
        ) {
            Some(surface.snapshot()?)
        } else {
            None
        };
        let sampler = match &self.operation {
            Operation::Image {
                source: Some(source),
                mode,
                ..
            } => {
                let bitmap = match &source.pixels {
                    ImagePixels::Bitmap(bitmap) => bitmap,
                    ImagePixels::PrimarySurface => copied.as_ref().unwrap(),
                };
                ScaledBitmap::with_mode(bitmap, source.area, self.bounds, *mode)?
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
        let coverage = if let Operation::Text { string, .. } = &self.operation {
            Some(string.coverage([left, top, right, bottom])?)
        } else {
            None
        };
        let mut stroke_pixels = if let Operation::Stroke(stroke) = &self.operation {
            stroke.path.pixels(width, height)?
        } else {
            Vec::new()
        }
        .into_iter()
        .peekable();
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
                    Operation::Fill { paint, mask } => {
                        if mask
                            .as_ref()
                            .is_none_or(|mask| mask.covers(x, y, self.bounds))
                        {
                            paint.apply(x, y, destination)
                        } else {
                            destination
                        }
                    }
                    Operation::Image { action, mask, .. } => {
                        if mask
                            .as_ref()
                            .is_none_or(|mask| mask.covers(x, y, self.bounds))
                        {
                            action.apply(
                                sampler.as_ref().map_or(0, |source| source.sample(x, y)),
                                x,
                                y,
                                destination,
                            )
                        } else {
                            destination
                        }
                    }
                    Operation::CopyBits(_) => sampler.as_ref().unwrap().sample(x, y),
                    Operation::Stroke(stroke) => {
                        while stroke_pixels
                            .peek()
                            .is_some_and(|pixel| (pixel.point[1], pixel.point[0]) < (y, x))
                        {
                            stroke_pixels.next();
                        }
                        let mut result = destination;
                        while stroke_pixels
                            .peek()
                            .is_some_and(|pixel| pixel.point == [x, y])
                        {
                            let pixel = stroke_pixels.next().unwrap();
                            result = stroke.apply(pixel.foreground, x, y, result);
                        }
                        result
                    }
                    Operation::Text {
                        foreground,
                        background,
                        ..
                    } => {
                        let destination = if let Some(background) = background {
                            let [left, top, right, bottom] =
                                background.bounds.bounds().map(i64::from);
                            if i64::from(x) >= left
                                && i64::from(x) < right
                                && i64::from(y) >= top
                                && i64::from(y) < bottom
                            {
                                background.paint.apply(x, y, destination)
                            } else {
                                destination
                            }
                        } else {
                            destination
                        };
                        if let Some(foreground) = foreground {
                            let index =
                                (y - top) as usize * (right - left) as usize + (x - left) as usize;
                            let alpha = coverage.as_ref().unwrap()[index];
                            coverage_mix(foreground.apply(x, y, destination), destination, alpha)
                        } else {
                            destination
                        }
                    }
                    Operation::Nop => destination,
                })?;
            }
        }
        Ok(())
    }
}
