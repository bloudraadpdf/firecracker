// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::memory::Memory;
use super::protocol::{DecodeError, Rect};
use super::transport::{AddressSpace, TransportError};

const MAX_BYTES: usize = 32 << 20;
const MAX_CHUNKS: usize = 4096;

pub(super) fn chunks(
    memory: &Memory,
    addresses: &AddressSpace,
    mut address: u64,
    length: usize,
) -> Result<Vec<u8>, TransportError> {
    if length > MAX_BYTES {
        return Err(DecodeError::Parameter.into());
    }
    let mut output = Vec::with_capacity(length);
    let mut previous = 0;
    for _ in 0..MAX_CHUNKS {
        let mut header = [0; 20];
        addresses.read(memory, address, &mut header)?;
        let size = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        let backlink = u64::from_le_bytes(header[4..12].try_into().unwrap());
        let next = u64::from_le_bytes(header[12..].try_into().unwrap());
        if backlink != previous || size > length - output.len() {
            return Err(DecodeError::Parameter.into());
        }
        let (region, offset) = addresses.resolve(address, 20 + size)?;
        let start = output.len();
        output.resize(start + size, 0);
        memory.read(region, offset + 20, &mut output[start..])?;
        if next == 0 {
            return if output.len() == length {
                Ok(output)
            } else {
                Err(DecodeError::Truncated.into())
            };
        }
        previous = address;
        address = next;
    }
    Err(DecodeError::Parameter.into())
}

#[derive(Debug)]
pub struct Bitmap {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
    has_alpha: bool,
}

impl Bitmap {
    pub(super) fn has_alpha(&self) -> bool {
        self.has_alpha
    }

    pub(super) fn validate_rect(&self, rect: Rect) -> Result<(), DecodeError> {
        let [left, top, right, bottom] = rect.bounds();
        if left < 0
            || top < 0
            || right > i32::try_from(self.width).unwrap()
            || bottom > i32::try_from(self.height).unwrap()
            || left == right
            || top == bottom
        {
            return Err(DecodeError::Parameter);
        }
        Ok(())
    }

    pub fn decode(
        memory: &Memory,
        addresses: &AddressSpace,
        address: u64,
    ) -> Result<Self, TransportError> {
        Self::decode_image(memory, addresses, address, false)
    }

    pub(super) fn decode_mask(
        memory: &Memory,
        addresses: &AddressSpace,
        address: u64,
    ) -> Result<Self, TransportError> {
        Self::decode_image(memory, addresses, address, true)
    }

    fn decode_image(
        memory: &Memory,
        addresses: &AddressSpace,
        address: u64,
        mask: bool,
    ) -> Result<Self, TransportError> {
        let mut header = [0; 48];
        addresses.read(memory, address, &mut header)?;
        let word = |offset| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
        let pointer = |offset| u64::from_le_bytes(header[offset..offset + 8].try_into().unwrap());
        let width = word(10);
        let height = word(14);
        if header[8] == 1 {
            if mask || header[9] & !3 != 0 {
                return Err(DecodeError::Parameter.into());
            }
            let length = usize::try_from(word(18)).map_err(|_| DecodeError::Parameter)?;
            let data = chunks(
                memory,
                addresses,
                address.checked_add(22).ok_or(DecodeError::Address)?,
                length,
            )?;
            let pixels =
                super::quic::decode(&data, width, height).map_err(|error| match error {
                    super::quic::Error::Truncated => DecodeError::Truncated,
                    super::quic::Error::Invalid => DecodeError::Parameter,
                })?;
            return Ok(Self {
                width,
                height,
                pixels,
                has_alpha: data[8] == 5,
            });
        }
        let format = header[18];
        let flags = header[19];
        let stride = word(28) as usize;
        let bits = match format {
            1 | 2 => 1,
            3 | 4 => 4,
            5 | 10 => 8,
            6 => 16,
            7 => 24,
            8 | 9 => 32,
            _ => return Err(DecodeError::Parameter.into()),
        };
        let length = stride.checked_mul(height as usize);
        if header[8] != 0
            || (mask && !matches!(format, 1 | 2))
            || header[9] & !3 != 0
            || flags & !7 != 0
            || width == 0
            || height == 0
            || u64::from(width) * u64::from(height) > (MAX_BYTES / 4) as u64
            || width != word(20)
            || height != word(24)
            || stride < (width as usize * bits).div_ceil(8)
            || length.is_none_or(|size| size > MAX_BYTES)
        {
            return Err(DecodeError::Parameter.into());
        }
        let length = length.unwrap();
        let palette = if mask {
            vec![0, 1]
        } else {
            Self::palette(memory, addresses, pointer(32), format)?
        };
        let data = if flags & 1 != 0 {
            let mut bytes = vec![0; length];
            addresses.read(memory, pointer(40), &mut bytes)?;
            bytes
        } else {
            chunks(memory, addresses, pointer(40), length)?
        };
        let mut pixels = Vec::with_capacity(width as usize * height as usize);
        for y in 0..height as usize {
            let row = if flags & 4 != 0 {
                y
            } else {
                height as usize - 1 - y
            };
            let row = &data[row * stride..(row + 1) * stride];
            for x in 0..width as usize {
                pixels.push(Self::pixel(row, x, format, &palette)?);
            }
        }
        Ok(Self {
            width,
            height,
            pixels,
            has_alpha: matches!(format, 9 | 10),
        })
    }

    pub(super) fn from_pixels(
        width: u32,
        height: u32,
        pixels: Vec<u32>,
    ) -> Result<Self, DecodeError> {
        if width == 0
            || height == 0
            || u64::from(width) * u64::from(height) != pixels.len() as u64
            || pixels.len() > MAX_BYTES / 4
        {
            return Err(DecodeError::Parameter);
        }
        Ok(Self {
            width,
            height,
            pixels,
            has_alpha: true,
        })
    }

    fn palette(
        memory: &Memory,
        addresses: &AddressSpace,
        address: u64,
        format: u8,
    ) -> Result<Vec<u32>, TransportError> {
        if format > 5 {
            return if address == 0 {
                Ok(Vec::new())
            } else {
                Err(DecodeError::Parameter.into())
            };
        }
        let mut header = [0; 10];
        addresses.read(memory, address, &mut header)?;
        let count = usize::from(u16::from_le_bytes(header[8..].try_into().unwrap()));
        if count == 0 || count > 256 {
            return Err(DecodeError::Parameter.into());
        }
        let (region, offset) = addresses.resolve(address, 10 + count * 4)?;
        let mut bytes = vec![0; count * 4];
        memory.read(region, offset + 10, &mut bytes)?;
        Ok(bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| u32::from_le_bytes(*pixel) | 0xff00_0000)
            .collect())
    }

    pub(super) fn pixel(
        row: &[u8],
        x: usize,
        format: u8,
        palette: &[u32],
    ) -> Result<u32, DecodeError> {
        let index = match format {
            1 => Some((row[x / 8] >> (x % 8)) & 1),
            2 => Some((row[x / 8] >> (7 - x % 8)) & 1),
            3 => Some((row[x / 2] >> ((x % 2) * 4)) & 15),
            4 => Some((row[x / 2] >> ((1 - x % 2) * 4)) & 15),
            5 => Some(row[x]),
            _ => None,
        };
        if let Some(index) = index {
            return palette
                .get(usize::from(index))
                .copied()
                .ok_or(DecodeError::Parameter);
        }
        Ok(match format {
            6 => {
                let value = u16::from_le_bytes(row[x * 2..x * 2 + 2].try_into().unwrap());
                let expand = |shift: u32| {
                    let value = u32::from((value >> shift) & 31);
                    (value << 3) | (value >> 2)
                };
                0xff00_0000 | (expand(10) << 16) | (expand(5) << 8) | expand(0)
            }
            7 => {
                let offset = x * 3;
                u32::from_le_bytes([row[offset], row[offset + 1], row[offset + 2], 255])
            }
            8 | 9 => {
                let value = u32::from_le_bytes(row[x * 4..x * 4 + 4].try_into().unwrap());
                if format == 8 {
                    value | 0xff00_0000
                } else {
                    value
                }
            }
            10 => (u32::from(row[x]) << 24) | 0x00ff_ffff,
            _ => return Err(DecodeError::Parameter),
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ScaleMode {
    Interpolate,
    Nearest,
}

impl ScaleMode {
    pub fn decode(value: u8) -> Result<Self, DecodeError> {
        match value {
            0 => Ok(Self::Interpolate),
            1 => Ok(Self::Nearest),
            _ => Err(DecodeError::Parameter),
        }
    }
}

pub(super) struct ScaledBitmap<'a> {
    bitmap: &'a Bitmap,
    source: [i64; 4],
    target: [i64; 4],
    mode: ScaleMode,
}

impl<'a> ScaledBitmap<'a> {
    pub fn new(
        bitmap: &'a Bitmap,
        source: Rect,
        target: Rect,
    ) -> Result<Option<Self>, DecodeError> {
        Self::with_mode(bitmap, source, target, ScaleMode::Nearest)
    }

    pub fn with_mode(
        bitmap: &'a Bitmap,
        source: Rect,
        target: Rect,
        mode: ScaleMode,
    ) -> Result<Option<Self>, DecodeError> {
        bitmap.validate_rect(source)?;
        let target = target.bounds().map(i64::from);
        if target[0] == target[2] || target[1] == target[3] {
            return Ok(None);
        }
        Ok(Some(Self {
            bitmap,
            source: source.bounds().map(i64::from),
            target,
            mode,
        }))
    }

    pub fn sample(&self, x: u32, y: u32) -> u32 {
        if matches!(self.mode, ScaleMode::Interpolate) {
            return self.interpolate(x, y);
        }
        let [left, top, right, bottom] = self.source;
        let [target_left, target_top, target_right, target_bottom] = self.target;
        let x = left
            + ((i64::from(x) - target_left) * 2 + 1) * (right - left)
                / ((target_right - target_left) * 2);
        let y = top
            + ((i64::from(y) - target_top) * 2 + 1) * (bottom - top)
                / ((target_bottom - target_top) * 2);
        self.bitmap.pixels
            [usize::try_from(y).unwrap() * self.bitmap.width as usize + usize::try_from(x).unwrap()]
    }

    fn interpolate(&self, x: u32, y: u32) -> u32 {
        let axis = |value: u32, source: (i64, i64), target: (i64, i64)| {
            let source = (i128::from(source.0), i128::from(source.1));
            let target = (i128::from(target.0), i128::from(target.1));
            let position = source.0 * 65536
                + ((i128::from(value) - target.0) * 2 + 1) * (source.1 - source.0) * 32768
                    / (target.1 - target.0)
                - 32768;
            let first = position.div_euclid(65536);
            (
                usize::try_from(first.clamp(source.0, source.1 - 1)).unwrap(),
                usize::try_from((first + 1).clamp(source.0, source.1 - 1)).unwrap(),
                u64::try_from(position.rem_euclid(65536)).unwrap(),
            )
        };
        let (x0, x1, fx) = axis(
            x,
            (self.source[0], self.source[2]),
            (self.target[0], self.target[2]),
        );
        let (y0, y1, fy) = axis(
            y,
            (self.source[1], self.source[3]),
            (self.target[1], self.target[3]),
        );
        let pixel = |x, y| self.bitmap.pixels[y * self.bitmap.width as usize + x];
        let mut output = 0;
        for shift in [0, 8, 16, 24] {
            let channel = |value| u64::from((value >> shift) & 255u32);
            let top = channel(pixel(x0, y0)) * (65536 - fx) + channel(pixel(x1, y0)) * fx;
            let bottom = channel(pixel(x0, y1)) * (65536 - fx) + channel(pixel(x1, y1)) * fx;
            let value = (top * (65536 - fy) + bottom * fy + (1 << 31)) >> 32;
            output |= u32::try_from(value).unwrap() << shift;
        }
        output
    }
}
