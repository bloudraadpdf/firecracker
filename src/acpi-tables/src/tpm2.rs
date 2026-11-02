// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use vm_memory::{Bytes, GuestAddress, GuestMemory};
use zerocopy::little_endian::{U32, U64};
use zerocopy::{Immutable, IntoBytes};

use crate::{Result, Sdt, SdtHeader, checksum};

/// TCG ACPI TPM2 revision 3, command-response buffer start method.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, IntoBytes, Immutable)]
pub struct Tpm2 {
    header: SdtHeader,
    reserved: U32,
    control_area: U64,
    start_method: U32,
}

impl Tpm2 {
    pub fn new(
        oem_id: [u8; 6],
        oem_table_id: [u8; 8],
        oem_revision: u32,
        control_area: u64,
    ) -> Self {
        let mut table = Self {
            header: SdtHeader::new(*b"TPM2", 52, 3, oem_id, oem_table_id, oem_revision),
            reserved: U32::new(0),
            control_area: U64::new(control_area),
            start_method: U32::new(7),
        };
        table.header.checksum = checksum(&[table.as_bytes()]);
        table
    }
}

impl Sdt for Tpm2 {
    fn len(&self) -> usize {
        self.as_bytes().len()
    }

    fn write_to_guest<M: GuestMemory>(&mut self, mem: &M, address: GuestAddress) -> Result<()> {
        mem.write_slice(self.as_bytes(), address)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crb_table_has_exact_wire_layout_and_checksum() {
        let table = Tpm2::new(*b"FIRECK", *b"FCVMTPM2", 0, 0xfed4_0040);
        let bytes = table.as_bytes();
        assert_eq!(bytes.len(), 52);
        assert_eq!(&bytes[..4], b"TPM2");
        assert_eq!(&bytes[4..8], &52u32.to_le_bytes());
        assert_eq!(bytes[8], 3);
        assert_eq!(&bytes[36..40], &[0; 4]);
        assert_eq!(&bytes[40..48], &0xfed4_0040u64.to_le_bytes());
        assert_eq!(&bytes[48..52], &7u32.to_le_bytes());
        assert_eq!(
            bytes
                .iter()
                .fold(0u8, |sum, value| sum.wrapping_add(*value)),
            0
        );
    }
}
