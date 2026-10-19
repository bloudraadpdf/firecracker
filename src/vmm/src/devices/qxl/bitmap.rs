// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::memory::Memory;
use super::protocol::DecodeError;
use super::transport::{AddressSpace, TransportError};

const MAX_BYTES: usize = 32 << 20;
const MAX_DIMENSION: u32 = 4096;
const MAX_CHUNKS: usize = 4096;

fn chunks(
    memory: &Memory,
    addresses: &AddressSpace,
    mut address: u64,
    length: usize,
) -> Result<Vec<u8>, TransportError> {
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
}

impl Bitmap {
    pub fn decode(
        memory: &Memory,
        addresses: &AddressSpace,
        address: u64,
    ) -> Result<Self, TransportError> {
        let mut header = [0; 48];
        addresses.read(memory, address, &mut header)?;
        let word = |offset| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
        let pointer = |offset| u64::from_le_bytes(header[offset..offset + 8].try_into().unwrap());
        let width = word(10);
        let height = word(14);
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
            || header[9] & !3 != 0
            || flags & !7 != 0
            || width == 0
            || height == 0
            || width > MAX_DIMENSION
            || height > MAX_DIMENSION
            || width != word(20)
            || height != word(24)
            || stride < (width as usize * bits).div_ceil(8)
            || length.is_none_or(|size| size > MAX_BYTES)
            || width as usize * height as usize > MAX_BYTES / 4
        {
            return Err(DecodeError::Parameter.into());
        }
        let length = length.unwrap();
        let palette = Self::palette(memory, addresses, pointer(32), format)?;
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

    fn pixel(row: &[u8], x: usize, format: u8, palette: &[u32]) -> Result<u32, DecodeError> {
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
