// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use core::ptr::write_volatile;
use core::sync::atomic::{Ordering, fence};

#[repr(C, align(8))]
pub(crate) struct Bytes<const N: usize>(pub(crate) [u8; N]);

pub(crate) struct HostRing {
    pub(crate) base: NonNull<u8>,
    capacity: u32,
    item_bytes: usize,
}

impl HostRing {
    pub(crate) fn new<const N: usize>(
        bytes: &mut Bytes<N>,
        capacity: u32,
        item_bytes: usize,
    ) -> Self {
        assert_eq!(N, 20 + capacity as usize * item_bytes);
        let base = NonNull::from(bytes.0.as_mut_slice()).cast();
        let result = Self {
            base,
            capacity,
            item_bytes,
        };
        result.set(0, capacity);
        result
    }

    pub(crate) fn get(&self, offset: usize) -> u32 {
        // SAFETY: tests access aligned header fields in live backing memory.
        unsafe { read_volatile(self.base.as_ptr().add(offset).cast()) }
    }

    pub(crate) fn set(&self, offset: usize, value: u32) {
        // SAFETY: simulated host writes a field in its live test ring.
        unsafe { write_volatile(self.base.as_ptr().add(offset).cast(), value) };
    }

    fn item(&self, counter: u32) -> *mut u8 {
        // SAFETY: the masked slot is within the constructor-checked mapping.
        unsafe {
            self.base
                .as_ptr()
                .add(20 + (counter % self.capacity) as usize * self.item_bytes)
        }
    }

    pub(crate) fn release(&mut self, id: u64) {
        assert_eq!(self.item_bytes, 8);
        let producer = self.get(4);
        assert!(producer.wrapping_sub(self.get(12)) < self.capacity);
        // SAFETY: the host owns this free entry until publication below.
        unsafe { write_bytes_array(self.item(producer), id.to_le_bytes()) };
        fence(Ordering::Release);
        self.set(4, producer.wrapping_add(1));
    }

    pub(crate) fn command(&self, pool: &Pool<'_>, counter: u32) -> HostOutput {
        assert_eq!(self.item_bytes, 16);
        let mut bytes = [0; 16];
        for (index, byte) in bytes.iter_mut().enumerate() {
            // SAFETY: the guest has published the command before this read.
            *byte = unsafe { read_volatile(self.item(counter).add(index)) };
        }
        assert_eq!(&bytes[12..], &[0; 4]);
        let physical = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        let virtual_address = pool.slot.decode(physical, 8).unwrap();
        let data = pool.owner.as_ptr().with_addr(virtual_address as usize);
        let mut release = [0; 8];
        for (index, byte) in release.iter_mut().enumerate() {
            // SAFETY: the published physical address resolves into this
            // pool's mapping; preserve the mapping's pointer provenance.
            *byte = unsafe { read_volatile(data.add(index)) };
        }
        HostOutput {
            data,
            id: u64::from_le_bytes(release),
            command: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        }
    }
}

pub(crate) struct HostOutput {
    pub(crate) data: *mut u8,
    pub(crate) id: u64,
    pub(crate) command: u32,
}

impl HostOutput {
    /// # Safety
    /// N must not exceed the published command payload's valid extent.
    pub(crate) unsafe fn snapshot<const N: usize>(&self) -> [u8; N] {
        let mut result = [0; N];
        for (index, byte) in result.iter_mut().enumerate() {
            // SAFETY: the caller supplies the complete published extent.
            *byte = unsafe { read_volatile(self.data.add(index)) };
        }
        result
    }

    pub(crate) fn complete(&self, next: u64) {
        // SAFETY: this host owns release_info after guest publication and
        // stops reading all referenced resources before reporting completion.
        unsafe { write_bytes_array(self.data, next.to_le_bytes()) };
    }
}

unsafe fn write_bytes_array<const N: usize>(destination: *mut u8, bytes: [u8; N]) {
    for (index, byte) in bytes.into_iter().enumerate() {
        // SAFETY: the caller supplies a writable range for the entire array.
        unsafe { write_volatile(destination.add(index), byte) };
    }
}

pub(crate) fn pool<'a>(memory: &'a mut [u64], video: Option<&'a mut [u64]>) -> Pool<'a> {
    let length = size_of_val(memory);
    let base = NonNull::from(memory).cast::<u8>();
    let video = video.map(|memory| {
        let bytes = size_of_val(memory);
        (NonNull::from(memory).cast::<u8>(), bytes)
    });
    let slot = crate::slots::SlotLayout::new(8, 8)
        .unwrap()
        .slot(1, 4, 0xe0000000, base.as_ptr().addr() as u64, length as u64)
        .unwrap();
    // SAFETY: the exclusive full-slice borrows retain disjoint aligned live
    // mappings; simulated host access stays within published payloads.
    unsafe { Pool::new(base, length, video, slot).unwrap() }
}

pub(crate) fn empty(pool: &mut Pool<'_>) {
    assert!(pool.is_empty());
    let whole = pool.main.allocate(pool.main.capacity()).unwrap();
    pool.free_main(whole);
    if let Some(video) = &mut pool.video {
        let whole = video.allocate(video.capacity()).unwrap();
        video.release(whole).unwrap();
    }
}
