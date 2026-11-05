// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::protocol::{Command, DecodeError, HEADER_SIZE, Header, MAX_BACKING_ENTRIES, Queue};
use super::renderer::{Renderer, Reply};
pub use crate::devices::virtio::buffers::BufferError as GpuQueueError;
use crate::devices::virtio::buffers::Buffers;
use crate::devices::virtio::queue::Queue as VirtQueue;
use crate::logger::warn;
use crate::vstate::memory::GuestMemoryMmap;

const MAX_REQUEST: usize = HEADER_SIZE + 8 + MAX_BACKING_ENTRIES * 16;
const DISPLAY_INFO_SIZE: usize = HEADER_SIZE + 16 * 24;

impl Buffers {
    fn execute(
        self,
        renderer: &mut Renderer,
        mem: &GuestMemoryMmap,
        queue: Queue,
    ) -> Result<u32, GpuQueueError> {
        if self.request.len() < HEADER_SIZE {
            return Err(GpuQueueError::InvalidBuffer);
        }
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
        self.respond(mem, &response)
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
        for _ in 0..queue.config.size {
            let Some(chain) = queue.pop()? else {
                break;
            };
            let index = chain.index;
            let length = match Buffers::new(mem, chain, MAX_REQUEST, usize::MAX)
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
