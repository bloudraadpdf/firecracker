// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! SMBIOS system identity shared by CloudHv firmware profiles.

use std::io;

use vm_memory::{Bytes, GuestAddress, GuestMemoryBackend};

use crate::vmm_config::boot_source::SystemUuid;
use crate::vstate::memory::GuestMemoryMmap;

pub(super) const PAGE_SIZE: usize = 4096;
const TABLE_OFFSET: usize = 32;
const ENTRY_LENGTH: usize = 24;

pub(super) fn encode(uuid: SystemUuid, base: u64) -> [u8; PAGE_SIZE] {
    let mut page = [0; PAGE_SIZE];
    let record = &mut page[TABLE_OFFSET..TABLE_OFFSET + 27];
    record[..8].copy_from_slice(&[1, 27, 0, 1, 1, 2, 0, 0]);
    record[8..24].copy_from_slice(&uuid.smbios_bytes());
    record[24] = 2;
    let strings = b"Firecracker\0Firecracker microVM\0\0";
    let strings_start = TABLE_OFFSET + 27;
    let end_marker = strings_start + strings.len();
    page[strings_start..end_marker].copy_from_slice(strings);
    page[end_marker..end_marker + 6].copy_from_slice(&[127, 4, 255, 127, 0, 0]);
    let table_size = u32::try_from(end_marker + 6 - TABLE_OFFSET).unwrap();
    page[..5].copy_from_slice(b"_SM3_");
    page[6..12].copy_from_slice(&[24, 3, 0, 0, 1, 0]);
    page[12..16].copy_from_slice(&table_size.to_le_bytes());
    page[16..24].copy_from_slice(&(base + u64::try_from(TABLE_OFFSET).unwrap()).to_le_bytes());
    page[5] = 0u8.wrapping_sub(
        page[..ENTRY_LENGTH]
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
    );
    page
}

pub(super) fn publish(memory: &GuestMemoryMmap, uuid: SystemUuid, base: u64) -> io::Result<()> {
    let page = encode(uuid, base);
    if !memory.check_range(GuestAddress(base), page.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SMBIOS reservation is outside guest RAM",
        ));
    }
    memory
        .write_slice(&page, GuestAddress(base))
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid() -> SystemUuid {
        "11223344-5566-7788-99aa-bbccddeeff01"
            .to_string()
            .try_into()
            .unwrap()
    }

    #[test]
    fn identity_table_has_valid_checksum_uuid_strings_and_end_marker() {
        let base = 0x801e0000;
        let page = encode(uuid(), base);
        assert_eq!(&page[..5], b"_SM3_");
        assert_eq!(page[6], 24);
        assert_eq!(
            page[..ENTRY_LENGTH]
                .iter()
                .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
            0
        );
        assert_eq!(
            u64::from_le_bytes(page[16..24].try_into().unwrap()),
            base + 32
        );
        let size = usize::try_from(u32::from_le_bytes(page[12..16].try_into().unwrap())).unwrap();
        assert!(size + TABLE_OFFSET <= PAGE_SIZE);
        let table = &page[TABLE_OFFSET..TABLE_OFFSET + size];
        assert_eq!(&table[..8], &[1, 27, 0, 1, 1, 2, 0, 0]);
        assert_eq!(
            &table[8..24],
            &[
                0x44, 0x33, 0x22, 0x11, 0x66, 0x55, 0x88, 0x77, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
                0xff, 1
            ]
        );
        let end = table.len() - 6;
        assert_eq!(&table[27..end], b"Firecracker\0Firecracker microVM\0\0");
        assert_eq!(&table[end..], &[127, 4, 255, 127, 0, 0]);
        assert!(page[TABLE_OFFSET + size..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn publication_keeps_each_platform_pointer_within_its_reserved_page() {
        for base in [0xf0000, 0x801e0000] {
            let memory = crate::test_utils::single_region_mem_at(base, PAGE_SIZE);
            publish(&memory, uuid(), base).unwrap();
            let mut actual = [0; PAGE_SIZE];
            memory.read_slice(&mut actual, GuestAddress(base)).unwrap();
            assert_eq!(actual, encode(uuid(), base));
            let short = crate::test_utils::single_region_mem_at(base, PAGE_SIZE - 1);
            publish(&short, uuid(), base).unwrap_err();
            let mut unchanged = vec![0xff; PAGE_SIZE - 1];
            short
                .read_slice(&mut unchanged, GuestAddress(base))
                .unwrap();
            assert!(unchanged.iter().all(|byte| *byte == 0));
        }
    }
}
