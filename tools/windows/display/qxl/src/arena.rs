// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Fixed BAR-memory allocation for display commands and surfaces.

use core::marker::PhantomData;
use core::num::NonZeroUsize;
use core::ptr::{NonNull, read_volatile, write_volatile};

const ALIGNMENT: usize = 8;
const END: usize = usize::MAX;

#[derive(Clone, Copy, Debug)]
#[repr(C)]
struct FreeBlock {
    size: usize,
    next: usize,
}

const MIN_BLOCK: usize = size_of::<FreeBlock>();

/// An allocation retained until all host and guest users have released it.
/// Dropping this token does not return memory to the arena.
#[derive(Debug)]
#[must_use]
pub struct Allocation<'memory> {
    base: NonNull<u8>,
    offset: usize,
    size: usize,
    length: NonZeroUsize,
    memory: PhantomData<&'memory mut [u8]>,
}

impl Allocation<'_> {
    pub fn as_ptr(&self) -> *mut u8 {
        // SAFETY: allocation offsets are within the original live mapping.
        unsafe { self.base.as_ptr().add(self.offset) }
    }

    pub fn len(&self) -> usize {
        self.length.get()
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

/// An eight-byte-aligned, first-fit allocator with coalesced free blocks.
/// All calls require the display driver's allocation semaphore.
#[derive(Debug)]
pub struct Arena<'memory> {
    base: NonNull<u8>,
    length: usize,
    first: usize,
    memory: PhantomData<&'memory mut [u8]>,
}

impl<'memory> Arena<'memory> {
    /// # Safety
    /// The mapping must remain readable and writable for `'memory`. No other
    /// allocator may manage it, including while allocation tokens remain live.
    /// Guest and host access must stay within live allocation payloads; free
    /// blocks belong exclusively to this allocator. Do not reset or unmap the
    /// device while this arena or its allocations remain in use.
    pub unsafe fn new(base: NonNull<u8>, length: usize) -> Option<Self> {
        let length = length & !(ALIGNMENT - 1);
        if !base.as_ptr().addr().is_multiple_of(ALIGNMENT)
            || length < MIN_BLOCK
            || length > isize::MAX as usize
        {
            return None;
        }
        let mut arena = Self {
            base,
            length,
            first: 0,
            memory: PhantomData,
        };
        arena.write(
            0,
            FreeBlock {
                size: length,
                next: END,
            },
        );
        Some(arena)
    }

    pub fn allocate(&mut self, length: usize) -> Option<Allocation<'memory>> {
        let length = NonZeroUsize::new(length)?;
        let size = length.get().max(MIN_BLOCK).checked_add(ALIGNMENT - 1)? & !(ALIGNMENT - 1);
        let mut previous = END;
        let mut offset = self.first;
        while offset != END {
            let block = self.read(offset);
            if block.size >= size {
                let remainder = block.size - size;
                let (size, next) = if remainder >= MIN_BLOCK {
                    let next = offset + size;
                    self.write(
                        next,
                        FreeBlock {
                            size: remainder,
                            next: block.next,
                        },
                    );
                    (size, next)
                } else {
                    (block.size, block.next)
                };
                self.link(previous, next);
                return Some(Allocation {
                    base: self.base,
                    offset,
                    size,
                    length,
                    memory: PhantomData,
                });
            }
            previous = offset;
            offset = block.next;
        }
        None
    }

    /// Return a consumed token, or preserve it if it belongs to another arena.
    /// The caller must have ended all asynchronous device use before release.
    pub fn release(&mut self, allocation: Allocation<'memory>) -> Result<(), Allocation<'memory>> {
        if allocation.base != self.base {
            return Err(allocation);
        }
        let mut previous = END;
        let mut next = self.first;
        while next != END && next < allocation.offset {
            previous = next;
            next = self.read(next).next;
        }
        let mut block = FreeBlock {
            size: allocation.size,
            next,
        };
        if next != END && allocation.offset + block.size == next {
            let following = self.read(next);
            block.size += following.size;
            block.next = following.next;
        }
        if previous != END {
            let mut preceding = self.read(previous);
            if previous + preceding.size == allocation.offset {
                preceding.size += block.size;
                preceding.next = block.next;
                self.write(previous, preceding);
                return Ok(());
            }
        }
        self.write(allocation.offset, block);
        self.link(previous, allocation.offset);
        Ok(())
    }

    pub fn capacity(&self) -> usize {
        self.length
    }

    fn link(&mut self, previous: usize, next: usize) {
        if previous == END {
            self.first = next;
        } else {
            let mut block = self.read(previous);
            block.next = next;
            self.write(previous, block);
        }
    }

    fn read(&self, offset: usize) -> FreeBlock {
        // SAFETY: the free list contains aligned, complete headers written by
        // this arena. Allocated payloads never overlap a free block header.
        unsafe { read_volatile(self.base.as_ptr().add(offset).cast()) }
    }

    fn write(&mut self, offset: usize, block: FreeBlock) {
        // SAFETY: splitting and coalescing retain aligned, disjoint free ranges
        // inside the mapping. Free blocks have at least MIN_BLOCK bytes.
        unsafe { write_volatile(self.base.as_ptr().add(offset).cast(), block) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arena(memory: &mut [u64]) -> Arena<'_> {
        let length = size_of_val(memory);
        let base = NonNull::from(memory).cast();
        // SAFETY: the exclusive slice borrow retains a live aligned region;
        // tests touch only payloads of allocations until their release.
        unsafe { Arena::new(base, length).unwrap() }
    }

    #[test]
    fn split_and_coalesce_both_neighbours_without_metadata_overhead() {
        let mut memory = [0u64; 64];
        let mut arena = arena(&mut memory);
        let first = arena.allocate(96).unwrap();
        let middle = arena.allocate(64).unwrap();
        let last = arena.allocate(352).unwrap();
        assert!(arena.allocate(1).is_none());
        arena.release(first).unwrap();
        arena.release(last).unwrap();
        arena.release(middle).unwrap();
        let whole = arena.allocate(arena.capacity()).unwrap();
        assert_eq!(whole.len(), 512);
        assert!(arena.allocate(1).is_none());
        arena.release(whole).unwrap();
    }

    #[test]
    fn reject_invalid_requests_without_consuming_space() {
        let mut memory = [0u64; 8];
        let mut arena = arena(&mut memory);
        for size in [0, usize::MAX, arena.capacity() + 1] {
            assert!(arena.allocate(size).is_none());
        }
        let whole = arena.allocate(arena.capacity()).unwrap();
        assert_eq!(whole.as_ptr().addr() % ALIGNMENT, 0);
        assert!(!whole.is_empty());
        arena.release(whole).unwrap();
    }

    #[test]
    fn wrong_arena_preserves_the_allocation() {
        let mut first = [0u64; 8];
        let mut second = [0u64; 8];
        let mut first = arena(&mut first);
        let mut second = arena(&mut second);
        let block = first.allocate(64).unwrap();
        let block = second.release(block).unwrap_err();
        assert!(first.allocate(1).is_none());
        first.release(block).unwrap();
        assert!(first.allocate(64).is_some());
    }

    #[test]
    fn varied_lifetimes_preserve_live_payloads_and_recover_all_space() {
        let mut memory = [0u64; 1024];
        let mut arena = arena(&mut memory);
        let mut live: [Option<(Allocation<'_>, u8)>; 32] = core::array::from_fn(|_| None);
        let mut random = 0x9e3779b9u32;
        for step in 0..4096 {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            let index = random as usize % live.len();
            if let Some((block, pattern)) = live[index].take() {
                for byte in 0..block.len() {
                    // SAFETY: this is a still-live allocation within the arena.
                    assert_eq!(unsafe { block.as_ptr().add(byte).read() }, pattern);
                }
                arena.release(block).unwrap();
            } else if let Some(block) = arena.allocate((random as usize >> 8) % 511 + 1) {
                let pattern = step as u8;
                // SAFETY: this allocation owns its complete payload range.
                unsafe { block.as_ptr().write_bytes(pattern, block.len()) };
                live[index] = Some((block, pattern));
            }
        }
        for (block, _) in live.into_iter().flatten() {
            arena.release(block).unwrap();
        }
        assert!(arena.allocate(arena.capacity()).is_some());
    }

    #[test]
    fn reject_unaligned_or_short_mappings_without_writes() {
        let mut memory = [0x5555555555555555u64; 8];
        let base = NonNull::from(memory.as_mut_slice()).cast::<u8>();
        // SAFETY: both ranges remain within the backing allocation. Invalid
        // alignment and size must be rejected before a header is written.
        unsafe {
            assert!(Arena::new(base.add(1), 63).is_none());
            assert!(Arena::new(base, MIN_BLOCK - 1).is_none());
        }
        assert_eq!(memory, [0x5555555555555555; 8]);
    }

    #[test]
    fn absorb_tiny_tails_and_round_the_mapping_down() {
        let mut memory = [0u64; 8];
        let base = NonNull::from(memory.as_mut_slice()).cast();
        // SAFETY: the aligned backing memory contains all 63 supplied bytes.
        let mut arena = unsafe { Arena::new(base, 63).unwrap() };
        assert_eq!(arena.capacity(), 56);
        let length = 56 - MIN_BLOCK + ALIGNMENT;
        let block = arena.allocate(length).unwrap();
        assert_eq!(block.len(), length);
        assert!(arena.allocate(1).is_none());
        arena.release(block).unwrap();
        assert!(arena.allocate(56).is_some());
    }
}
