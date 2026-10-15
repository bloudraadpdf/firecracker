// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use vm_memory::{Bytes, GuestAddress, GuestMemory};

use crate::{Result, Sdt};

/// Firmware ACPI Control Structure for an S0/S5-only platform.
#[derive(Debug)]
pub struct Facs;

impl Sdt for Facs {
    fn alignment(&self) -> u64 {
        64
    }

    fn len(&self) -> usize {
        64
    }

    fn write_to_guest<M: GuestMemory>(&mut self, mem: &M, address: GuestAddress) -> Result<()> {
        let mut bytes = [0u8; 64];
        bytes[..4].copy_from_slice(b"FACS");
        bytes[4..8].copy_from_slice(&64u32.to_le_bytes());
        bytes[32] = 2;
        mem.write_slice(&bytes, address)?;
        Ok(())
    }
}
