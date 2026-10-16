// Copyright 2024 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright 2023 Rivos, Inc.
//
// SPDX-License-Identifier: Apache-2.0

use std::mem::size_of;

use vm_memory::{Address, Bytes, GuestAddress, GuestMemory};
use zerocopy::IntoBytes;

use crate::{AcpiError, Result, Sdt, SdtHeader, checksum};

/// Root System Description Table with 32-bit or 64-bit table addresses.
#[derive(Clone, Default, Debug)]
pub struct RootTable {
    header: SdtHeader,
    tables: Vec<u8>,
}

#[derive(Debug)]
pub enum RootTableEntries {
    Rsdt(Vec<u32>),
    Xsdt(Vec<u64>),
}

impl RootTable {
    pub fn new(
        oem_id: [u8; 6],
        oem_table_id: [u8; 8],
        oem_revision: u32,
        tables: RootTableEntries,
    ) -> Self {
        let (signature, tables_bytes): (_, Vec<u8>) = match tables {
            RootTableEntries::Rsdt(addresses) => (
                *b"RSDT",
                addresses.into_iter().flat_map(u32::to_le_bytes).collect(),
            ),
            RootTableEntries::Xsdt(addresses) => (
                *b"XSDT",
                addresses.into_iter().flat_map(u64::to_le_bytes).collect(),
            ),
        };

        let header = SdtHeader::new(
            signature,
            (std::mem::size_of::<SdtHeader>() + tables_bytes.len())
                .try_into()
                .unwrap(),
            1,
            oem_id,
            oem_table_id,
            oem_revision,
        );

        let mut table = RootTable {
            header,
            tables: tables_bytes,
        };

        table.header.checksum = checksum(&[table.header.as_bytes(), (table.tables.as_slice())]);

        table
    }
}

impl Sdt for RootTable {
    fn len(&self) -> usize {
        std::mem::size_of::<SdtHeader>() + self.tables.len()
    }

    fn write_to_guest<M: GuestMemory>(&mut self, mem: &M, address: GuestAddress) -> Result<()> {
        mem.write_slice(self.header.as_bytes(), address)?;
        let address = address
            .checked_add(size_of::<SdtHeader>() as u64)
            .ok_or(AcpiError::InvalidGuestAddress)?;
        mem.write_slice(self.tables.as_slice(), address)?;
        Ok(())
    }
}
