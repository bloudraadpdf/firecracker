// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::ops::Range;

use super::memory::{
    COMMAND_RING, CURSOR_RING, Memory, RAM_BYTES, RAM_HEADER, RELEASE_RING, Region, VRAM_BYTES,
};
use super::protocol::{Command, CommandKind, DecodeError, MemorySlot, Ring, RingSize};

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error("QXL shared memory: {0}")]
    Memory(#[from] io::Error),
}

#[derive(Debug)]
pub struct AddressSpace {
    ram: Range<u64>,
    vram: Range<u64>,
    slots: [Option<(MemorySlot, Region)>; 2],
    generations: [u8; 2],
}

impl AddressSpace {
    pub fn new(ram: u64, vram: u64) -> Result<Self, DecodeError> {
        let range = |base: u64, length: usize| {
            let length = u64::try_from(length).unwrap();
            if !base.is_multiple_of(length) {
                return Err(DecodeError::Address);
            }
            Ok(base..base.checked_add(length).ok_or(DecodeError::Address)?)
        };
        let ram = range(ram, RAM_BYTES)?;
        let vram = range(vram, VRAM_BYTES)?;
        if ram.start < vram.end && vram.start < ram.end {
            return Err(DecodeError::Address);
        }
        Ok(Self {
            ram,
            vram,
            slots: [None; 2],
            generations: [0; 2],
        })
    }

    fn index(id: u8) -> Result<usize, DecodeError> {
        match id {
            1 | 2 => Ok(usize::from(id - 1)),
            _ => Err(DecodeError::Slot),
        }
    }

    pub fn add(&mut self, id: u8, range: Range<u64>) -> Result<u8, DecodeError> {
        let index = Self::index(id)?;
        if self.slots[index].is_some() {
            return Err(DecodeError::Slot);
        }
        let ram = self.ram.start..self.ram.start + u64::try_from(RAM_HEADER).unwrap();
        let (region, backing) = if range.start >= ram.start && range.end <= ram.end {
            (Region::Ram, ram)
        } else {
            (Region::Vram, self.vram.clone())
        };
        let generation = self.generations[index].wrapping_add(1);
        let slot = MemorySlot::new(id, generation, range, backing)?;
        self.slots[index] = Some((slot, region));
        self.generations[index] = generation;
        Ok(generation)
    }

    pub fn remove(&mut self, id: u8) -> Result<(), DecodeError> {
        self.slots[Self::index(id)?] = None;
        Ok(())
    }

    pub fn reset(&mut self) {
        self.slots.fill(None);
    }

    pub fn resolve(&self, address: u64, length: usize) -> Result<(Region, usize), DecodeError> {
        let id = u8::try_from(address >> 56).unwrap();
        let (slot, region) = self.slots[Self::index(id)?].ok_or(DecodeError::Slot)?;
        let range = slot.resolve(address, u64::try_from(length).unwrap())?;
        let base = match region {
            Region::Ram => self.ram.start,
            Region::Vram => self.vram.start,
            Region::Rom => unreachable!(),
        };
        Ok((region, usize::try_from(range.start - base).unwrap()))
    }

    pub fn read(
        &self,
        memory: &Memory,
        address: u64,
        output: &mut [u8],
    ) -> Result<(), TransportError> {
        let (region, offset) = self.resolve(address, output.len())?;
        Ok(memory.read(region, offset, output)?)
    }

    pub fn write(&self, memory: &Memory, address: u64, input: &[u8]) -> Result<(), TransportError> {
        let (region, offset) = self.resolve(address, input.len())?;
        Ok(memory.write(region, offset, input)?)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Queue {
    Commands,
    Cursor,
}

impl Queue {
    fn index(self) -> usize {
        match self {
            Self::Commands => 0,
            Self::Cursor => 1,
        }
    }

    fn offset(self) -> usize {
        match self {
            Self::Commands => COMMAND_RING,
            Self::Cursor => CURSOR_RING,
        }
    }
}

#[derive(Debug, Default)]
pub struct Queues {
    consumers: [u32; 2],
    release_producer: u32,
    release_consumer: u32,
}

impl Queues {
    fn release_room(&mut self, memory: &Memory) -> Result<bool, TransportError> {
        if self.release_producer.wrapping_sub(self.release_consumer) == 8 {
            memory.store(RELEASE_RING + 16, self.release_consumer.wrapping_add(1))?;
            std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
        }
        let consumer = memory.load(RELEASE_RING + 12)?;
        let outstanding = self.release_producer.wrapping_sub(self.release_consumer);
        if memory.load(RELEASE_RING)? != 8
            || memory.load(RELEASE_RING + 4)? != self.release_producer
            || consumer.wrapping_sub(self.release_consumer) > outstanding
        {
            return Err(DecodeError::Ring.into());
        }
        self.release_consumer = consumer;
        Ok(self.release_producer.wrapping_sub(consumer) < 8)
    }

    pub fn drain(
        &mut self,
        memory: &Memory,
        addresses: &AddressSpace,
        queue: Queue,
        mut execute: impl FnMut(Command) -> Result<(), TransportError>,
    ) -> Result<u32, TransportError> {
        let offset = queue.offset();
        let index = queue.index();
        let mut completed = 0;
        // A producer can refill the ring while it is drained. Bound each dispatch.
        for _ in 0..32 {
            let mut header = [0; Ring::HEADER_SIZE];
            let producer = memory.load(offset + 4)?;
            memory.read(Region::Ram, offset, &mut header)?;
            header[4..8].copy_from_slice(&producer.to_le_bytes());
            let mut ring = Ring::decode(&header, RingSize::Commands, self.consumers[index])?;
            let Some((item, notify)) = ring.pop() else {
                memory.store(offset + 8, self.consumers[index].wrapping_add(1))?;
                // Arm the notification before checking for a concurrent producer.
                std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
                if memory.load(offset + 4)? != self.consumers[index] {
                    continue;
                }
                break;
            };
            let mut bytes = [0; Command::SIZE];
            memory.read(
                Region::Ram,
                offset + Ring::HEADER_SIZE + usize::try_from(item).unwrap() * Command::SIZE,
                &mut bytes,
            )?;
            let command = Command::decode(&bytes)?;
            if command.kind != CommandKind::Nop {
                if matches!(command.kind, CommandKind::Cursor) != matches!(queue, Queue::Cursor) {
                    return Err(DecodeError::Parameter.into());
                }
                if !self.release_room(memory)? {
                    break;
                }
                let mut release = [0; 8];
                addresses.read(memory, command.address, &mut release)?;
                if release == [0; 8] {
                    return Err(DecodeError::Parameter.into());
                }
                execute(command)?;
                addresses.write(memory, command.address, &[0; 8])?;
                memory.write(
                    Region::Ram,
                    RELEASE_RING
                        + Ring::HEADER_SIZE
                        + usize::try_from(self.release_producer & 7).unwrap() * 8,
                    &release,
                )?;
                self.release_producer = self.release_producer.wrapping_add(1);
                memory.store(RELEASE_RING + 4, self.release_producer)?;
                if memory.load(RELEASE_RING + 8)? == self.release_producer {
                    memory.interrupt(1)?;
                }
            }
            self.consumers[index] = ring.consumer();
            memory.store(offset + 12, ring.consumer())?;
            if notify {
                memory.interrupt(1 << index)?;
            }
            completed += 1;
        }
        memory.store(offset + 8, self.consumers[index].wrapping_add(1))?;
        Ok(completed)
    }
}
