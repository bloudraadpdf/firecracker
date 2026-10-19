// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! VIRTIO 1.3, section 5.7.6: little-endian control and cursor requests.

use std::num::NonZeroU32;

pub const HEADER_SIZE: usize = 24;
pub const MAX_BACKING_ENTRIES: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("truncated GPU command")]
    Truncated,
    #[error("unsupported GPU command {0:#x}")]
    Unsupported(u32),
    #[error("invalid GPU command parameter")]
    InvalidParameter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Queue {
    Control,
    Cursor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub command_type: u32,
    pub fence: Option<u64>,
}

impl Header {
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let reader = Reader(bytes.get(..HEADER_SIZE).ok_or(DecodeError::Truncated)?);
        let flags = reader.u32(4)?;
        if flags & !1 != 0 || reader.u32(16)? != 0 {
            return Err(DecodeError::InvalidParameter);
        }
        Ok(Self {
            command_type: reader.u32(0)?,
            fence: (flags & 1 != 0).then_some(reader.u64(8)?),
        })
    }

    pub fn response(self, response_type: u32) -> [u8; HEADER_SIZE] {
        let mut bytes = [0; HEADER_SIZE];
        bytes[..4].copy_from_slice(&response_type.to_le_bytes());
        if let Some(fence) = self.fence {
            bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
            bytes[8..16].copy_from_slice(&fence.to_le_bytes());
        }
        bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceId(NonZeroU32);

impl ResourceId {
    pub fn new(value: u32) -> Result<Self, DecodeError> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or(DecodeError::InvalidParameter)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackingEntry {
    pub address: u64,
    pub length: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backing<'a>(&'a [u8]);

impl Backing<'_> {
    pub fn entries(&self) -> impl ExactSizeIterator<Item = BackingEntry> + '_ {
        self.0.as_chunks::<16>().0.iter().map(|entry| BackingEntry {
            address: u64::from_le_bytes(entry[..8].try_into().unwrap()),
            length: u32::from_le_bytes(entry[8..12].try_into().unwrap()),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<'a> {
    DisplayInfo,
    Create2d {
        id: ResourceId,
        format: u32,
        width: u32,
        height: u32,
    },
    Unref(ResourceId),
    Scanout {
        rect: Rect,
        scanout: u32,
        resource: Option<ResourceId>,
    },
    Flush {
        rect: Rect,
        id: ResourceId,
    },
    Transfer {
        rect: Rect,
        offset: u64,
        id: ResourceId,
    },
    Attach {
        id: ResourceId,
        backing: Backing<'a>,
    },
    Detach(ResourceId),
    UpdateCursor {
        scanout: u32,
        x: u32,
        y: u32,
        resource: Option<ResourceId>,
        hot_x: u32,
        hot_y: u32,
    },
    MoveCursor {
        scanout: u32,
        x: u32,
        y: u32,
    },
}

impl<'a> Command<'a> {
    pub fn decode(queue: Queue, bytes: &'a [u8]) -> Result<(Header, Self), DecodeError> {
        let header = Header::decode(bytes)?;
        let reader = Reader(&bytes[HEADER_SIZE..]);
        let cursor = matches!(header.command_type, 0x0300 | 0x0301);
        if cursor != (queue == Queue::Cursor) {
            return Err(DecodeError::InvalidParameter);
        }
        let command = match header.command_type {
            0x0100 => Self::DisplayInfo,
            0x0101 => Self::Create2d {
                id: reader.id(0)?,
                format: reader.u32(4)?,
                width: reader.u32(8)?,
                height: reader.u32(12)?,
            },
            0x0102 | 0x0107 => {
                reader.require(8)?;
                let id = reader.id(0)?;
                if header.command_type == 0x0102 {
                    Self::Unref(id)
                } else {
                    Self::Detach(id)
                }
            }
            0x0103 => Self::Scanout {
                rect: reader.rect(0)?,
                scanout: reader.u32(16)?,
                resource: reader.optional_id(20)?,
            },
            0x0104 => {
                reader.require(24)?;
                Self::Flush {
                    rect: reader.rect(0)?,
                    id: reader.id(16)?,
                }
            }
            0x0105 => {
                reader.require(32)?;
                Self::Transfer {
                    rect: reader.rect(0)?,
                    offset: reader.u64(16)?,
                    id: reader.id(24)?,
                }
            }
            0x0106 => {
                let id = reader.id(0)?;
                let count =
                    usize::try_from(reader.u32(4)?).map_err(|_| DecodeError::InvalidParameter)?;
                if count == 0 || count > MAX_BACKING_ENTRIES {
                    return Err(DecodeError::InvalidParameter);
                }
                let entries = reader
                    .0
                    .get(8..8 + count * 16)
                    .ok_or(DecodeError::Truncated)?;
                Self::Attach {
                    id,
                    backing: Backing(entries),
                }
            }
            0x0300 | 0x0301 => {
                reader.require(32)?;
                let (scanout, x, y) = (reader.u32(0)?, reader.u32(4)?, reader.u32(8)?);
                if header.command_type == 0x0300 {
                    Self::UpdateCursor {
                        scanout,
                        x,
                        y,
                        resource: reader.optional_id(16)?,
                        hot_x: reader.u32(20)?,
                        hot_y: reader.u32(24)?,
                    }
                } else {
                    Self::MoveCursor { scanout, x, y }
                }
            }
            command => return Err(DecodeError::Unsupported(command)),
        };
        Ok((header, command))
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn require(&self, length: usize) -> Result<(), DecodeError> {
        self.0
            .get(..length)
            .map(|_| ())
            .ok_or(DecodeError::Truncated)
    }

    fn u32(&self, offset: usize) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(
            self.0
                .get(offset..offset + 4)
                .ok_or(DecodeError::Truncated)?
                .try_into()
                .unwrap(),
        ))
    }

    fn u64(&self, offset: usize) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(
            self.0
                .get(offset..offset + 8)
                .ok_or(DecodeError::Truncated)?
                .try_into()
                .unwrap(),
        ))
    }

    fn id(&self, offset: usize) -> Result<ResourceId, DecodeError> {
        ResourceId::new(self.u32(offset)?)
    }

    fn optional_id(&self, offset: usize) -> Result<Option<ResourceId>, DecodeError> {
        Ok(NonZeroU32::new(self.u32(offset)?).map(ResourceId))
    }

    fn rect(&self, offset: usize) -> Result<Rect, DecodeError> {
        Ok(Rect {
            x: self.u32(offset)?,
            y: self.u32(offset + 4)?,
            width: self.u32(offset + 8)?,
            height: self.u32(offset + 12)?,
        })
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
