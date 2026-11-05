// Copyright 2024 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use kvm_ioctls::VcpuFd;
use vm_memory::{Bytes, GuestAddress, GuestMemoryError};

use super::regs::{ID_AA64MMFR0_EL1, SCTLR_EL1, TCR_EL1, TTBR1_EL1};
use crate::vstate::memory::GuestMemoryMmap;

/// Errors when reading an ARM64 kernel virtual address.
#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum PagingError {
    /// Cannot read register {id:#x}: {source}
    Register { id: u64, source: kvm_ioctls::Error },
    /// Cannot read guest memory: {0}
    Memory(#[from] GuestMemoryError),
    /// Unsupported or invalid ARM64 kernel page tables
    PageTable,
}

pub(crate) fn register(vcpu: &VcpuFd, id: u64) -> Result<u64, PagingError> {
    let mut bytes = [0; 8];
    vcpu.get_one_reg(id, &mut bytes)
        .map_err(|source| PagingError::Register { id, source })?;
    Ok(u64::from_le_bytes(bytes))
}

pub(crate) fn translate_kernel_address(
    vcpu: &VcpuFd,
    memory: &GuestMemoryMmap,
    address: u64,
) -> Result<u64, PagingError> {
    if register(vcpu, SCTLR_EL1)? & 1 == 0 {
        return Ok(address);
    }
    let tcr = register(vcpu, TCR_EL1)?;
    if address & (1 << 55) == 0 || (tcr >> 30) & 3 != 2 {
        return Err(PagingError::PageTable);
    }
    let pa_size = match register(vcpu, ID_AA64MMFR0_EL1)? & 15 {
        0 => 32,
        1 => 36,
        2 => 40,
        3 => 42,
        4 => 44,
        5 => 48,
        _ => return Err(PagingError::PageTable),
    };
    translate_4k(
        address,
        register(vcpu, TTBR1_EL1)?,
        (tcr >> 16) & 63,
        pa_size,
        |address| Ok(u64::from_le(memory.read_obj(GuestAddress(address))?)),
    )
}

fn translate_4k(
    gva: u64,
    ttbr: u64,
    t1sz: u64,
    pa_size: u32,
    read: impl Fn(u64) -> Result<u64, PagingError>,
) -> Result<u64, PagingError> {
    if !(16..=39).contains(&t1sz) {
        return Err(PagingError::PageTable);
    }
    let va_bits = 64 - t1sz;
    let start_level = 4 - (va_bits - 12).div_ceil(9);
    let root_bits = va_bits - (12 + 9 * (3 - start_level));
    let pa_mask = (1_u64 << pa_size) - 1;
    let mut table = ttbr & pa_mask & !((1 << (root_bits + 3)) - 1);
    for level in start_level..=3 {
        let shift = 12 + 9 * (3 - level);
        let index_bits = if level == start_level { root_bits } else { 9 };
        let index = (gva >> shift) & ((1 << index_bits) - 1);
        let descriptor = read(table + index * 8)?;
        match (descriptor & 3, level) {
            (3, 0..=2) => table = descriptor & pa_mask & !0xfff,
            (1, 1..=2) | (3, 3) => {
                let offset_mask = (1 << shift) - 1;
                return Ok((descriptor & pa_mask & !offset_mask) | (gva & offset_mask));
            }
            _ => return Err(PagingError::PageTable),
        }
    }
    Err(PagingError::PageTable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translation_is_physical_with_the_mmu_disabled() {
        let vm = crate::vstate::vm::tests::setup_vm_with_memory(0x4000);
        let mut vcpu = crate::arch::aarch64::vcpu::KvmVcpu::new(0, &vm).unwrap();
        vcpu.init(&[]).unwrap();
        vcpu.fd
            .set_one_reg(SCTLR_EL1, &0_u64.to_le_bytes())
            .unwrap();
        assert_eq!(
            translate_kernel_address(&vcpu.fd, vm.guest_memory(), 0x1234).unwrap(),
            0x1234
        );
    }

    #[test]
    fn translate_47_and_48_bit_addresses() {
        let entries = [
            (0x805aaf80, 0x1003),
            (0x1010, 0x2003),
            (0x2f48, 0x3003),
            (0x3008, 0x9003),
        ];
        for (ttbr, t1sz) in [(0x10000805aa800, 17), (0x10000805aa000, 16)] {
            let address = translate_4k(0xfffff800bd201234, ttbr, t1sz, 48, |address| {
                entries
                    .iter()
                    .find(|(entry, _)| *entry == address)
                    .map(|(_, value)| *value)
                    .ok_or(PagingError::PageTable)
            })
            .unwrap();
            assert_eq!(address, 0x9234);
        }
    }

    #[test]
    fn translate_blocks_and_invalid_descriptors() {
        assert_eq!(
            translate_4k(0xffffff8000001234, 0x1000, 25, 48, |address| {
                assert_eq!(address, 0x1000);
                Ok(0x80000001)
            })
            .unwrap(),
            0x80001234
        );
        assert_eq!(
            translate_4k(0xffffffffff345678, 0x1000, 39, 48, |address| {
                assert_eq!(address, 0x1048);
                Ok(0x80200001)
            })
            .unwrap(),
            0x80345678
        );
        for descriptor in [0, 1, 2] {
            translate_4k(0xfffff800bd201234, 0x1000, 16, 48, |_| Ok(descriptor)).unwrap_err();
        }
        for t1sz in [0, 15, 40, 64] {
            translate_4k(0, 0, t1sz, 48, |_| unreachable!()).unwrap_err();
        }
    }
}
