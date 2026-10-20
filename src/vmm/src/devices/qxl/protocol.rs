// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Packed little-endian QXL device ABI, as published by spice-protocol 0.12.14.

use std::ops::Range;

const ADDRESS_MASK: u64 = (1 << 48) - 1;
const MAX_SLOT_BYTES: u64 = 64 << 20;
const MAX_SURFACE_BYTES: u64 = 32 << 20;
const MAX_DIMENSION: u32 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("truncated QXL structure")]
    Truncated,
    #[error("invalid QXL parameter")]
    Parameter,
    #[error("invalid QXL memory slot or generation")]
    Slot,
    #[error("QXL pointer exceeds its memory slot")]
    Address,
    #[error("invalid QXL ring state")]
    Ring,
    #[error("unsupported QXL command {0}")]
    Command(u32),
}

pub(super) fn bytes<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N], DecodeError> {
    data.get(offset..)
        .and_then(|data| data.get(..N))
        .ok_or(DecodeError::Truncated)?
        .try_into()
        .map_err(|_| DecodeError::Truncated)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandKind {
    Nop,
    Draw,
    Update,
    Cursor,
    Message,
    Surface,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub address: u64,
    pub kind: CommandKind,
}

impl Command {
    pub const SIZE: usize = 16;

    pub fn decode(data: &[u8]) -> Result<Self, DecodeError> {
        bytes::<16>(data, 0)?;
        let opcode = u32::from_le_bytes(bytes(data, 8)?);
        let kind = match opcode {
            0 => CommandKind::Nop,
            1 => CommandKind::Draw,
            2 => CommandKind::Update,
            3 => CommandKind::Cursor,
            4 => CommandKind::Message,
            5 => CommandKind::Surface,
            other => return Err(DecodeError::Command(other)),
        };
        Ok(Self {
            address: u64::from_le_bytes(bytes(data, 0)?),
            kind,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemorySlot {
    id: u8,
    generation: u8,
    start: u64,
    end: u64,
}

impl MemorySlot {
    pub(super) fn range(&self) -> Range<u64> {
        self.start..self.end
    }

    pub fn new(
        id: u8,
        generation: u8,
        range: Range<u64>,
        backing: Range<u64>,
    ) -> Result<Self, DecodeError> {
        if id == 0
            || range.start >= range.end
            || range.start < backing.start
            || range.end > backing.end
            || range.end - range.start > MAX_SLOT_BYTES
        {
            return Err(DecodeError::Slot);
        }
        Ok(Self {
            id,
            generation,
            start: range.start,
            end: range.end,
        })
    }

    pub fn resolve(&self, address: u64, length: u64) -> Result<Range<u64>, DecodeError> {
        if address >> 56 != u64::from(self.id)
            || (address >> 48) & 255 != u64::from(self.generation)
        {
            return Err(DecodeError::Slot);
        }
        let offset = address & ADDRESS_MASK;
        if offset
            .checked_add(length)
            .is_none_or(|end| end > self.end - self.start)
        {
            return Err(DecodeError::Address);
        }
        let start = self.start + offset;
        Ok(start..start + length)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingSize {
    Commands,
    Releases,
}

impl RingSize {
    pub fn capacity(self) -> u32 {
        match self {
            Self::Commands => 32,
            Self::Releases => 8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ring {
    capacity: u32,
    producer: u32,
    consumer: u32,
    notify_consumer: u32,
}

impl Ring {
    pub const HEADER_SIZE: usize = 20;

    pub fn decode(data: &[u8], size: RingSize, consumer: u32) -> Result<Self, DecodeError> {
        bytes::<20>(data, 0)?;
        let capacity = size.capacity();
        let producer = u32::from_le_bytes(bytes(data, 4)?);
        let shared_consumer = u32::from_le_bytes(bytes(data, 12)?);
        if u32::from_le_bytes(bytes(data, 0)?) != capacity
            || shared_consumer != consumer
            || producer.wrapping_sub(consumer) > capacity
        {
            return Err(DecodeError::Ring);
        }
        Ok(Self {
            capacity,
            producer,
            consumer,
            notify_consumer: u32::from_le_bytes(bytes(data, 16)?),
        })
    }

    pub fn pending(&self) -> u32 {
        self.producer.wrapping_sub(self.consumer)
    }

    pub fn pop(&mut self) -> Option<(u32, bool)> {
        if self.pending() == 0 {
            return None;
        }
        let index = self.consumer & (self.capacity - 1);
        self.consumer = self.consumer.wrapping_add(1);
        Some((index, self.consumer == self.notify_consumer))
    }

    pub fn consumer(&self) -> u32 {
        self.consumer
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Rect {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Result<Self, DecodeError> {
        if left > right || top > bottom {
            return Err(DecodeError::Parameter);
        }
        Ok(Self {
            left,
            top,
            right,
            bottom,
        })
    }

    pub fn bounds(&self) -> [i32; 4] {
        [self.left, self.top, self.right, self.bottom]
    }

    pub fn decode(data: &[u8]) -> Result<Self, DecodeError> {
        Self::new(
            i32::from_le_bytes(bytes(data, 4)?),
            i32::from_le_bytes(bytes(data, 0)?),
            i32::from_le_bytes(bytes(data, 12)?),
            i32::from_le_bytes(bytes(data, 8)?),
        )
    }

    pub fn clipped(&self, width: u32, height: u32) -> Result<[u32; 4], DecodeError> {
        if width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(DecodeError::Parameter);
        }
        let width = i32::try_from(width).unwrap();
        let height = i32::try_from(height).unwrap();
        Ok([
            u32::try_from(self.left.clamp(0, width)).unwrap(),
            u32::try_from(self.top.clamp(0, height)).unwrap(),
            u32::try_from(self.right.clamp(0, width)).unwrap(),
            u32::try_from(self.bottom.clamp(0, height)).unwrap(),
        ])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceFormat {
    Xrgb8888,
    Argb8888,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrimarySurface {
    width: u32,
    height: u32,
    stride: i32,
    format: SurfaceFormat,
    address: u64,
    keep_data: bool,
    length: u64,
}

impl PrimarySurface {
    pub const SIZE: usize = 40;

    pub(super) fn encode(&self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        for (offset, value) in [
            (0, self.width),
            (4, self.height),
            (
                12,
                match self.format {
                    SurfaceFormat::Xrgb8888 => 32,
                    SurfaceFormat::Argb8888 => 96,
                },
            ),
            (24, u32::from(self.keep_data)),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[8..12].copy_from_slice(&self.stride.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.address.to_le_bytes());
        bytes
    }

    pub fn decode(data: &[u8]) -> Result<Self, DecodeError> {
        bytes::<40>(data, 0)?;
        let width = u32::from_le_bytes(bytes(data, 0)?);
        let height = u32::from_le_bytes(bytes(data, 4)?);
        let stride = i32::from_le_bytes(bytes(data, 8)?);
        let format = match u32::from_le_bytes(bytes(data, 12)?) {
            32 => SurfaceFormat::Xrgb8888,
            96 => SurfaceFormat::Argb8888,
            _ => return Err(DecodeError::Parameter),
        };
        let flags = u32::from_le_bytes(bytes(data, 24)?);
        let row_bytes = u64::from(stride.unsigned_abs());
        let length = row_bytes * u64::from(height);
        if width == 0
            || height == 0
            || width > MAX_DIMENSION
            || height > MAX_DIMENSION
            || row_bytes < u64::from(width) * 4
            || length > MAX_SURFACE_BYTES
            || flags & !1 != 0
            || u32::from_le_bytes(bytes(data, 28)?) != 0
        {
            return Err(DecodeError::Parameter);
        }
        Ok(Self {
            width,
            height,
            stride,
            format,
            address: u64::from_le_bytes(bytes(data, 32)?),
            keep_data: flags & 1 != 0,
            length,
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn stride(&self) -> i32 {
        self.stride
    }

    pub fn format(&self) -> SurfaceFormat {
        self.format
    }

    pub fn address(&self) -> u64 {
        self.address
    }

    pub fn keep_data(&self) -> bool {
        self.keep_data
    }

    pub fn length(&self) -> u64 {
        self.length
    }
}
