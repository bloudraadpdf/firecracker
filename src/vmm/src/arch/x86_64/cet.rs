// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! KVM control-flow enforcement state outside XSAVE.

use kvm_bindings::{CpuId, KVM_REG_SIZE_U64, KVM_REG_X86, KVMIO, kvm_one_reg};
use kvm_ioctls::VcpuFd;
use vmm_sys_util::ioctl::{ioctl_with_mut_ref, ioctl_with_ref};
use vmm_sys_util::ioctl_iow_nr;

use super::generated::msr_index::{
    MSR_IA32_INT_SSP_TAB, MSR_IA32_PL0_SSP, MSR_IA32_PL1_SSP, MSR_IA32_PL2_SSP, MSR_IA32_PL3_SSP,
    MSR_IA32_S_CET, MSR_IA32_U_CET,
};

// Linux asm/kvm.h: KVM_X86_REG_KVM(KVM_REG_GUEST_SSP), register type 3, index 0.
const GUEST_SSP: u64 = KVM_REG_X86 | KVM_REG_SIZE_U64 | (3 << 32);

pub(super) fn shadow_stack_enabled(cpuid: &CpuId) -> bool {
    cpuid
        .as_slice()
        .iter()
        .any(|entry| entry.function == 7 && entry.index == 0 && entry.ecx & (1 << 7) != 0)
}

pub(super) fn msrs_to_save(cpuid: &CpuId) -> Vec<u32> {
    let shadow_stack = shadow_stack_enabled(cpuid);
    let ibt = cpuid
        .as_slice()
        .iter()
        .any(|entry| entry.function == 7 && entry.index == 0 && entry.edx & (1 << 20) != 0);
    let mut msrs = Vec::new();
    if shadow_stack || ibt {
        msrs.extend([MSR_IA32_U_CET, MSR_IA32_S_CET]);
    }
    if shadow_stack {
        msrs.extend([
            MSR_IA32_PL0_SSP,
            MSR_IA32_PL1_SSP,
            MSR_IA32_PL2_SSP,
            MSR_IA32_PL3_SSP,
            MSR_IA32_INT_SSP_TAB,
        ]);
    }
    msrs
}

pub(super) fn get_guest_ssp(vcpu: &VcpuFd) -> Result<u64, kvm_ioctls::Error> {
    ioctl_iow_nr!(KVM_GET_ONE_REG, KVMIO, 0xab, kvm_one_reg);
    let mut value = 0u64;
    let mut register = kvm_one_reg {
        id: GUEST_SSP,
        addr: std::ptr::from_mut(&mut value) as u64,
    };
    // JUSTIFICATION: kvm-ioctls exposes ONE_REG only on ARM and RISC-V.
    // SAFETY: The register ID specifies exactly eight bytes in the live value buffer.
    let result = unsafe { ioctl_with_mut_ref(vcpu, KVM_GET_ONE_REG(), &mut register) };
    if result < 0 {
        return Err(kvm_ioctls::Error::last());
    }
    Ok(value)
}

pub(super) fn set_guest_ssp(vcpu: &VcpuFd, value: u64) -> Result<(), kvm_ioctls::Error> {
    ioctl_iow_nr!(KVM_SET_ONE_REG, KVMIO, 0xac, kvm_one_reg);
    let register = kvm_one_reg {
        id: GUEST_SSP,
        addr: std::ptr::from_ref(&value) as u64,
    };
    // JUSTIFICATION: kvm-ioctls exposes ONE_REG only on ARM and RISC-V.
    // SAFETY: The register ID specifies exactly eight bytes in the live value buffer.
    let result = unsafe { ioctl_with_ref(vcpu, KVM_SET_ONE_REG(), &register) };
    if result < 0 {
        return Err(kvm_ioctls::Error::last());
    }
    Ok(())
}
