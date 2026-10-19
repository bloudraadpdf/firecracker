// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::bitmap::chunks;
use super::line::{Progression, rasterize};
use super::memory::Memory;
use super::protocol::{DecodeError, bytes};
use super::transport::{AddressSpace, TransportError};

#[derive(Debug)]
struct Figure {
    points: Vec<[i32; 2]>,
    closed: bool,
    reset_style: bool,
}

#[derive(Debug)]
struct Style {
    ends: Vec<u64>,
    period: u64,
    start_with_gap: bool,
}

impl Style {
    fn decode(
        memory: &Memory,
        addresses: &AddressSpace,
        attr: &[u8],
    ) -> Result<Option<Self>, TransportError> {
        if attr[0] & !12 != 0 || i32::from_le_bytes(bytes(attr, 4)?) != 16 {
            return Err(DecodeError::Parameter.into());
        }
        if attr[0] & 8 == 0 {
            return Ok(None);
        }
        let count = usize::from(attr[3]);
        if count == 0 || count > 100 {
            return Err(DecodeError::Parameter.into());
        }
        let mut data = vec![0; count * 4];
        addresses.read(memory, u64::from_le_bytes(bytes(attr, 12)?), &mut data)?;
        let mut total = 0;
        let mut ends = Vec::with_capacity(count);
        for length in data.as_chunks::<4>().0 {
            let length = i32::from_le_bytes(*length);
            if length <= 0 {
                return Err(DecodeError::Parameter.into());
            }
            total += u64::try_from(length).unwrap();
            ends.push(total);
        }
        Ok(Some(Self {
            ends,
            period: total * if count % 2 == 0 { 1 } else { 2 },
            start_with_gap: attr[0] & 4 != 0,
        }))
    }

    fn foreground(&self, position: u64) -> bool {
        let position = position % self.period;
        let length = *self.ends.last().unwrap();
        let index = self.ends.partition_point(|&end| end <= position % length);
        (index % 2 == 0) ^ self.start_with_gap ^ (position >= length)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Pixel {
    pub point: [u32; 2],
    pub foreground: bool,
}

#[derive(Debug)]
pub(super) struct Path {
    figures: Vec<Figure>,
    style: Option<Style>,
}

impl Path {
    pub fn decode(
        memory: &Memory,
        addresses: &AddressSpace,
        address: u64,
        attr: &[u8],
    ) -> Result<Self, TransportError> {
        bytes::<20>(attr, 0)?;
        let style = Style::decode(memory, addresses, attr)?;
        let mut header = [0; 4];
        addresses.read(memory, address, &mut header)?;
        let length = u32::from_le_bytes(header) as usize;
        if length > 1 << 20 {
            return Err(DecodeError::Parameter.into());
        }
        addresses.resolve(address, 24)?;
        let data = chunks(
            memory,
            addresses,
            address.checked_add(4).ok_or(DecodeError::Address)?,
            length,
        )?;
        let mut figures = Vec::new();
        let mut current: Option<Figure> = None;
        let mut position = 0;
        let mut total = 0;
        while position < data.len() {
            let flags = u32::from_le_bytes(bytes(&data, position)?);
            let count = u32::from_le_bytes(bytes(&data, position + 4)?) as usize;
            position += 8;
            total += count;
            if flags & !15 != 0
                || count == 0
                || total > 65536
                || flags & 8 != 0 && flags & 2 == 0
                || flags & 4 != 0 && flags & 1 == 0
            {
                return Err(DecodeError::Parameter.into());
            }
            if flags & 1 != 0 {
                if current.is_some() {
                    return Err(DecodeError::Parameter.into());
                }
                current = Some(Figure {
                    points: Vec::new(),
                    closed: false,
                    reset_style: flags & 4 != 0,
                });
            }
            let figure = current.as_mut().ok_or(DecodeError::Parameter)?;
            let points = data
                .get(position..position + count * 8)
                .ok_or(DecodeError::Truncated)?;
            for point in points.as_chunks::<8>().0 {
                figure.points.push([
                    i32::from_le_bytes(bytes(point, 0)?),
                    i32::from_le_bytes(bytes(point, 4)?),
                ]);
            }
            position += count * 8;
            if flags & 2 != 0 {
                figure.closed = flags & 8 != 0;
                figures.push(current.take().unwrap());
            }
        }
        if current.is_some() {
            return Err(DecodeError::Truncated.into());
        }
        Ok(Self { figures, style })
    }

    pub fn is_styled(&self) -> bool {
        self.style.is_some()
    }

    pub fn pixels(&self, width: u32, height: u32) -> Result<Vec<Pixel>, DecodeError> {
        if width > 4096 || height > 4096 {
            return Err(DecodeError::Parameter);
        }
        let mut pixels = Vec::new();
        let mut phase = 0;
        let mut work = 0;
        for figure in &self.figures {
            if figure.reset_style {
                phase = 0;
            }
            let closing = figure
                .closed
                .then(|| [*figure.points.last().unwrap(), figure.points[0]]);
            for segment in figure
                .points
                .windows(2)
                .map(|points| [points[0], points[1]])
                .chain(closing)
            {
                let [start, end] = segment;
                let Some(progression) = Progression::new(start, end) else {
                    continue;
                };
                work += progression.length.min(u64::from(width.max(height)));
                if work > 1 << 24 {
                    return Err(DecodeError::Parameter);
                }
                for point in rasterize(start, end, width, height)? {
                    if pixels.len() == 1 << 20 {
                        return Err(DecodeError::Parameter);
                    }
                    pixels.push(Pixel {
                        point,
                        foreground: self.style.as_ref().is_none_or(|style| {
                            style.foreground(phase + progression.offset(point) * 16)
                        }),
                    });
                }
                if let Some(style) = &self.style {
                    phase = (phase + progression.length * 16) % style.period;
                }
            }
        }
        // Stable ordering preserves repeated raster operations at intersections.
        pixels.sort_by_key(|pixel| (pixel.point[1], pixel.point[0]));
        Ok(pixels)
    }
}
