// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use vm_memory::{Bytes, GuestMemoryBackend, GuestMemoryError};

use super::protocol::{Command, DecodeError, HEADER_SIZE, Header, MAX_BACKING_ENTRIES, Queue};
use super::renderer::{Renderer, Reply};
use crate::devices::virtio::queue::{
    DescriptorChain, InvalidAvailIdx, Queue as VirtQueue, QueueError, VIRTQ_DESC_F_WRITE,
};
use crate::logger::warn;
use crate::vstate::memory::{GuestAddress, GuestMemoryMmap};

const MAX_REQUEST: usize = HEADER_SIZE + 8 + MAX_BACKING_ENTRIES * 16;
const DISPLAY_INFO_SIZE: usize = HEADER_SIZE + 16 * 24;

#[derive(Debug, thiserror::Error)]
pub enum GpuQueueError {
    #[error("invalid GPU descriptor chain")]
    InvalidBuffer,
    #[error("GPU request allocation failed")]
    OutOfMemory,
    #[error("GPU guest memory: {0}")]
    Memory(#[from] GuestMemoryError),
    #[error("GPU queue: {0}")]
    Queue(#[from] QueueError),
    #[error("GPU available ring: {0}")]
    Available(#[from] InvalidAvailIdx),
}

struct Buffers {
    request: Vec<u8>,
    writable: Vec<(GuestAddress, usize)>,
    capacity: usize,
}

impl Buffers {
    fn new(mem: &GuestMemoryMmap, chain: DescriptorChain) -> Result<Self, GpuQueueError> {
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
                    .ok_or(GpuQueueError::InvalidBuffer)?;
                writable.push((address, count));
            } else {
                length = length
                    .checked_add(count)
                    .ok_or(GpuQueueError::InvalidBuffer)?;
                if writing || length > MAX_REQUEST {
                    return Err(GpuQueueError::InvalidBuffer);
                }
                readable.push((address, count));
            }
        }
        if length < HEADER_SIZE {
            return Err(GpuQueueError::InvalidBuffer);
        }
        let mut request = Vec::new();
        request
            .try_reserve_exact(length)
            .map_err(|_| GpuQueueError::OutOfMemory)?;
        request.resize(length, 0);
        let mut offset = 0;
        for (address, count) in readable {
            mem.read_slice(&mut request[offset..offset + count], address)?;
            offset += count;
        }
        Ok(Self {
            request,
            writable,
            capacity,
        })
    }

    fn execute(
        self,
        renderer: &mut Renderer,
        mem: &GuestMemoryMmap,
        queue: Queue,
    ) -> Result<u32, GpuQueueError> {
        let parsed = Command::decode(queue, &self.request);
        let display_info = matches!(&parsed, Ok((_, Command::DisplayInfo)));
        let needed = if display_info {
            DISPLAY_INFO_SIZE
        } else {
            HEADER_SIZE
        };
        if self.capacity < needed && !(queue == Queue::Cursor && self.capacity == 0) {
            return Err(GpuQueueError::InvalidBuffer);
        }
        // Rejected requests must still acknowledge a requested fence.
        let header = Header {
            command_type: u32::from_le_bytes(self.request[..4].try_into().unwrap()),
            fence: (self.request[4] & 1 != 0)
                .then(|| u64::from_le_bytes(self.request[8..16].try_into().unwrap())),
        };
        let reply = match parsed {
            Ok((_, command)) => renderer
                .execute(command, mem)
                .map_err(|error| error.response_type()),
            Err(DecodeError::Unsupported(_)) => Err(0x1200),
            Err(_) => Err(0x1205),
        };
        if self.capacity == 0 {
            return Ok(0);
        }
        let response_type = match reply {
            Ok(Reply::NoData) => 0x1100,
            Ok(Reply::DisplayInfo { .. }) => 0x1101,
            Err(kind) => kind,
        };
        let mut response = header.response(response_type).to_vec();
        if let Ok(Reply::DisplayInfo { width, height }) = reply {
            response.resize(DISPLAY_INFO_SIZE, 0);
            response[32..36].copy_from_slice(&width.to_le_bytes());
            response[36..40].copy_from_slice(&height.to_le_bytes());
            response[40..44].copy_from_slice(&1u32.to_le_bytes());
        }
        let length = u32::try_from(response.len()).unwrap();
        let mut remaining = response.as_slice();
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

/// Process one bounded queue batch and report whether its interrupt is needed.
pub fn process_queue(
    queue: &mut VirtQueue,
    kind: Queue,
    renderer: &mut Renderer,
    mem: &GuestMemoryMmap,
) -> Result<bool, GpuQueueError> {
    let mut used = false;
    let result: Result<(), GpuQueueError> = (|| {
        for _ in 0..queue.size {
            let Some(chain) = queue.pop()? else {
                break;
            };
            let index = chain.index;
            let length = match Buffers::new(mem, chain)
                .and_then(|buffers| buffers.execute(renderer, mem, kind))
            {
                Ok(length) => length,
                Err(error) => {
                    warn!("gpu: rejected command buffer: {error}");
                    0
                }
            };
            queue.add_used(index, length)?;
            used = true;
        }
        Ok(())
    })();
    queue.advance_used_ring_idx();
    result?;
    Ok(used && queue.prepare_kick())
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
