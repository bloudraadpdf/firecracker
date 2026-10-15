// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Bounded CloudHv SMBIOS system-identity handoff.

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;

use vm_memory::{Bytes, GuestAddress, GuestMemoryBackend};

use crate::arch::BootProtocol;
use crate::vmm_config::boot_source::{FirmwareConfig, SystemUuid};
use crate::vstate::memory::GuestMemoryMmap;

pub(super) const BASE: u64 = 0xf0000;
const PAGE_SIZE: usize = 4096;
pub(super) const SIZE: u64 = PAGE_SIZE as u64;
const TABLE_OFFSET: usize = 32;
const ENTRY_LENGTH: usize = 24;
const MAX_PROGRAM_HEADERS: u16 = 128;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

/// Reject ELF load segments that could collide with the fixed firmware handoff.
/// linux-loader keeps its ELF header types private; it validates the PVH note when loading.
pub(crate) fn validate_image(file: &File) -> io::Result<()> {
    let size = file.metadata()?.len();
    let mut header = [0; 64];
    file.read_exact_at(&mut header, 0)?;
    if &header[..7] != b"\x7fELF\x02\x01\x01"
        || u16_at(&header, 16) != 2
        || !matches!(u16_at(&header, 18), 3 | 62)
        || u32_at(&header, 20) != 1
        || u16_at(&header, 52) != 64
        || u16_at(&header, 54) != 56
    {
        return Err(invalid("CloudHv identity requires an x86 ELF64 image"));
    }
    let offset = u64_at(&header, 32);
    let count = u16_at(&header, 56);
    if !(1..=MAX_PROGRAM_HEADERS).contains(&count)
        || offset < 64
        || offset
            .checked_add(u64::from(count) * 56)
            .is_none_or(|end| end > size)
    {
        return Err(invalid("invalid CloudHv ELF program-header bounds"));
    }
    let mut loads = 0;
    for index in 0..count {
        let mut program = [0; 56];
        file.read_exact_at(&mut program, offset + u64::from(index) * 56)?;
        if u32_at(&program, 0) != 1 {
            continue;
        }
        loads += 1;
        let start = u64_at(&program, 24);
        let memory_size = u64_at(&program, 40);
        let file_size = u64_at(&program, 32);
        let end = start
            .checked_add(memory_size)
            .ok_or_else(|| invalid("ELF load address wraps"))?;
        if file_size > memory_size
            || u64_at(&program, 8)
                .checked_add(file_size)
                .is_none_or(|end| end > size)
        {
            return Err(invalid("invalid CloudHv ELF load bounds"));
        }
        if start < BASE + SIZE && end > BASE {
            return Err(invalid("CloudHv ELF load overlaps SMBIOS reservation"));
        }
    }
    if loads == 0 {
        return Err(invalid("CloudHv ELF has no load segments"));
    }
    Ok(())
}

fn encode(uuid: SystemUuid) -> [u8; PAGE_SIZE] {
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
    page[16..24].copy_from_slice(&(BASE + u64::try_from(TABLE_OFFSET).unwrap()).to_le_bytes());
    page[5] = 0u8.wrapping_sub(
        page[..ENTRY_LENGTH]
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
    );
    page
}

/// Publish only on a cold boot whose caller explicitly selected the CloudHv ABI.
pub(crate) fn configure(
    memory: &GuestMemoryMmap,
    firmware: Option<&FirmwareConfig>,
    protocol: BootProtocol,
) -> io::Result<()> {
    let Some(firmware) = firmware else {
        return Ok(());
    };
    if protocol != BootProtocol::PvhBoot {
        return Err(invalid("CloudHv identity requires PVH boot"));
    }
    let page = encode(firmware.system_uuid);
    if !memory.check_range(GuestAddress(BASE), page.len()) {
        return Err(invalid("SMBIOS reservation is outside guest RAM"));
    }
    memory
        .write_slice(&page, GuestAddress(BASE))
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmm_config::boot_source::FirmwareType;
    use vmm_sys_util::tempfile::TempFile;

    fn firmware() -> FirmwareConfig {
        FirmwareConfig {
            profile: FirmwareType::CloudHv,
            system_uuid: "11223344-5566-7788-99aa-bbccddeeff01"
                .to_string()
                .try_into()
                .unwrap(),
        }
    }

    #[test]
    fn identity_table_has_valid_checksum_uuid_strings_and_end_marker() {
        let page = encode(firmware().system_uuid);
        assert_eq!(&page[..5], b"_SM3_");
        assert_eq!(page[6], 24);
        assert_eq!(
            page[..ENTRY_LENGTH]
                .iter()
                .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
            0
        );
        assert_eq!(u64_at(&page, 16), BASE + 32);
        let size = usize::try_from(u32_at(&page, 12)).unwrap();
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
    fn profile_is_required_and_non_pvh_never_overwrites_memory() {
        let memory = crate::test_utils::single_region_mem_at(BASE, PAGE_SIZE);
        let sentinel = [0xa5; PAGE_SIZE];
        memory.write_slice(&sentinel, GuestAddress(BASE)).unwrap();
        for protocol in [BootProtocol::PvhBoot, BootProtocol::LinuxBoot] {
            configure(&memory, None, protocol).unwrap();
        }
        configure(&memory, Some(&firmware()), BootProtocol::LinuxBoot).unwrap_err();
        let mut actual = [0; PAGE_SIZE];
        memory.read_slice(&mut actual, GuestAddress(BASE)).unwrap();
        assert_eq!(actual, sentinel);
        configure(&memory, Some(&firmware()), BootProtocol::PvhBoot).unwrap();
        memory.read_slice(&mut actual, GuestAddress(BASE)).unwrap();
        assert_eq!(actual, encode(firmware().system_uuid));
        let short = crate::test_utils::single_region_mem_at(BASE, PAGE_SIZE - 1);
        configure(&short, Some(&firmware()), BootProtocol::PvhBoot).unwrap_err();
    }

    fn image(start: u64, file_size: u64, memory_size: u64) -> Vec<u8> {
        let mut bytes = vec![0; 128];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
        bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
        bytes[64..68].copy_from_slice(&1u32.to_le_bytes());
        bytes[72..80].copy_from_slice(&120u64.to_le_bytes());
        bytes[88..96].copy_from_slice(&start.to_le_bytes());
        bytes[96..104].copy_from_slice(&file_size.to_le_bytes());
        bytes[104..112].copy_from_slice(&memory_size.to_le_bytes());
        bytes
    }

    fn check_image(bytes: &[u8]) -> io::Result<()> {
        let file = TempFile::new_in(std::path::Path::new(".")).unwrap();
        file.as_file().write_all_at(bytes, 0).unwrap();
        validate_image(file.as_file())
    }

    #[test]
    fn elf_overlap_includes_zero_fill_and_checks_both_boundaries() {
        for (start, size) in [(BASE - 1, 1), (BASE + SIZE, 8), (0x100000, 8)] {
            check_image(&image(start, 1, size)).unwrap();
        }
        for (start, size) in [
            (BASE, 1),
            (BASE - 1, 2),
            (BASE + SIZE - 1, 1),
            (BASE - 8, SIZE + 16),
        ] {
            check_image(&image(start, 0, size)).unwrap_err();
        }
    }

    #[test]
    fn cloudhv_elf64_with_32bit_pvh_entry_loads_before_identity_publication() {
        let mut bytes = image(0x100000, 0x400000, 0x400000);
        bytes.resize(0x400000, 0);
        bytes[7] = 3;
        bytes[18..20].copy_from_slice(&3u16.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x4fffd0u64.to_le_bytes());
        bytes[56..58].copy_from_slice(&2u16.to_le_bytes());
        bytes[68..72].copy_from_slice(&7u32.to_le_bytes());
        bytes[72..80].fill(0);
        bytes[80..88].copy_from_slice(&0x100000u64.to_le_bytes());
        bytes[112..120].copy_from_slice(&4u64.to_le_bytes());
        bytes[120..128].copy_from_slice(&[4, 0, 0, 0, 4, 0, 0, 0]);
        for (offset, value) in [
            (128, 176u64),
            (136, 0x1000b0),
            (144, 0x1000b0),
            (152, 20),
            (160, 20),
            (168, 4),
        ] {
            bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        bytes[176..196].copy_from_slice(&[
            4, 0, 0, 0, 4, 0, 0, 0, 18, 0, 0, 0, b'X', b'e', b'n', 0, 0xd0, 0xff, 0x4f, 0,
        ]);
        let file = TempFile::new_in(std::path::Path::new(".")).unwrap();
        for machine in [3u16, 62] {
            bytes[18..20].copy_from_slice(&machine.to_le_bytes());
            file.as_file().write_all_at(&bytes, 0).unwrap();
            validate_image(file.as_file()).unwrap();
            let memory = crate::test_utils::single_region_mem_at(0, 0x500000);
            let entry = crate::arch::load_kernel(file.as_file(), &memory).unwrap();
            assert_eq!(entry.protocol, BootProtocol::PvhBoot);
            assert_eq!(entry.entry_addr, GuestAddress(0x4fffd0));
            configure(&memory, Some(&firmware()), entry.protocol).unwrap();
        }
    }

    #[test]
    fn elf_headers_and_load_bounds_are_bounded_before_publication() {
        let valid = image(0x100000, 8, 8);
        for (offset, bytes) in [
            (0, vec![0]),
            (4, vec![1]),
            (18, 183u16.to_le_bytes().to_vec()),
            (32, u64::MAX.to_le_bytes().to_vec()),
            (54, 55u16.to_le_bytes().to_vec()),
            (56, 129u16.to_le_bytes().to_vec()),
            (64, 0u32.to_le_bytes().to_vec()),
        ] {
            let mut bad = valid.clone();
            bad[offset..offset + bytes.len()].copy_from_slice(&bytes);
            check_image(&bad).unwrap_err();
        }
        check_image(&image(u64::MAX - 4, 8, 8)).unwrap_err();
        check_image(&image(0x100000, 9, 8)).unwrap_err();
        check_image(&image(0x100000, 9, 9)).unwrap_err();
        check_image(&valid[..119]).unwrap_err();
    }
}
