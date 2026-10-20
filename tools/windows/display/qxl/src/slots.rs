// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Shared miniport/display memory-slot address conversion.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotError {
    InvalidBitCount,
    InvalidId,
    InvalidGeneration,
    InvalidRange,
    WrongSlot,
    OutOfBounds,
}

#[derive(Clone, Copy, Debug)]
pub struct SlotLayout {
    id_bits: u8,
    generation_bits: u8,
}

impl SlotLayout {
    pub fn new(id_bits: u8, generation_bits: u8) -> Result<Self, SlotError> {
        // Both values come from byte-sized ROM fields and identify byte-sized IDs.
        if !(1..=8).contains(&id_bits) || !(1..=8).contains(&generation_bits) {
            return Err(SlotError::InvalidBitCount);
        }
        Ok(Self {
            id_bits,
            generation_bits,
        })
    }

    pub fn slot(
        self,
        id: u8,
        generation: u8,
        physical_start: u64,
        virtual_start: u64,
        length: u64,
    ) -> Result<MemorySlot, SlotError> {
        if u16::from(id) >= 1u16 << self.id_bits {
            return Err(SlotError::InvalidId);
        }
        if u16::from(generation) >= 1u16 << self.generation_bits {
            return Err(SlotError::InvalidGeneration);
        }
        let offset_bits = 64 - self.id_bits - self.generation_bits;
        let offset_mask = u64::MAX >> (self.id_bits + self.generation_bits);
        if length == 0 || length - 1 > offset_mask {
            return Err(SlotError::InvalidRange);
        }
        let physical_end = physical_start
            .checked_add(length)
            .ok_or(SlotError::InvalidRange)?;
        virtual_start
            .checked_add(length)
            .ok_or(SlotError::InvalidRange)?;
        let high_bits =
            ((u64::from(id) << self.generation_bits) | u64::from(generation)) << offset_bits;
        Ok(MemorySlot {
            high_bits,
            offset_mask,
            physical_start,
            physical_end,
            virtual_start,
            length,
        })
    }
}

/// A validated slot. Its host-facing end address is exclusive.
#[derive(Clone, Copy, Debug)]
pub struct MemorySlot {
    high_bits: u64,
    offset_mask: u64,
    physical_start: u64,
    physical_end: u64,
    virtual_start: u64,
    length: u64,
}

impl MemorySlot {
    pub fn physical_range(self) -> (u64, u64) {
        (self.physical_start, self.physical_end)
    }

    /// Port of PA: encode a guest virtual address as a QXLPHYSICAL value.
    pub fn encode(self, address: u64, bytes: u64) -> Result<u64, SlotError> {
        let offset = address
            .checked_sub(self.virtual_start)
            .ok_or(SlotError::OutOfBounds)?;
        self.check_range(offset, bytes)?;
        Ok(self.high_bits | offset)
    }

    /// Port of VA: reject another slot or generation before resolving its offset.
    pub fn decode(self, address: u64, bytes: u64) -> Result<u64, SlotError> {
        if address & !self.offset_mask != self.high_bits {
            return Err(SlotError::WrongSlot);
        }
        let offset = address & self.offset_mask;
        self.check_range(offset, bytes)?;
        self.virtual_start
            .checked_add(offset)
            .ok_or(SlotError::OutOfBounds)
    }

    fn check_range(self, offset: u64, bytes: u64) -> Result<(), SlotError> {
        if bytes == 0
            || offset
                .checked_add(bytes)
                .is_none_or(|end| end > self.length)
        {
            Err(SlotError::OutOfBounds)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot() -> MemorySlot {
        SlotLayout::new(8, 8)
            .unwrap()
            .slot(1, 7, 0xe0000000, 0x90000000, 4096)
            .unwrap()
    }

    #[test]
    fn wire_addresses_match_qxl_bit_layout() {
        let slot = slot();
        assert_eq!(slot.physical_range(), (0xe0000000, 0xe0001000));
        assert_eq!(slot.encode(0x90000000, 4096), Ok(0x0107000000000000));
        assert_eq!(slot.encode(0x90000fff, 1), Ok(0x0107000000000fff));
        assert_eq!(slot.decode(0x0107000000000fff, 1), Ok(0x90000fff));
    }

    #[test]
    fn reject_cross_slot_and_stale_generation_addresses() {
        assert_eq!(
            slot().decode(0x0207000000000000, 1),
            Err(SlotError::WrongSlot)
        );
        assert_eq!(
            slot().decode(0x0108000000000000, 1),
            Err(SlotError::WrongSlot)
        );
    }

    #[test]
    fn validate_full_buffer_not_only_start_address() {
        let slot = slot();
        for (address, bytes) in [
            (0x8fffffff, 1),
            (0x90000000, 0),
            (0x90000fff, 2),
            (0x90001000, 1),
            (0x90000001, u64::MAX),
        ] {
            assert_eq!(slot.encode(address, bytes), Err(SlotError::OutOfBounds));
        }
        assert_eq!(
            slot.decode(0x0107000000000fff, 2),
            Err(SlotError::OutOfBounds)
        );
    }

    #[test]
    fn reject_unrepresentable_layouts_and_mappings() {
        for (id_bits, generation_bits) in [(0, 8), (8, 0), (9, 8), (8, 255)] {
            assert!(SlotLayout::new(id_bits, generation_bits).is_err());
        }
        let layout = SlotLayout::new(1, 1).unwrap();
        assert!(matches!(
            layout.slot(2, 0, 0, 0, 1),
            Err(SlotError::InvalidId)
        ));
        assert!(matches!(
            layout.slot(0, 2, 0, 0, 1),
            Err(SlotError::InvalidGeneration)
        ));
        for (physical, virtual_address, length) in [
            (0, 0, 0),
            (u64::MAX, 0, 1),
            (0, u64::MAX, 1),
            (0, 0, (1 << 62) + 1),
        ] {
            assert!(matches!(
                layout.slot(0, 0, physical, virtual_address, length),
                Err(SlotError::InvalidRange)
            ));
        }
        let slot = layout.slot(1, 1, 0, 0, 1).unwrap();
        assert_eq!(slot.encode(0, 1), Ok(0xc000000000000000));
    }
}
