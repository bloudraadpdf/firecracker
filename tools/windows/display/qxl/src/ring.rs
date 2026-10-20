// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Packed SPICE rings used by the XDDM command, cursor and release paths.

use core::ptr::{NonNull, read_volatile, write_volatile};
use core::sync::atomic::{Ordering, fence};

const HEADER_BYTES: usize = 20;
const PROD: usize = 4;
const NOTIFY_PROD: usize = 8;
const CONS: usize = 12;
const NOTIFY_CONS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingError {
    InvalidMapping,
    InvalidCapacity,
    CorruptCounters,
}

#[derive(Debug)]
struct Mapping<const ITEM_BYTES: usize, const CAPACITY: u32> {
    base: NonNull<u8>,
}

impl<const ITEM_BYTES: usize, const CAPACITY: u32> Mapping<ITEM_BYTES, CAPACITY> {
    // The caller provides the mapping and exclusive guest-side ring ownership.
    unsafe fn new(base: NonNull<u8>, length: usize) -> Result<Self, RingError> {
        if base.as_ptr().addr() & 3 != 0 || length < HEADER_BYTES + ITEM_BYTES * CAPACITY as usize {
            return Err(RingError::InvalidMapping);
        }
        let result = Self { base };
        if result.read(0) != CAPACITY {
            return Err(RingError::InvalidCapacity);
        }
        Ok(result)
    }

    fn read(&self, offset: usize) -> u32 {
        // SAFETY: the constructor requires a live mapping aligned to four bytes.
        // All callers use a four-byte field wholly within the ring header.
        unsafe { read_volatile(self.base.as_ptr().add(offset).cast::<u32>()) }
    }

    fn write(&mut self, offset: usize, value: u32) {
        // SAFETY: the mapping contract gives this endpoint exclusive ownership
        // of the written header field; the host owns the opposite endpoint.
        unsafe { write_volatile(self.base.as_ptr().add(offset).cast::<u32>(), value) }
    }

    fn used(&self) -> Result<u32, RingError> {
        let used = self.read(PROD).wrapping_sub(self.read(CONS));
        if used > CAPACITY {
            Err(RingError::CorruptCounters)
        } else {
            Ok(used)
        }
    }

    fn item(&self, counter: u32) -> *mut u8 {
        let offset = HEADER_BYTES + (counter & (CAPACITY - 1)) as usize * ITEM_BYTES;
        // SAFETY: the masked index is within the constructor-validated mapping.
        unsafe { self.base.as_ptr().add(offset) }
    }
}

/// Guest producer for a 32-entry command or cursor ring.
#[derive(Debug)]
pub struct CommandRing(Mapping<16, 32>);

impl CommandRing {
    /// # Safety
    /// `base..base+length` must remain live, readable and writable for this
    /// endpoint's lifetime. It must be a little-endian QXL command/cursor ring
    /// in shared RAM with consistent mapping cache attributes. Serialise all guest access through this
    /// endpoint, and stop using it before the device resets or unmaps the RAM.
    /// The host may consume entries and update its consumer/notification fields.
    pub unsafe fn new(base: NonNull<u8>, length: usize) -> Result<Self, RingError> {
        // SAFETY: the caller supplies the mapping and endpoint ownership.
        unsafe { Mapping::new(base, length).map(Self) }
    }

    /// None means full, with a consumer notification armed. Some contains
    /// whether the caller must notify the host after publishing the command.
    pub fn push(&mut self, command: &[u8; 16]) -> Result<Option<bool>, RingError> {
        let ring = &mut self.0;
        if ring.used()? == 32 {
            ring.write(NOTIFY_CONS, ring.read(CONS).wrapping_add(1));
            fence(Ordering::SeqCst);
            if ring.used()? == 32 {
                return Ok(None);
            }
        }
        let producer = ring.read(PROD);
        let item = ring.item(producer);
        for (offset, &byte) in command.iter().enumerate() {
            // SAFETY: this free entry is owned by the guest until PROD advances.
            // Byte stores preserve the packed, potentially unaligned payload.
            unsafe { write_volatile(item.add(offset), byte) };
        }
        fence(Ordering::Release);
        let producer = producer.wrapping_add(1);
        ring.write(PROD, producer);
        fence(Ordering::SeqCst);
        Ok(Some(producer == ring.read(NOTIFY_PROD)))
    }
}

/// Guest consumer for the eight-entry resource release ring.
#[derive(Debug)]
pub struct ReleaseRing(Mapping<8, 8>);

impl ReleaseRing {
    /// # Safety
    /// The mapping and serialisation requirements are those of CommandRing::new,
    /// but this mapping must be the QXL release ring. The host is its producer;
    /// this endpoint exclusively owns the guest consumer/notification fields.
    pub unsafe fn new(base: NonNull<u8>, length: usize) -> Result<Self, RingError> {
        // SAFETY: the caller supplies the mapping and endpoint ownership.
        unsafe { Mapping::new(base, length).map(Self) }
    }

    /// None means empty, with a producer notification armed. Some contains the
    /// released resource ID and whether the caller must notify the host.
    pub fn pop(&mut self) -> Result<Option<(u64, bool)>, RingError> {
        let ring = &mut self.0;
        if ring.used()? == 0 {
            ring.write(NOTIFY_PROD, ring.read(PROD).wrapping_add(1));
            fence(Ordering::SeqCst);
            if ring.used()? == 0 {
                return Ok(None);
            }
        }
        fence(Ordering::Acquire);
        let consumer = ring.read(CONS);
        let item = ring.item(consumer);
        let mut bytes = [0; 8];
        for (offset, byte) in bytes.iter_mut().enumerate() {
            // SAFETY: PROD makes this entry available until CONS advances.
            unsafe { *byte = read_volatile(item.add(offset)) };
        }
        let consumer = consumer.wrapping_add(1);
        fence(Ordering::Release);
        ring.write(CONS, consumer);
        fence(Ordering::SeqCst);
        Ok(Some((
            u64::from_le_bytes(bytes),
            consumer == ring.read(NOTIFY_CONS),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C, align(4))]
    struct Bytes<const N: usize>([u8; N]);

    impl<const N: usize> Bytes<N> {
        fn new(capacity: u32, producer: u32, consumer: u32) -> Self {
            let mut result = Self([0; N]);
            result.0[..4].copy_from_slice(&capacity.to_le_bytes());
            result.0[PROD..PROD + 4].copy_from_slice(&producer.to_le_bytes());
            result.0[CONS..CONS + 4].copy_from_slice(&consumer.to_le_bytes());
            result
        }

        fn base(&mut self) -> NonNull<u8> {
            NonNull::from(&mut self.0[0])
        }
    }

    #[test]
    fn command_publication_wraps_and_notifies() {
        let mut bytes = Bytes::<532>::new(32, u32::MAX, u32::MAX);
        // SAFETY: the aligned test storage remains live and has one guest endpoint.
        let mut ring = unsafe { CommandRing::new(bytes.base(), 532) }.unwrap();
        let command = [0x51; 16];
        assert_eq!(ring.push(&command), Ok(Some(true)));
        assert_eq!(ring.0.read(PROD), 0);
        for index in 0..16 {
            // SAFETY: the final entry lies within the live test mapping.
            let byte = unsafe { read_volatile(ring.0.item(u32::MAX).add(index)) };
            assert_eq!(byte, 0x51);
        }
    }

    #[test]
    fn full_command_ring_arms_notification_without_overwriting() {
        let mut bytes = Bytes::<532>::new(32, 35, 3);
        // SAFETY: test storage is aligned, live and accessed through this endpoint.
        let mut ring = unsafe { CommandRing::new(bytes.base(), 532) }.unwrap();
        assert_eq!(ring.push(&[1; 16]), Ok(None));
        assert_eq!(ring.0.read(PROD), 35);
        assert_eq!(ring.0.read(NOTIFY_CONS), 4);
    }

    #[test]
    fn release_wraps_and_empty_ring_arms_notification() {
        let mut bytes = Bytes::<84>::new(8, 0, u32::MAX);
        bytes.0[HEADER_BYTES + 7 * 8..].copy_from_slice(&0x1122334455667788u64.to_le_bytes());
        // SAFETY: test storage is aligned, live and accessed through this endpoint.
        let mut ring = unsafe { ReleaseRing::new(bytes.base(), 84) }.unwrap();
        assert_eq!(ring.pop(), Ok(Some((0x1122334455667788, true))));
        assert_eq!(ring.0.read(CONS), 0);
        assert_eq!(ring.pop(), Ok(None));
        assert_eq!(ring.0.read(NOTIFY_PROD), 1);
    }

    #[test]
    fn malformed_mappings_and_counters_are_rejected() {
        let mut bytes = Bytes::<532>::new(16, 0, 0);
        // SAFETY: all tested ranges are within the live aligned test allocation.
        unsafe {
            assert!(matches!(
                CommandRing::new(bytes.base(), 532),
                Err(RingError::InvalidCapacity)
            ));
            assert!(matches!(
                CommandRing::new(bytes.base(), 531),
                Err(RingError::InvalidMapping)
            ));
            let unaligned = NonNull::new(bytes.base().as_ptr().add(1)).unwrap();
            assert!(matches!(
                CommandRing::new(unaligned, 531),
                Err(RingError::InvalidMapping)
            ));
        }
        let mut bytes = Bytes::<532>::new(32, 33, 0);
        // SAFETY: the mapping is valid; deliberately corrupt counters are checked.
        let mut ring = unsafe { CommandRing::new(bytes.base(), 532) }.unwrap();
        assert_eq!(ring.push(&[0; 16]), Err(RingError::CorruptCounters));
    }
}
