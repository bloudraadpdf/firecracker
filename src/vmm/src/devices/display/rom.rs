// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#[cfg(feature = "windows")]
use std::io::{self, Read};
#[cfg(feature = "windows")]
use std::path::Path;

use serde::{Deserialize, Serialize};
#[cfg(feature = "windows")]
use vm_allocator::{AddressAllocator, AllocPolicy, RangeInclusive};

#[cfg(feature = "windows")]
use crate::vstate::vm::KvmVm;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ExpansionRom {
    pub(crate) bytes: Vec<u8>,
    pub(crate) mapped: u64,
    pub(crate) bar: u32,
    pub(crate) probe: bool,
}

#[cfg(feature = "windows")]
pub(crate) fn validate_rom(bytes: &[u8]) -> io::Result<()> {
    if bytes.len() < 512
        || bytes.len() > 0x10000
        || bytes[..2] != [0x55, 0xaa]
        || usize::from(bytes[2]) * 512 != bytes.len()
        || bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) != 0
    {
        return Err(io::Error::other("invalid VGA option ROM"));
    }
    Ok(())
}

#[cfg(feature = "windows")]
impl ExpansionRom {
    pub(crate) fn load(path: &Path) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(0x1_0001)
            .read_to_end(&mut bytes)?;
        validate_rom(&bytes)?;
        Ok(bytes)
    }

    pub(crate) fn allocate(bytes: Vec<u8>, allocator: &mut AddressAllocator) -> io::Result<Self> {
        validate_rom(&bytes)?;
        let size = bytes.len().next_power_of_two().max(2048) as u64;
        let mapped = allocator
            .allocate(size, size, AllocPolicy::FirstMatch)
            .map_err(io::Error::other)?
            .start();
        Ok(Self {
            bytes,
            mapped,
            bar: u32::try_from(mapped).map_err(io::Error::other)?,
            probe: false,
        })
    }

    pub(crate) fn region(&self) -> (u64, u64) {
        (
            self.mapped,
            self.bytes.len().next_power_of_two().max(2048) as u64,
        )
    }

    pub(crate) fn read_bar(&self) -> u32 {
        if self.probe {
            !(u32::try_from(self.region().1).unwrap() - 1) & !0x7ff
        } else {
            self.bar
        }
    }

    pub(crate) fn write_bar(&mut self, offset: u8, bytes: &[u8]) {
        if usize::from(offset) + bytes.len() > 4 {
            return;
        }
        let mut value = self.bar.to_le_bytes();
        value[usize::from(offset)..usize::from(offset) + bytes.len()].copy_from_slice(bytes);
        let value = u32::from_le_bytes(value);
        self.probe = value & !0x7ff == 0xffff_f800;
        if !self.probe {
            self.bar = value & (!(u32::try_from(self.region().1).unwrap() - 1) | 1);
        }
    }

    pub(crate) fn read(&self, base: u64, offset: u64, bytes: &mut [u8]) {
        if base != self.mapped || self.bar & 1 == 0 {
            return;
        }
        if let Ok(start) = usize::try_from(offset) {
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = start
                    .checked_add(index)
                    .and_then(|at| self.bytes.get(at))
                    .copied()
                    .unwrap_or(0xff);
            }
        }
    }

    pub(crate) fn relocate(&mut self, vm: &KvmVm) {
        if self.probe || self.bar & 1 == 0 {
            return;
        }
        let new = u64::from(self.bar & !0x7ff);
        let (old, size) = self.region();
        if new == old {
            return;
        }
        let mut allocator = vm.resource_allocator();
        let moved = allocator
            .mmio32_memory
            .allocate(size, size, AllocPolicy::ExactMatch(new))
            .is_ok_and(|range| {
                if vm.common.mmio_bus.move_range(old, new, size).is_err() {
                    let _ = allocator.mmio32_memory.free(&range);
                    false
                } else {
                    let _ = allocator
                        .mmio32_memory
                        .free(&RangeInclusive::new(old, old + size - 1).unwrap());
                    true
                }
            });
        if moved {
            self.mapped = new;
        } else {
            self.bar = u32::try_from(old).unwrap() | (self.bar & 1);
        }
    }
}
