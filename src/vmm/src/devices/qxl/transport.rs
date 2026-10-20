// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::ops::Range;

use super::memory::{
    COMMAND_RING, CURSOR_RING, Memory, RAM_BYTES, RAM_HEADER, RELEASE_RING, Region, VRAM_BYTES,
};
use super::protocol::{Command, CommandKind, DecodeError, MemorySlot, Ring, RingSize};
use super::state::{AddressSpaceState, QueueState, ReleaseState, SlotRegion, SlotState};

const MAX_DEFERRED_RELEASES: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("QXL operation is waiting for the guest to consume releases")]
    Busy,
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

    pub fn relocate(&mut self, ram: u64, vram: u64) -> Result<(), DecodeError> {
        *self = Self::restore(&self.save(), ram, vram)?;
        Ok(())
    }

    pub(super) fn save(&self) -> AddressSpaceState {
        AddressSpaceState {
            generations: self.generations,
            slots: self.slots.map(|slot| {
                slot.map(|(slot, region)| {
                    let (region, base) = match region {
                        Region::Ram => (SlotRegion::Ram, self.ram.start),
                        Region::Vram => (SlotRegion::Vram, self.vram.start),
                        Region::Rom => unreachable!(),
                    };
                    let range = slot.range();
                    SlotState {
                        region,
                        range: range.start - base..range.end - base,
                    }
                })
            }),
        }
    }

    pub(super) fn restore(
        state: &AddressSpaceState,
        ram: u64,
        vram: u64,
    ) -> Result<Self, DecodeError> {
        let mut next = Self::new(ram, vram)?;
        next.generations = state.generations;
        for (index, saved) in state.slots.iter().enumerate() {
            if let Some(saved) = saved {
                let (region, backing) = match saved.region {
                    SlotRegion::Ram => (Region::Ram, ram..ram + u64::try_from(RAM_HEADER).unwrap()),
                    SlotRegion::Vram => (Region::Vram, next.vram.clone()),
                };
                let address = |offset| {
                    backing
                        .start
                        .checked_add(offset)
                        .ok_or(DecodeError::Address)
                };
                let range = address(saved.range.start)?..address(saved.range.end)?;
                next.slots[index] = Some((
                    MemorySlot::new(
                        u8::try_from(index + 1).unwrap(),
                        state.generations[index],
                        range,
                        backing,
                    )?,
                    region,
                ));
            }
        }
        Ok(next)
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
    releases: Vec<Release>,
}

#[derive(Debug)]
struct Release {
    address: u64,
    id: [u8; 8],
}

impl Queues {
    pub(super) fn save(&self) -> QueueState {
        QueueState {
            consumers: self.consumers,
            release_producer: self.release_producer,
            release_consumer: self.release_consumer,
            releases: self
                .releases
                .iter()
                .map(|release| ReleaseState {
                    address: release.address,
                    id: release.id,
                })
                .collect(),
        }
    }

    pub(super) fn restore(
        state: &QueueState,
        memory: &Memory,
        addresses: &AddressSpace,
    ) -> Result<Self, TransportError> {
        let outstanding = state.release_producer.wrapping_sub(state.release_consumer);
        if outstanding > 8
            || state.releases.len() > MAX_DEFERRED_RELEASES
            || memory.load(RELEASE_RING)? != 8
            || memory.load(RELEASE_RING + 4)? != state.release_producer
            || memory
                .load(RELEASE_RING + 12)?
                .wrapping_sub(state.release_consumer)
                > outstanding
        {
            return Err(DecodeError::Ring.into());
        }
        let mut unique = std::collections::HashSet::with_capacity(state.releases.len());
        for release in &state.releases {
            addresses.resolve(release.address, 8)?;
            if release.id == [0; 8] || !unique.insert(release.address) {
                return Err(DecodeError::Parameter.into());
            }
        }
        let next = Self {
            consumers: state.consumers,
            release_producer: state.release_producer,
            release_consumer: state.release_consumer,
            releases: state
                .releases
                .iter()
                .map(|release| Release {
                    address: release.address,
                    id: release.id,
                })
                .collect(),
        };
        for queue in [Queue::Commands, Queue::Cursor] {
            next.ring(memory, queue)?;
        }
        Ok(next)
    }

    fn ring(&self, memory: &Memory, queue: Queue) -> Result<Ring, TransportError> {
        let offset = queue.offset();
        let mut header = [0; Ring::HEADER_SIZE];
        let producer = memory.load(offset + 4)?;
        memory.read(Region::Ram, offset, &mut header)?;
        header[4..8].copy_from_slice(&producer.to_le_bytes());
        Ok(Ring::decode(
            &header,
            RingSize::Commands,
            self.consumers[queue.index()],
        )?)
    }

    pub fn pending(&self, memory: &Memory, queue: Queue) -> Result<bool, TransportError> {
        Ok(self.ring(memory, queue)?.pending() != 0)
    }

    pub fn pending_releases(&self) -> bool {
        !self.releases.is_empty()
    }

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
        execute: impl FnMut(Command) -> Result<(), TransportError>,
    ) -> Result<u32, TransportError> {
        self.dispatch(memory, addresses, queue, None, execute)
    }

    pub fn synchronize(
        &mut self,
        memory: &Memory,
        addresses: &AddressSpace,
        queue: Queue,
        execute: impl FnMut(Command) -> Result<(), TransportError>,
    ) -> Result<u32, TransportError> {
        let ring = self.ring(memory, queue)?;
        let target = ring.consumer().wrapping_add(ring.pending());
        self.dispatch(memory, addresses, queue, Some(target), execute)
    }

    fn publish(&mut self, memory: &Memory, addresses: &AddressSpace) -> Result<(), TransportError> {
        if !self.releases.is_empty() && self.release_room(memory)? {
            for (index, release) in self.releases.iter().enumerate() {
                let next = self
                    .releases
                    .get(index + 1)
                    .map_or([0; 8], |release| release.id);
                addresses.write(memory, release.address, &next)?;
            }
            memory.write(
                Region::Ram,
                RELEASE_RING
                    + Ring::HEADER_SIZE
                    + usize::try_from(self.release_producer & 7).unwrap() * 8,
                &self.releases[0].id,
            )?;
            self.release_producer = self.release_producer.wrapping_add(1);
            memory.store(RELEASE_RING + 4, self.release_producer)?;
            if memory.load(RELEASE_RING + 8)? == self.release_producer {
                memory.interrupt(1)?;
            }
            self.releases.clear();
        }
        Ok(())
    }

    fn dispatch(
        &mut self,
        memory: &Memory,
        addresses: &AddressSpace,
        queue: Queue,
        target: Option<u32>,
        mut execute: impl FnMut(Command) -> Result<(), TransportError>,
    ) -> Result<u32, TransportError> {
        self.publish(memory, addresses)?;
        let result = self.drain_commands(memory, addresses, queue, target, &mut execute);
        self.publish(memory, addresses)?;
        if result.is_ok() && target.is_some_and(|target| self.consumers[queue.index()] != target) {
            return Err(DecodeError::Ring.into());
        }
        result
    }

    fn drain_commands(
        &mut self,
        memory: &Memory,
        addresses: &AddressSpace,
        queue: Queue,
        target: Option<u32>,
        execute: &mut impl FnMut(Command) -> Result<(), TransportError>,
    ) -> Result<u32, TransportError> {
        let offset = queue.offset();
        let index = queue.index();
        let mut completed = 0;
        let mut started = false;
        // A producer can refill the ring while it is drained. Bound each dispatch.
        for _ in 0..32 {
            if target == Some(self.consumers[index]) {
                break;
            }
            let mut ring = self.ring(memory, queue)?;
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
                if !started {
                    if !self.release_room(memory)? && target.is_none() {
                        break;
                    }
                    started = true;
                }
                if self.releases.len() == MAX_DEFERRED_RELEASES {
                    return Err(TransportError::Busy);
                }
                if self
                    .releases
                    .iter()
                    .any(|release| release.address == command.address)
                {
                    return Err(DecodeError::Parameter.into());
                }
                let mut release = [0; 8];
                addresses.read(memory, command.address, &mut release)?;
                if release == [0; 8] {
                    return Err(DecodeError::Parameter.into());
                }
                execute(command)?;
                self.releases.push(Release {
                    address: command.address,
                    id: release,
                });
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
