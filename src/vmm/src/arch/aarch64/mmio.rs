// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! ARM64 kernel MMIO loads without a valid KVM instruction syndrome.

use std::mem::offset_of;

use kvm_bindings::{KVM_REG_ARM_CORE, KVM_REG_ARM64, KVM_REG_SIZE_U64, kvm_regs, user_pt_regs};
use kvm_ioctls::VcpuFd;
use vm_memory::{Bytes, GuestAddress, GuestMemoryError};

use super::paging;
use super::regs::{SCTLR_EL1, arm64_core_reg_id};
use crate::vstate::bus::{Bus, BusError};
use crate::vstate::memory::GuestMemoryMmap;

#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub(crate) enum MmioError {
    /// Cannot translate the guest instruction or operand: {0}
    Paging(#[from] paging::PagingError),
    /// Cannot read the guest instruction: {0}
    Memory(#[from] GuestMemoryError),
    /// Unsupported instruction {instruction:#010x} at {pc:#x}
    Instruction { instruction: u32, pc: u64 },
    /// Unsupported processor state or MMIO syndrome
    State,
    /// Operand does not match the aligned MMIO fault address
    Address,
    /// Cannot read the MMIO device: {0}
    Bus(#[from] BusError),
    /// Cannot write register {id:#x}: {source}
    Register { id: u64, source: kvm_ioctls::Error },
}

#[derive(Debug, PartialEq, Eq)]
struct LoadPair32 {
    first: usize,
    second: usize,
    base: usize,
    offset: i64,
}

impl LoadPair32 {
    fn decode(instruction: u32) -> Option<Self> {
        if instruction & 0xffc0_0000 != 0x2940_0000 {
            return None;
        }
        let first = (instruction & 31) as usize;
        let second = ((instruction >> 10) & 31) as usize;
        if first == second {
            return None;
        }
        let immediate = i64::from((instruction >> 15) & 127);
        Some(Self {
            first,
            second,
            base: ((instruction >> 5) & 31) as usize,
            offset: (if immediate < 64 {
                immediate
            } else {
                immediate - 128
            }) * 4,
        })
    }
}

fn core_register(index: usize) -> u64 {
    arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(user_pt_regs, regs) + index * 8)
}

pub(crate) fn emulate(
    vcpu: &VcpuFd,
    memory: &GuestMemoryMmap,
    bus: &Bus,
    iss: u64,
    fault_ipa: u64,
) -> Result<(), MmioError> {
    let pc_id = arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(user_pt_regs, pc));
    let pc = paging::register(vcpu, pc_id)?;
    let address = paging::translate_kernel_address(vcpu, memory, pc)?;
    let instruction = u32::from_le(memory.read_obj(GuestAddress(address))?);
    let pair = LoadPair32::decode(instruction).ok_or(MmioError::Instruction { instruction, pc })?;
    let pstate = paging::register(
        vcpu,
        arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(user_pt_regs, pstate)),
    )?;
    let sctlr = paging::register(vcpu, SCTLR_EL1)?;
    if !matches!(pstate & 31, 4 | 5)
        || sctlr & (1 << 25) != 0
        || iss & ((1 << 24) | (1 << 10) | (1 << 9) | (1 << 8) | (1 << 7) | (1 << 6)) != 0
    {
        return Err(MmioError::State);
    }
    let base_id = if pair.base != 31 {
        core_register(pair.base)
    } else if pstate & 1 != 0 {
        arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(kvm_regs, sp_el1))
    } else {
        arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(user_pt_regs, sp))
    };
    let base = paging::register(vcpu, base_id)?;
    if pair.base == 31 && sctlr & (1 << 3) != 0 && base & 15 != 0 {
        return Err(MmioError::Address);
    }
    let operand = base.wrapping_add_signed(pair.offset);
    if operand & 3 != 0
        || operand & 0xfff > 0xff8
        || paging::translate_kernel_address(vcpu, memory, operand)? != fault_ipa
    {
        return Err(MmioError::Address);
    }
    let next_pc = pc.checked_add(4).ok_or(MmioError::Address)?;
    let mut data = [0; 8];
    bus.read(fault_ipa, &mut data)?;
    for (index, bytes) in [pair.first, pair.second]
        .into_iter()
        .zip(data.as_chunks::<4>().0.iter())
    {
        if index != 31 {
            let id = core_register(index);
            let value = u64::from(u32::from_le_bytes(*bytes));
            vcpu.set_one_reg(id, &value.to_le_bytes())
                .map_err(|source| MmioError::Register { id, source })?;
        }
    }
    vcpu.set_one_reg(pc_id, &next_pc.to_le_bytes())
        .map_err(|source| MmioError::Register { id: pc_id, source })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_tpm_instruction_and_signed_offsets() {
        assert_eq!(
            LoadPair32::decode(0x2943_2141).unwrap(),
            LoadPair32 {
                first: 1,
                second: 8,
                base: 10,
                offset: 24
            }
        );
        for (instruction, offset) in [
            (0x2940_2141, 0),
            (0x2960_2141, -256),
            (0x295f_a141, 252),
            (0x297f_a141, -4),
        ] {
            assert_eq!(LoadPair32::decode(instruction).unwrap().offset, offset);
        }
    }

    #[test]
    fn other_forms_and_overlapping_destinations_are_rejected() {
        for instruction in [
            0xa943_2141,
            0x2903_2141,
            0x29c3_2141,
            0x28c3_2141,
            0x2d43_2141,
            0x2943_2148,
        ] {
            assert_eq!(LoadPair32::decode(instruction), None);
        }
    }
}
