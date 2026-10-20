// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Retained image hash chains and LRU eviction from display/res.c.

use crate::bitmap::{Bitmap, Format};
use crate::display_resources::{Error, Pool};
use crate::image::Image;
#[cfg(windows)]
use crate::palette::Colors;
use crate::palette::Palette;
use crate::slots::MemorySlot;

const NONE: u16 = u16::MAX;

#[derive(Debug)]
struct Entry<'memory> {
    image: Option<Image<'memory>>,
    hash: u32,
    hash_next: u16,
    previous: u16,
    next: u16,
}

impl Entry<'_> {
    fn empty(next: u16) -> Self {
        Self {
            image: None,
            hash: 0,
            hash_next: NONE,
            previous: NONE,
            next,
        }
    }
}

#[derive(Debug)]
struct State {
    first: u16,
    last: u16,
    free: u16,
    bytes: usize,
    limit: usize,
}

impl State {
    fn new(limit: usize) -> Self {
        Self {
            first: NONE,
            last: NONE,
            free: 0,
            bytes: 0,
            limit,
        }
    }
}

struct Cache<'storage, 'memory> {
    entries: &'storage mut [Entry<'memory>],
    buckets: &'storage mut [u16],
    state: &'storage mut State,
}

impl<'memory> Cache<'_, 'memory> {
    fn bucket(&self, hash: u32) -> usize {
        hash as usize % self.buckets.len()
    }

    fn unlink(&mut self, index: u16) {
        let entry = &self.entries[usize::from(index)];
        let (previous, next) = (entry.previous, entry.next);
        if previous == NONE {
            self.state.first = next;
        } else {
            self.entries[usize::from(previous)].next = next;
        }
        if next == NONE {
            self.state.last = previous;
        } else {
            self.entries[usize::from(next)].previous = previous;
        }
    }

    fn touch(&mut self, index: u16) {
        let first = self.state.first;
        let entry = &mut self.entries[usize::from(index)];
        entry.previous = NONE;
        entry.next = first;
        if first != NONE {
            self.entries[usize::from(first)].previous = index;
        } else {
            self.state.last = index;
        }
        self.state.first = index;
    }

    fn evict(&mut self, pool: &mut Pool<'memory>) -> Result<bool, Error> {
        let index = self.state.last;
        if index == NONE {
            return Ok(false);
        }
        let entry = &mut self.entries[usize::from(index)];
        let image = entry.image.take().ok_or(Error::InvalidChain)?;
        let bytes = image.bytes();
        if let Err((error, image)) = image.release(pool) {
            entry.image = Some(image);
            return Err(error);
        }
        let (hash, next) = (entry.hash, entry.hash_next);
        let bucket = self.bucket(hash);
        let mut current = self.buckets[bucket];
        let mut previous = NONE;
        while current != index {
            if current == NONE {
                return Err(Error::InvalidChain);
            }
            previous = current;
            current = self.entries[usize::from(current)].hash_next;
        }
        if previous == NONE {
            self.buckets[bucket] = next;
        } else {
            self.entries[usize::from(previous)].hash_next = next;
        }
        self.unlink(index);
        self.entries[usize::from(index)] = Entry::empty(self.state.free);
        self.state.free = index;
        self.state.bytes -= bytes;
        Ok(true)
    }

    fn clear(&mut self, pool: &mut Pool<'memory>) -> Result<(), Error> {
        while self.evict(pool)? {}
        Ok(())
    }

    fn acquire(
        &mut self,
        pool: &mut Pool<'memory>,
        slot: MemorySlot,
        width: u32,
        height: u32,
        pixels: &[u8],
        alpha: bool,
    ) -> Result<Image<'memory>, Error> {
        let format = if alpha {
            Format::Bgra32
        } else {
            Format::Bgrx32
        };
        let bitmap = Bitmap::new(width, height, format, pixels).ok_or(Error::InvalidSize)?;
        self.acquire_bitmap(pool, slot, bitmap, None)
    }

    fn acquire_bitmap(
        &mut self,
        pool: &mut Pool<'memory>,
        slot: MemorySlot,
        bitmap: Bitmap<'_>,
        palette: Option<&Palette<'memory>>,
    ) -> Result<Image<'memory>, Error> {
        if !bitmap.validate_palette(palette.map_or(0, |palette| palette.colors().len() / 4)) {
            return Err(Error::InvalidSize);
        }
        if self.state.first != NONE {
            let image = self.entries[usize::from(self.state.first)]
                .image
                .as_ref()
                .ok_or(Error::InvalidChain)?;
            if !pool.owns(image.resource()) {
                return Err(Error::WrongPool);
            }
            if slot
                .encode(
                    image.resource().as_ptr().addr() as u64,
                    image.bytes() as u64,
                )
                .map_err(Error::Address)?
                != image.address()
            {
                return Err(Error::Address(crate::slots::SlotError::WrongSlot));
            }
        }
        let seed = bitmap.width | (bitmap.height << 13) | ((bitmap.format as u32) << 26);
        let mut hash = crate::hash::murmur2a(bitmap.pixels, seed).ok_or(Error::InvalidSize)?;
        if let Some(palette) = palette {
            hash = crate::hash::murmur2a(palette.colors(), hash).ok_or(Error::InvalidSize)?;
        }
        let bucket = self.bucket(hash);
        let mut index = self.buckets[bucket];
        while index != NONE {
            let entry = &self.entries[usize::from(index)];
            let image = entry.image.as_ref().ok_or(Error::InvalidChain)?;
            if entry.hash == hash && image.matches_bitmap(bitmap, palette) {
                let retained = image.retain(pool)?;
                self.unlink(index);
                self.touch(index);
                return Ok(retained);
            }
            index = entry.hash_next;
        }
        let bytes = bitmap
            .pixels
            .len()
            .checked_add(48)
            .ok_or(Error::InvalidSize)?;
        let cacheable = bytes <= self.state.limit;
        if cacheable {
            while self.state.free == NONE || bytes > self.state.limit - self.state.bytes {
                if !self.evict(pool)? {
                    return Err(Error::OutOfMemory);
                }
            }
        }
        let image = loop {
            match Image::allocate_bitmap(pool, slot, bitmap, palette) {
                Ok(image) => break image,
                Err(Error::OutOfMemory) if self.evict(pool)? => {}
                Err(error) => return Err(error),
            }
        };
        if cacheable {
            let retained = match image.retain(pool) {
                Ok(retained) => retained,
                Err(error) => {
                    image.release(pool).map_err(|(error, _)| error)?;
                    return Err(error);
                }
            };
            let index = self.state.free;
            let entry = &mut self.entries[usize::from(index)];
            self.state.free = entry.next;
            *entry = Entry {
                image: Some(retained),
                hash,
                hash_next: self.buckets[bucket],
                previous: NONE,
                next: NONE,
            };
            self.buckets[bucket] = index;
            self.touch(index);
            self.state.bytes += bytes;
        }
        Ok(image)
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::engine::Memory;
    use core::marker::PhantomData;
    use core::mem::{align_of, size_of};

    const CAPACITY: usize = 1 << 15;
    const ENTRIES_BYTES: usize = CAPACITY * size_of::<Entry<'static>>();
    const PALETTE_OFFSET: usize = ENTRIES_BYTES + CAPACITY * size_of::<u16>();
    const STORAGE_BYTES: usize =
        PALETTE_OFFSET + crate::palette::CAPACITY * size_of::<Option<Palette<'static>>>();

    const _: () = {
        assert!(CAPACITY < NONE as usize);
        assert!(STORAGE_BYTES <= u32::MAX as usize);
        assert!(ENTRIES_BYTES.is_multiple_of(align_of::<u16>()));
        assert!(PALETTE_OFFSET.is_multiple_of(align_of::<Option<Palette<'static>>>()));
    };

    #[derive(Debug)]
    pub(crate) struct Images<'memory> {
        memory: Memory,
        state: State,
        lifetime: PhantomData<Entry<'memory>>,
    }

    impl<'memory> Images<'memory> {
        /// # Safety
        /// Run at PASSIVE_LEVEL; retain the mappings and pool until clear succeeds.
        pub(crate) unsafe fn new(limit: usize) -> Result<Self, u32> {
            // SAFETY: the native callback retains the passive-level GDI context.
            let memory = unsafe { Memory::zeroed(STORAGE_BYTES as u32) }?;
            if !memory
                .as_ptr()
                .as_ptr()
                .addr()
                .is_multiple_of(align_of::<Entry<'memory>>())
            {
                return Err(13);
            }
            let pointer = memory.as_ptr().as_ptr().cast::<Entry<'memory>>();
            for index in 0..CAPACITY {
                let next = if index + 1 == CAPACITY {
                    NONE
                } else {
                    (index + 1) as u16
                };
                // SAFETY: the fresh, aligned GDI allocation covers every entry;
                // initialise typed values before constructing any Rust slice.
                unsafe { pointer.add(index).write(Entry::empty(next)) };
            }
            // SAFETY: this disjoint trailing region covers all u16 hash heads.
            unsafe {
                memory
                    .as_ptr()
                    .as_ptr()
                    .add(ENTRIES_BYTES)
                    .write_bytes(0xff, CAPACITY * 2)
            };
            for index in 0..crate::palette::CAPACITY {
                // SAFETY: this aligned trailing region has no other typed users.
                unsafe {
                    memory
                        .as_ptr()
                        .as_ptr()
                        .add(PALETTE_OFFSET)
                        .cast::<Option<Palette<'memory>>>()
                        .add(index)
                        .write(None)
                };
            }
            Ok(Self {
                memory,
                state: State::new(limit),
                lifetime: PhantomData,
            })
        }

        fn cache(&mut self) -> Cache<'_, 'memory> {
            let base = self.memory.as_ptr().as_ptr();
            // SAFETY: new initialises these aligned, disjoint regions in the
            // retained GDI allocation. This exclusive borrow covers all metadata.
            let (entries, buckets) = unsafe {
                (
                    core::slice::from_raw_parts_mut(base.cast(), CAPACITY),
                    core::slice::from_raw_parts_mut(base.add(ENTRIES_BYTES).cast(), CAPACITY),
                )
            };
            Cache {
                entries,
                buckets,
                state: &mut self.state,
            }
        }

        pub(crate) fn acquire(
            &mut self,
            pool: &mut Pool<'memory>,
            slot: MemorySlot,
            width: u32,
            height: u32,
            pixels: &[u8],
            alpha: bool,
        ) -> Result<Image<'memory>, Error> {
            self.cache()
                .acquire(pool, slot, width, height, pixels, alpha)
        }

        fn palettes(&mut self) -> crate::palette::Cache<'_, 'memory> {
            // SAFETY: new initialises this disjoint aligned array; the GDI
            // allocation and hardware lease outlive this exclusive view.
            crate::palette::Cache(unsafe {
                core::slice::from_raw_parts_mut(
                    self.memory.as_ptr().as_ptr().add(PALETTE_OFFSET).cast(),
                    crate::palette::CAPACITY,
                )
            })
        }

        pub(crate) fn acquire_indexed(
            &mut self,
            pool: &mut Pool<'memory>,
            slot: MemorySlot,
            bitmap: Bitmap<'_>,
            colors: Colors<'_>,
        ) -> Result<Image<'memory>, Error> {
            let palette = self.palettes().acquire(pool, slot, colors)?;
            let result = self
                .cache()
                .acquire_bitmap(pool, slot, bitmap, Some(&palette));
            palette.release(pool).map_err(|(error, _)| error)?;
            result
        }

        pub(crate) fn clear(&mut self, pool: &mut Pool<'memory>) -> Result<(), Error> {
            self.cache().clear(pool)?;
            self.palettes().clear(pool)
        }
    }
}

#[cfg(windows)]
pub(crate) use native::Images;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_resources::test_utils::{Bytes, HostRing, empty, pool};
    use crate::draw::ImageDraw;
    use crate::ring::{CommandRing, ReleaseRing};
    use crate::slots::SlotLayout;

    fn slot(memory: &mut [u64], generation: u8) -> MemorySlot {
        SlotLayout::new(8, 8)
            .unwrap()
            .slot(
                1,
                generation,
                0xe0000000,
                memory.as_ptr().addr() as u64,
                core::mem::size_of_val(memory) as u64,
            )
            .unwrap()
    }

    #[test]
    fn hits_update_lru_and_byte_budget_evicts_the_oldest_image() {
        let mut backing = [0u64; 512];
        let slot = slot(&mut backing, 4);
        let mut pool = pool(&mut backing, None);
        let mut entries = [Entry::empty(1), Entry::empty(NONE)];
        let mut buckets = [NONE; 2];
        let mut state = State::new(104);
        let mut cache = Cache {
            entries: &mut entries,
            buckets: &mut buckets,
            state: &mut state,
        };
        let first = cache
            .acquire(&mut pool, slot, 1, 1, &[1; 4], false)
            .unwrap();
        let first_address = first.address();
        first.release(&mut pool).unwrap();
        cache
            .acquire(&mut pool, slot, 1, 1, &[2; 4], false)
            .unwrap()
            .release(&mut pool)
            .unwrap();
        let hit = cache
            .acquire(&mut pool, slot, 1, 1, &[1; 4], false)
            .unwrap();
        assert_eq!(hit.address(), first_address);
        hit.release(&mut pool).unwrap();
        cache
            .acquire(&mut pool, slot, 1, 1, &[3; 4], false)
            .unwrap()
            .release(&mut pool)
            .unwrap();
        assert_eq!(cache.state.bytes, 104);
        assert!(cache.entries.iter().any(|entry| {
            entry
                .image
                .as_ref()
                .is_some_and(|image| image.matches(1, 1, &[1; 4], false))
        }));
        assert!(!cache.entries.iter().any(|entry| {
            entry
                .image
                .as_ref()
                .is_some_and(|image| image.matches(1, 1, &[2; 4], false))
        }));
        cache.clear(&mut pool).unwrap();
        empty(&mut pool);
    }

    #[test]
    fn hash_collisions_compare_pixels_and_reject_other_pools_or_generations() {
        let mut backing = [0u64; 512];
        let mut other = [0u64; 512];
        let slot = slot(&mut backing, 4);
        let stale = SlotLayout::new(8, 8)
            .unwrap()
            .slot(
                1,
                5,
                0xe0000000,
                backing.as_ptr().addr() as u64,
                core::mem::size_of_val(&backing) as u64,
            )
            .unwrap();
        let mut pool = pool(&mut backing, None);
        let mut foreign = crate::display_resources::test_utils::pool(&mut other, None);
        let mut entries = [Entry::empty(1), Entry::empty(NONE)];
        let mut buckets = [NONE];
        let mut state = State::new(104);
        let mut cache = Cache {
            entries: &mut entries,
            buckets: &mut buckets,
            state: &mut state,
        };
        let first = cache
            .acquire(&mut pool, slot, 1, 1, &[1; 4], false)
            .unwrap();
        let address = first.address();
        first.release(&mut pool).unwrap();
        cache.entries[0].hash = crate::hash::murmur2a(&[2; 4], 1 | (1 << 13) | (8 << 26)).unwrap();
        let second = cache
            .acquire(&mut pool, slot, 1, 1, &[2; 4], false)
            .unwrap();
        assert_ne!(second.address(), address);
        second.release(&mut pool).unwrap();
        assert!(matches!(
            cache.acquire(&mut foreign, slot, 1, 1, &[2; 4], false),
            Err(Error::WrongPool)
        ));
        assert!(matches!(
            cache.acquire(&mut pool, stale, 1, 1, &[2; 4], false),
            Err(Error::Address(crate::slots::SlotError::WrongSlot))
        ));
        assert_eq!(cache.clear(&mut foreign), Err(Error::WrongPool));
        cache.clear(&mut pool).unwrap();
        empty(&mut pool);
        empty(&mut foreign);
    }

    #[test]
    fn eviction_and_cache_clear_keep_inflight_payloads_until_host_release() {
        let mut backing = [0u64; 512];
        let slot = slot(&mut backing, 4);
        let mut pool = pool(&mut backing, None);
        let mut entries = [Entry::empty(NONE)];
        let mut buckets = [NONE];
        let mut state = State::new(52);
        let mut cache = Cache {
            entries: &mut entries,
            buckets: &mut buckets,
            state: &mut state,
        };
        let mut commands_memory = Bytes([0; 532]);
        let host = HostRing::new(&mut commands_memory, 32, 16);
        let mut release_memory = Bytes([0; 84]);
        let mut release_host = HostRing::new(&mut release_memory, 8, 8);
        // SAFETY: each fake host retains its disjoint complete ring mapping.
        let (mut commands, mut releases) = unsafe {
            (
                CommandRing::new(host.base, 532).unwrap(),
                ReleaseRing::new(release_host.base, 84).unwrap(),
            )
        };
        let image = cache
            .acquire(&mut pool, slot, 1, 1, &[1, 2, 3, 0], false)
            .unwrap();
        let pointer = image.resource().as_ptr();
        let draw = ImageDraw::copy([0, 0, 1, 1], [0, 0, 1, 1], 0xcc).unwrap();
        // SAFETY: the fake primary and immutable image remain live through publication.
        unsafe { draw.submit(&image, &mut pool, &mut commands, 0) }.unwrap();
        image.release(&mut pool).unwrap();
        cache
            .acquire(&mut pool, slot, 1, 1, &[9; 4], false)
            .unwrap()
            .release(&mut pool)
            .unwrap();
        cache.clear(&mut pool).unwrap();
        assert!(!pool.is_empty());
        // SAFETY: the uncompleted host command still owns the entire first image.
        let retained_pixels = unsafe { core::slice::from_raw_parts(pointer.add(48), 4) };
        assert_eq!(retained_pixels, &[1, 2, 3, 0]);
        let output = host.command(&pool, 0);
        output.complete(0);
        release_host.release(output.id);
        // SAFETY: the fake host stopped every command and image read before release.
        assert_eq!(unsafe { pool.reap(&mut releases, 1) }.unwrap().outputs, 1);
        empty(&mut pool);
    }

    #[test]
    fn arena_pressure_evicts_cached_payloads_before_failing_allocation() {
        let mut backing = [0u64; 128];
        let slot = slot(&mut backing, 4);
        let mut pool = pool(&mut backing, None);
        let mut entries = core::array::from_fn::<_, 4, _>(|index| {
            Entry::empty(if index == 3 { NONE } else { index as u16 + 1 })
        });
        let mut buckets = [NONE; 4];
        let mut state = State::new(4096);
        let mut cache = Cache {
            entries: &mut entries,
            buckets: &mut buckets,
            state: &mut state,
        };
        for value in [1, 2, 3] {
            cache
                .acquire(&mut pool, slot, 8, 8, &[value; 256], false)
                .unwrap()
                .release(&mut pool)
                .unwrap();
        }
        assert_eq!(cache.state.bytes, 608);
        assert!(!cache.entries.iter().any(|entry| {
            entry
                .image
                .as_ref()
                .is_some_and(|image| image.matches(8, 8, &[1; 256], false))
        }));
        let larger = cache
            .acquire(&mut pool, slot, 16, 8, &[0; 512], false)
            .unwrap();
        larger.release(&mut pool).unwrap();
        cache.clear(&mut pool).unwrap();
        empty(&mut pool);
    }
}
