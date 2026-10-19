// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::bitmap::{Bitmap, chunks};
use super::memory::Memory;
use super::protocol::{DecodeError, bytes};
use super::raster::alpha_over;
use super::transport::{AddressSpace, TransportError};
use crate::devices::display::Frame;

#[derive(Debug)]
enum Pixels {
    Alpha(Vec<u32>),
    Masked(Vec<[u32; 2]>),
}

#[derive(Debug)]
pub struct Shape {
    width: u16,
    height: u16,
    hotspot: [u16; 2],
    pixels: Pixels,
}

impl Shape {
    pub fn decode(
        memory: &Memory,
        addresses: &AddressSpace,
        address: u64,
    ) -> Result<Self, TransportError> {
        let mut header = [0; 22];
        addresses.read(memory, address, &mut header)?;
        let word = |offset| u16::from_le_bytes(bytes(&header, offset).unwrap());
        let kind = word(8);
        let width = word(10);
        let height = word(12);
        let hotspot = [word(14), word(16)];
        let length = u32::from_le_bytes(bytes(&header, 18)?) as usize;
        if kind > 6 || width > 256 || height > 256 {
            return Err(DecodeError::Parameter.into());
        }
        if width == 0 && height == 0 && hotspot == [0, 0] && length == 0 {
            // The driver's transparent cursor leaves the unused chunk links unset.
            return Ok(Self {
                width,
                height,
                hotspot,
                pixels: Pixels::Alpha(Vec::new()),
            });
        }
        if width == 0 || height == 0 || hotspot[0] >= width || hotspot[1] >= height {
            return Err(DecodeError::Parameter.into());
        }
        let (width_usize, height_usize) = (usize::from(width), usize::from(height));
        let bits = [32, 1, 4, 8, 16, 24, 32][usize::from(kind)];
        let stride = (width_usize * bits).div_ceil(8);
        let body = stride * height_usize;
        let palette_size = match kind {
            2 => 16 * 4,
            3 => 256 * 4,
            _ => 0,
        };
        let mask_stride = width_usize.div_ceil(8);
        let mask_size = if kind == 0 {
            0
        } else {
            mask_stride * height_usize
        };
        if length != body + palette_size + mask_size {
            return Err(DecodeError::Parameter.into());
        }
        addresses.resolve(address, 42)?;
        let data = chunks(
            memory,
            addresses,
            address.checked_add(22).ok_or(DecodeError::Address)?,
            length,
        )?;
        let palette: Vec<_> = data[body..body + palette_size]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|color| u32::from_le_bytes(*color) | 0xff00_0000)
            .collect();
        let mut alpha = Vec::new();
        let mut masked = Vec::new();
        let bit = |offset, x: usize, y: usize| {
            data[offset + y * mask_stride + x / 8] & (0x80 >> (x % 8)) != 0
        };
        let format = [9, 2, 4, 5, 6, 7, 8][usize::from(kind)];
        for y in 0..height_usize {
            let row = &data[y * stride..(y + 1) * stride];
            for x in 0..width_usize {
                if kind == 1 {
                    let word = |set| if set { 0x00ff_ffff } else { 0 };
                    masked.push([word(bit(0, x, y)), word(bit(body, x, y))]);
                } else {
                    let color = Bitmap::pixel(row, x, format, &palette)?;
                    if kind == 0 {
                        alpha.push(color);
                    } else {
                        let color = color & 0x00ff_ffff;
                        let and =
                            if bit(body + palette_size, x, y) && matches!(color, 0 | 0x00ff_ffff) {
                                0x00ff_ffff
                            } else {
                                0
                            };
                        masked.push([and, color]);
                    }
                }
            }
        }
        Ok(Self {
            width,
            height,
            hotspot,
            pixels: if kind == 0 {
                Pixels::Alpha(alpha)
            } else {
                Pixels::Masked(masked)
            },
        })
    }

    pub fn draw(&self, frame: &mut Frame, position: [i16; 2]) {
        for y in 0..self.height {
            let target_y = i32::from(position[1]) - i32::from(self.hotspot[1]) + i32::from(y);
            if !(0..i32::from(frame.height)).contains(&target_y) {
                continue;
            }
            for x in 0..self.width {
                let target_x = i32::from(position[0]) - i32::from(self.hotspot[0]) + i32::from(x);
                if !(0..i32::from(frame.width)).contains(&target_x) {
                    continue;
                }
                let source = usize::from(y) * usize::from(self.width) + usize::from(x);
                let target = usize::try_from(target_y).unwrap() * usize::from(frame.width)
                    + usize::try_from(target_x).unwrap();
                let pixel = &mut frame.pixels[target];
                *pixel = match &self.pixels {
                    Pixels::Alpha(colors) => alpha_over(colors[source], *pixel, 255, false),
                    Pixels::Masked(masks) => (*pixel & masks[source][0]) ^ masks[source][1],
                } & 0x00ff_ffff;
            }
        }
    }
}

#[derive(Debug)]
pub enum Command {
    Set {
        position: [i16; 2],
        visible: bool,
        shape: Shape,
    },
    Move([i16; 2]),
    Hide,
    Trail {
        length: u16,
        frequency: u16,
    },
}

impl Command {
    pub fn decode(
        memory: &Memory,
        addresses: &AddressSpace,
        address: u64,
    ) -> Result<Self, TransportError> {
        let mut header = [0; 9];
        addresses.read(memory, address, &mut header)?;
        let read = |output: &mut [u8]| -> Result<(), TransportError> {
            addresses.resolve(address, 9 + output.len())?;
            addresses.read(
                memory,
                address.checked_add(9).ok_or(DecodeError::Address)?,
                output,
            )
        };
        let position = |data: &[u8]| {
            [
                i16::from_le_bytes(bytes(data, 0).unwrap()),
                i16::from_le_bytes(bytes(data, 2).unwrap()),
            ]
        };
        Ok(match header[8] {
            0 => {
                let mut data = [0; 13];
                read(&mut data)?;
                if data[4] > 1 {
                    return Err(DecodeError::Parameter.into());
                }
                Self::Set {
                    position: position(&data),
                    visible: data[4] != 0,
                    shape: Shape::decode(memory, addresses, u64::from_le_bytes(bytes(&data, 5)?))?,
                }
            }
            1 => {
                let mut data = [0; 4];
                read(&mut data)?;
                Self::Move(position(&data))
            }
            2 => Self::Hide,
            3 => {
                let mut data = [0; 4];
                read(&mut data)?;
                Self::Trail {
                    length: u16::from_le_bytes(bytes(&data, 0)?),
                    frequency: u16::from_le_bytes(bytes(&data, 2)?),
                }
            }
            other => return Err(DecodeError::Command(u32::from(other)).into()),
        })
    }
}
