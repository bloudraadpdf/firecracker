// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Packed QXL palettes and the retained palette LRU from display/res.c.

use crate::display_resources::{Error, Pool, Region, Resource};
use crate::slots::MemorySlot;

#[cfg(any(test, windows))]
pub(crate) const CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct Colors<'a> {
    unique: u32,
    colors: &'a [u32],
}

impl<'a> Colors<'a> {
    pub fn new(unique: u32, colors: &'a [u32]) -> Option<Self> {
        (!colors.is_empty() && colors.len() <= 256).then_some(Self { unique, colors })
    }
}

#[derive(Debug)]
pub struct Palette<'memory> {
    memory: Resource<'memory>,
    physical: u64,
    unique: u32,
}

impl<'memory> Palette<'memory> {
    pub fn allocate(
        pool: &mut Pool<'memory>,
        slot: MemorySlot,
        colors: Colors<'_>,
    ) -> Result<Self, Error> {
        let bytes = 10 + colors.colors.len() * 4;
        let memory = pool.resource(Region::Main, bytes)?;
        let physical = match slot.encode(memory.as_ptr().addr() as u64, bytes as u64) {
            Ok(address) => address,
            Err(error) => {
                pool.release(memory).map_err(|(error, _)| error)?;
                return Err(Error::Address(error));
            }
        };
        // SAFETY: the unpublished allocation owns every byte of this packed palette.
        let output = unsafe { core::slice::from_raw_parts_mut(memory.as_ptr(), bytes) };
        output[..8].copy_from_slice(&u64::from(colors.unique).to_le_bytes());
        output[8..10].copy_from_slice(&(colors.colors.len() as u16).to_le_bytes());
        for (output, color) in output[10..]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(colors.colors)
        {
            output.copy_from_slice(&color.to_le_bytes());
        }
        Ok(Self {
            memory,
            physical,
            unique: colors.unique,
        })
    }

    pub fn address(&self) -> u64 {
        self.physical
    }

    pub fn retain(&self, pool: &mut Pool<'memory>) -> Result<Self, Error> {
        Ok(Self {
            memory: pool.retain(&self.memory)?,
            physical: self.physical,
            unique: self.unique,
        })
    }

    pub fn release(self, pool: &mut Pool<'memory>) -> Result<(), (Error, Self)> {
        pool.release(self.memory)
            .map_err(|(error, memory)| (error, Self { memory, ..self }))
    }

    pub(crate) fn validate_slot(&self, slot: MemorySlot) -> Result<(), Error> {
        if slot
            .encode(self.memory.as_ptr().addr() as u64, self.memory.len() as u64)
            .map_err(Error::Address)?
            != self.address()
        {
            return Err(Error::Address(crate::slots::SlotError::WrongSlot));
        }
        Ok(())
    }

    pub(crate) fn resource(&self) -> &Resource<'memory> {
        &self.memory
    }

    pub(crate) fn release_with(
        self,
        first: Resource<'memory>,
        pool: &mut Pool<'memory>,
    ) -> Result<(), (Error, Resource<'memory>, Self)> {
        pool.release_pair(first, self.memory)
            .map_err(|(error, first, memory)| (error, first, Self { memory, ..self }))
    }

    pub(crate) fn colors(&self) -> &[u8] {
        // SAFETY: this palette retains its fully initialised packed colour words.
        unsafe { core::slice::from_raw_parts(self.memory.as_ptr().add(10), self.memory.len() - 10) }
    }

    #[cfg(any(test, windows))]
    fn matches(&self, colors: Colors<'_>) -> bool {
        self.unique == colors.unique
            && self.colors().len() == colors.colors.len() * 4
            && self
                .colors()
                .as_chunks::<4>()
                .0
                .iter()
                .zip(colors.colors)
                .all(|(stored, color)| *stored == color.to_le_bytes())
    }
}

#[cfg(any(test, windows))]
#[derive(Debug)]
pub(crate) struct Cache<'storage, 'memory>(pub(crate) &'storage mut [Option<Palette<'memory>>]);

#[cfg(any(test, windows))]
impl<'memory> Cache<'_, 'memory> {
    fn evict(&mut self, pool: &mut Pool<'memory>) -> Result<bool, Error> {
        let Some(index) = self.0.iter().rposition(Option::is_some) else {
            return Ok(false);
        };
        let palette = self.0[index].take().ok_or(Error::InvalidChain)?;
        if let Err((error, palette)) = palette.release(pool) {
            self.0[index] = Some(palette);
            return Err(error);
        }
        Ok(true)
    }

    pub(crate) fn clear(&mut self, pool: &mut Pool<'memory>) -> Result<(), Error> {
        while self.evict(pool)? {}
        Ok(())
    }

    pub(crate) fn acquire(
        &mut self,
        pool: &mut Pool<'memory>,
        slot: MemorySlot,
        colors: Colors<'_>,
    ) -> Result<Palette<'memory>, Error> {
        if self
            .0
            .iter()
            .flatten()
            .any(|palette| !pool.owns(palette.resource()))
        {
            return Err(Error::WrongPool);
        }
        if colors.unique != 0
            && let Some(index) = self.0.iter().position(|palette| {
                palette
                    .as_ref()
                    .is_some_and(|palette| palette.matches(colors))
            })
        {
            let palette = self.0[index].as_ref().ok_or(Error::InvalidChain)?;
            palette.validate_slot(slot)?;
            let retained = palette.retain(pool)?;
            self.0[..=index].rotate_right(1);
            return Ok(retained);
        }
        let palette = loop {
            match Palette::allocate(pool, slot, colors) {
                Ok(palette) => break palette,
                Err(Error::OutOfMemory) if self.evict(pool)? => {}
                Err(error) => return Err(error),
            }
        };
        if colors.unique != 0 {
            let retained = match palette.retain(pool) {
                Ok(retained) => retained,
                Err(error) => {
                    palette.release(pool).map_err(|(error, _)| error)?;
                    return Err(error);
                }
            };
            let index = self
                .0
                .iter()
                .position(Option::is_none)
                .unwrap_or(self.0.len() - 1);
            if self.0[index].is_some()
                && let Err(error) = self.evict(pool)
            {
                retained.release(pool).map_err(|(error, _)| error)?;
                palette.release(pool).map_err(|(error, _)| error)?;
                return Err(error);
            }
            self.0[..=index].rotate_right(1);
            self.0[0] = Some(retained);
        }
        Ok(palette)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_resources::test_utils::{empty, pool};
    use crate::slots::SlotLayout;

    #[test]
    fn palette_lru_preserves_shared_payloads_and_never_caches_zero_identity() {
        let mut memory = [0u64; 4096];
        let slot = SlotLayout::new(8, 8)
            .unwrap()
            .slot(
                1,
                4,
                0xe0000000,
                memory.as_ptr().addr() as u64,
                core::mem::size_of_val(&memory) as u64,
            )
            .unwrap();
        let mut pool = pool(&mut memory, None);
        let mut entries = core::array::from_fn::<_, CAPACITY, _>(|_| None);
        let mut cache = Cache(&mut entries);
        let colors = [0x123456, 0xabcdef];
        let data = Colors::new(1, &colors).unwrap();
        let retained = cache.acquire(&mut pool, slot, data).unwrap();
        let address = retained.address();
        let hit = cache.acquire(&mut pool, slot, data).unwrap();
        assert_eq!(hit.address(), address);
        hit.release(&mut pool).unwrap();
        for unique in 2..=CAPACITY as u32 + 1 {
            cache
                .acquire(&mut pool, slot, Colors::new(unique, &colors).unwrap())
                .unwrap()
                .release(&mut pool)
                .unwrap();
        }
        assert!(!cache.0.iter().flatten().any(|palette| palette.unique == 1));
        cache.clear(&mut pool).unwrap();
        assert_eq!(
            retained.colors(),
            &[0x56, 0x34, 0x12, 0, 0xef, 0xcd, 0xab, 0]
        );
        retained.release(&mut pool).unwrap();
        let uncached = cache
            .acquire(&mut pool, slot, Colors::new(0, &colors).unwrap())
            .unwrap();
        assert!(cache.0.iter().all(Option::is_none));
        uncached.release(&mut pool).unwrap();
        empty(&mut pool);
    }
}
