// Copyright 2026 Bloudraad Ltd. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Read-only stage-2 preparation for sparse tmpfs snapshot mappings.

use std::fs::File;
use std::io;
use std::mem::MaybeUninit;
use std::ops::Range;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;

use kvm_bindings::{KVMIO, kvm_pre_fault_memory};
use kvm_ioctls::VcpuFd;
use vm_memory::{Address, ByteValued, GuestMemoryBackend, GuestMemoryRegion};
use vmm_sys_util::ioctl::ioctl_with_mut_ref;
use vmm_sys_util::ioctl_iowr_nr;
use vmm_sys_util::seek_hole::SeekHole;

use super::memory::GuestMemoryMmap;

ioctl_iowr_nr!(KVM_PRE_FAULT_MEMORY, KVMIO, 0xd5, kvm_pre_fault_memory);

pub(crate) fn kernel_prefault_ranges(file: &File) -> io::Result<Vec<Range<u64>>> {
    use linux_loader::elf::{Elf64_Ehdr, Elf64_Phdr, PF_R, PF_W, PF_X, PT_LOAD};

    let mut magic = [0; 4];
    file.read_exact_at(&mut magic, 0)?;
    if magic != *b"\x7fELF" {
        return Ok(Vec::new());
    }
    let mut header = Elf64_Ehdr::default();
    file.read_exact_at(header.as_mut_slice(), 0)?;
    if header.e_ident[4] != 2
        || header.e_ident[5] != 1
        || usize::from(header.e_phentsize) != size_of::<Elf64_Phdr>()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid ELF program header layout",
        ));
    }
    let mut ranges = Vec::new();
    for index in 0..u64::from(header.e_phnum) {
        let offset = header
            .e_phoff
            .checked_add(index * u64::from(header.e_phentsize))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "ELF program header overflow")
            })?;
        let mut segment = Elf64_Phdr::default();
        file.read_exact_at(segment.as_mut_slice(), offset)?;
        let executable = segment.p_flags & PF_X != 0;
        let read_only = segment.p_flags & (PF_R | PF_W) == PF_R;
        if segment.p_type != PT_LOAD || segment.p_filesz == 0 || !(executable || read_only) {
            continue;
        }
        let end = segment
            .p_paddr
            .checked_add(segment.p_filesz)
            .and_then(|end| end.checked_add(4095))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "ELF executable range overflow")
            })?;
        ranges.push((segment.p_paddr & !4095)..(end & !4095));
    }
    Ok(ranges)
}

pub(crate) fn prepare(
    vcpu: &VcpuFd,
    memory: &GuestMemoryMmap,
    prefault_ranges: &[Range<u64>],
) -> io::Result<()> {
    for region in memory.iter() {
        let Some(backing) = region.file_offset() else {
            continue;
        };
        if !is_tmpfs(backing.file())? {
            continue;
        }
        let mut file = backing.file().try_clone()?;
        for slot in region.plugged_slots() {
            let guest_start = slot.guest_addr.raw_value();
            let guest_end = guest_start + slot.slice.len() as u64;
            for range in prefault_ranges {
                let start = range.start.max(guest_start);
                let end = range.end.min(guest_end);
                if start >= end {
                    continue;
                }
                let file_start = backing.start() + start - region.start_addr().raw_value();
                let file_end = file_start + end - start;
                for_data_range(&mut file, file_start..file_end, |range| {
                    let start = start + range.start - file_start;
                    let size = range.end - range.start;
                    prefault_range(start..start + size, |request| {
                        // SAFETY: The vCPU descriptor is live, the request number
                        // matches its fully initialised, writable 64-byte ABI buffer.
                        let result =
                            unsafe { ioctl_with_mut_ref(vcpu, KVM_PRE_FAULT_MEMORY(), request) };
                        if result < 0 {
                            Err(io::Error::last_os_error())
                        } else {
                            Ok(())
                        }
                    })
                })?;
            }
        }
    }
    Ok(())
}

fn is_tmpfs(file: &File) -> io::Result<bool> {
    let mut stat = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `file` holds a live descriptor and `stat` has space for the
    // complete statfs result. It is read only after a successful syscall.
    if unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: A successful fstatfs initialised the complete structure.
    Ok(i128::from(unsafe { stat.assume_init() }.f_type) == i128::from(libc::TMPFS_MAGIC))
}

fn for_data_range(
    file: &mut File,
    range: Range<u64>,
    mut prepare: impl FnMut(Range<u64>) -> io::Result<()>,
) -> io::Result<()> {
    let mut offset = range.start;
    while offset < range.end {
        let Some(start) = file.seek_data(offset)?.filter(|start| *start < range.end) else {
            break;
        };
        let end = file
            .seek_hole(start)?
            .ok_or_else(|| io::Error::other("snapshot data extent has no end"))?
            .min(range.end);
        if end <= start {
            return Err(io::Error::other("snapshot extent made no progress"));
        }
        prepare(start..end)?;
        offset = end;
    }
    Ok(())
}

fn prefault_range(
    range: Range<u64>,
    mut prepare: impl FnMut(&mut kvm_pre_fault_memory) -> io::Result<()>,
) -> io::Result<()> {
    let mut request = kvm_pre_fault_memory {
        gpa: range.start,
        size: range.end - range.start,
        ..Default::default()
    };
    while request.size != 0 {
        let remaining = request.size;
        prepare(&mut request)?;
        if request.size >= remaining {
            return Err(io::Error::other("KVM prefault made no progress"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sparse_file() -> File {
        let file = memfd::MemfdOptions::default()
            .create("prefault-test")
            .unwrap()
            .into_file();
        file.set_len(12 * 4096).unwrap();
        file.write_all_at(&[1], 2 * 4096).unwrap();
        file.write_all_at(&[1], 7 * 4096).unwrap();
        file
    }

    #[test]
    fn prefault_ranges_select_code_and_read_only_data() {
        use linux_loader::elf::{Elf64_Ehdr, Elf64_Phdr, PF_R, PF_W, PF_X, PT_LOAD};

        let file = sparse_file();
        let header = Elf64_Ehdr {
            e_ident: [0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            e_phoff: size_of::<Elf64_Ehdr>() as u64,
            e_phentsize: size_of::<Elf64_Phdr>() as u16,
            e_phnum: 6,
            ..Default::default()
        };
        file.write_all_at(header.as_slice(), 0).unwrap();
        for (index, flags) in [PF_X, PF_R, PF_R | PF_W, 0, PF_X | PF_W, PF_X]
            .into_iter()
            .enumerate()
        {
            let segment = Elf64_Phdr {
                p_type: PT_LOAD,
                p_flags: flags,
                p_paddr: 0x1001 + index as u64 * 0x10000,
                p_vaddr: 0xffff_ffff_8000_0000,
                p_filesz: if index == 5 { 0 } else { 4096 },
                p_memsz: 3 * 4096,
                ..Default::default()
            };
            file.write_all_at(
                segment.as_slice(),
                header.e_phoff + index as u64 * u64::from(header.e_phentsize),
            )
            .unwrap();
        }
        assert_eq!(
            kernel_prefault_ranges(&file).unwrap(),
            [0x1000..0x3000, 0x11000..0x13000, 0x41000..0x43000]
        );
        file.write_all_at(&[0, 0, 0, 0], 0).unwrap();
        assert!(kernel_prefault_ranges(&file).unwrap().is_empty());
        file.write_all_at(header.as_slice(), 0).unwrap();
        file.write_all_at(&[1], 5).unwrap();
        file.set_len(8).unwrap();
        assert!(kernel_prefault_ranges(&file).is_err());
    }

    #[test]
    fn sparse_extents_respect_slot_bounds() {
        let mut file = sparse_file();
        assert!(is_tmpfs(&file).unwrap());
        let mut ranges = Vec::new();
        for_data_range(&mut file, 4096..8 * 4096, |range| {
            ranges.push(range);
            Ok(())
        })
        .unwrap();
        assert_eq!(ranges, [2 * 4096..3 * 4096, 7 * 4096..8 * 4096]);
        ranges.clear();
        for_data_range(&mut file, 3 * 4096..7 * 4096, |range| {
            ranges.push(range);
            Ok(())
        })
        .unwrap();
        assert!(ranges.is_empty());
    }

    #[test]
    fn prefault_preserves_snapshot_cow_holes_and_dirty_log() {
        use std::os::unix::fs::MetadataExt;

        use vm_memory::{Bytes, GuestAddress, MemoryRegionAddress};

        use crate::vmm_config::machine_config::HugePageConfig;
        use crate::vstate::{kvm::Kvm, memory::snapshot_file, vm::KvmVm};

        let kvm = Kvm::new(vec![]).unwrap();
        if kvm
            .fd
            .check_extension_raw(u64::from(kvm_bindings::KVM_CAP_PRE_FAULT_MEMORY))
            == 0
        {
            return;
        }
        let file = sparse_file();
        let allocated = file.metadata().unwrap().blocks();
        let size = 12 * 4096;
        let regions = snapshot_file(
            file.try_clone().unwrap(),
            [(GuestAddress(0), size)].into_iter(),
            true,
            HugePageConfig::None,
        )
        .unwrap();
        let mut vm = KvmVm::new(kvm).unwrap();
        vm.register_dram_memory_regions(regions).unwrap();
        let vcpu = vm.fd().create_vcpu(0).unwrap();
        prepare(&vcpu, vm.guest_memory(), &[0..size as u64]).unwrap();
        assert_eq!(file.metadata().unwrap().blocks(), allocated);
        assert!(
            vm.fd()
                .get_dirty_log(0, size)
                .unwrap()
                .iter()
                .all(|word| *word == 0)
        );

        let region = vm.guest_memory().iter().next().unwrap();
        region
            .write_slice(&[2], MemoryRegionAddress(2 * 4096))
            .unwrap();
        let mut original = [0];
        file.read_exact_at(&mut original, 2 * 4096).unwrap();
        assert_eq!(original, [1]);
        let mut changed = [0];
        region
            .read_slice(&mut changed, MemoryRegionAddress(2 * 4096))
            .unwrap();
        assert_eq!(changed, [2]);
        assert_eq!(file.metadata().unwrap().blocks(), allocated);
    }

    #[test]
    fn kernel_partial_progress_is_completed() {
        let mut calls = 0;
        prefault_range(4096..4 * 4096, |request| {
            assert_eq!(request.flags, 0);
            assert_eq!(request.padding, [0; 5]);
            assert_eq!(request.gpa, 4096 + calls * 4096);
            request.gpa += 4096;
            request.size -= 4096;
            calls += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(calls, 3);
        assert!(prefault_range(0..4096, |_| Ok(())).is_err());
    }
}
