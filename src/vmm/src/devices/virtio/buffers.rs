// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use vm_memory::{Bytes, GuestMemoryBackend, GuestMemoryError};

use super::queue::{DescriptorChain, InvalidAvailIdx, QueueError, VIRTQ_DESC_F_WRITE};
use crate::vstate::memory::{GuestAddress, GuestMemoryMmap};

#[derive(Debug, thiserror::Error)]
pub enum BufferError {
    #[error("invalid descriptor buffer")]
    InvalidBuffer,
    #[error("request allocation failed")]
    OutOfMemory,
    #[error("guest memory: {0}")]
    Memory(#[from] GuestMemoryError),
    #[error("queue: {0}")]
    Queue(#[from] QueueError),
    #[error("available ring: {0}")]
    Available(#[from] InvalidAvailIdx),
}

pub(crate) struct Buffers {
    pub request: Vec<u8>,
    pub capacity: usize,
    writable: Vec<(GuestAddress, usize)>,
}

impl Buffers {
    pub fn new(
        mem: &GuestMemoryMmap,
        chain: DescriptorChain,
        request_limit: usize,
        response_limit: usize,
    ) -> Result<Self, BufferError> {
        let mut readable = Vec::new();
        let mut writable = Vec::new();
        let mut length = 0usize;
        let mut capacity = 0usize;
        let mut writing = false;
        for descriptor in chain.checked_iter(mem) {
            let descriptor = descriptor?;
            let address = GuestAddress(descriptor.addr);
            let count = descriptor.len as usize;
            mem.get_slice(address, count)?;
            if descriptor.flags & VIRTQ_DESC_F_WRITE != 0 {
                writing = true;
                capacity = capacity
                    .checked_add(count)
                    .ok_or(BufferError::InvalidBuffer)?;
                if capacity > response_limit {
                    return Err(BufferError::InvalidBuffer);
                }
                writable.push((address, count));
            } else {
                length = length
                    .checked_add(count)
                    .ok_or(BufferError::InvalidBuffer)?;
                if writing || length > request_limit {
                    return Err(BufferError::InvalidBuffer);
                }
                readable.push((address, count));
            }
        }
        let mut request = Vec::new();
        request
            .try_reserve_exact(length)
            .map_err(|_| BufferError::OutOfMemory)?;
        request.resize(length, 0);
        let mut offset = 0;
        for (address, count) in readable {
            mem.read_slice(&mut request[offset..offset + count], address)?;
            offset += count;
        }
        Ok(Self {
            request,
            capacity,
            writable,
        })
    }

    pub fn respond(self, mem: &GuestMemoryMmap, bytes: &[u8]) -> Result<u32, BufferError> {
        if bytes.len() > self.capacity {
            return Err(BufferError::InvalidBuffer);
        }
        let length = u32::try_from(bytes.len()).map_err(|_| BufferError::InvalidBuffer)?;
        let mut remaining = bytes;
        for (address, count) in self.writable {
            let count = count.min(remaining.len());
            mem.write_slice(&remaining[..count], address)?;
            remaining = &remaining[count..];
            if remaining.is_empty() {
                break;
            }
        }
        Ok(length)
    }
}
