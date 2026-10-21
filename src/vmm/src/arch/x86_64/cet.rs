// Copyright 2026 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use kvm_bindings::{CpuId, KVM_REG_SIZE_U64, KVM_REG_X86, KVMIO, kvm_one_reg};
use kvm_ioctls::VcpuFd;
use vmm_sys_util::errno;
use vmm_sys_util::ioctl::ioctl_with_ref;
use vmm_sys_util::ioctl_iow_nr;

use crate::arch::x86_64::generated::msr_index::{
    MSR_IA32_INT_SSP_TAB, MSR_IA32_PL0_SSP, MSR_IA32_PL1_SSP, MSR_IA32_PL2_SSP, MSR_IA32_PL3_SSP,
    MSR_IA32_S_CET, MSR_IA32_U_CET,
};

// Linux v7.0 arch/x86/include/uapi/asm/kvm.h: KVM_X86_REG_KVM(KVM_REG_GUEST_SSP).
const GUEST_SSP_REG: u64 = KVM_REG_X86 | KVM_REG_SIZE_U64 | (3 << 32);

// kvm-ioctls 0.25 exposes ONE_REG only on aarch64 and riscv64.
#[allow(missing_docs)]
mod ioctls {
    use super::*;
    ioctl_iow_nr!(KVM_GET_ONE_REG, KVMIO, 0xab, kvm_one_reg);
    ioctl_iow_nr!(KVM_SET_ONE_REG, KVMIO, 0xac, kvm_one_reg);
}

/// Errors saving or restoring CET state.
#[derive(Debug, PartialEq, Eq, thiserror::Error, displaydoc::Display)]
pub enum CetError {
    /// CET requires MSR {0:#x}, which is not available in the snapshot MSR list.
    MissingMsr(u32),
    /// Snapshot contains CET MSR {0:#x}, which its guest CPUID forbids.
    UnexpectedMsr(u32),
    /// KVM ONE_REG is required to preserve the live shadow stack pointer.
    OneRegUnsupported,
    /// Snapshot live shadow stack pointer does not match its guest CPUID.
    InvalidSspState,
    /// Failed to read the live shadow stack pointer: {0}
    GetSsp(errno::Error),
    /// Failed to restore the live shadow stack pointer: {0}
    SetSsp(errno::Error),
}

#[derive(Clone, Copy)]
pub(super) struct Features {
    shadow_stack: bool,
    indirect_branch_tracking: bool,
}

impl Features {
    pub(super) fn from_cpuid(cpuid: &CpuId) -> Self {
        let entry = cpuid
            .as_slice()
            .iter()
            .find(|entry| entry.function == 7 && entry.index == 0);
        Self {
            shadow_stack: shadow_stack_enabled(cpuid),
            indirect_branch_tracking: entry.is_some_and(|entry| entry.edx & (1 << 20) != 0),
        }
    }

    fn msr_is_accessible(self, index: u32) -> bool {
        match index {
            MSR_IA32_U_CET | MSR_IA32_S_CET => self.shadow_stack || self.indirect_branch_tracking,
            MSR_IA32_PL0_SSP | MSR_IA32_PL1_SSP | MSR_IA32_PL2_SSP | MSR_IA32_PL3_SSP
            | MSR_IA32_INT_SSP_TAB => self.shadow_stack,
            _ => true,
        }
    }

    fn require_msrs(self, available: &[u32]) -> Result<(), CetError> {
        for index in [MSR_IA32_U_CET, MSR_IA32_PL3_SSP] {
            if self.msr_is_accessible(index) && !available.contains(&index) {
                return Err(CetError::MissingMsr(index));
            }
        }
        Ok(())
    }

    pub(super) fn validate_saved_msrs(self, saved: &[u32]) -> Result<(), CetError> {
        self.require_msrs(saved)?;
        if let Some(index) = saved.iter().find(|index| !self.msr_is_accessible(**index)) {
            return Err(CetError::UnexpectedMsr(*index));
        }
        Ok(())
    }

    pub(super) fn snapshot_msrs(self, available: &[u32]) -> Result<Vec<u32>, CetError> {
        self.require_msrs(available)?;
        Ok(available
            .iter()
            .copied()
            .filter(|index| self.msr_is_accessible(*index))
            .collect())
    }

    pub(super) fn validate_ssp(
        self,
        one_reg_supported: bool,
        ssp: Option<u64>,
    ) -> Result<(), CetError> {
        if self.shadow_stack && !one_reg_supported {
            return Err(CetError::OneRegUnsupported);
        }
        if self.shadow_stack != ssp.is_some() {
            return Err(CetError::InvalidSspState);
        }
        Ok(())
    }

    pub(super) fn save_ssp(
        self,
        vcpu: &VcpuFd,
        one_reg_supported: bool,
    ) -> Result<Option<u64>, CetError> {
        if !self.shadow_stack {
            return Ok(None);
        }
        if !one_reg_supported {
            return Err(CetError::OneRegUnsupported);
        }
        get_guest_ssp(vcpu).map(Some).map_err(CetError::GetSsp)
    }

    pub(super) fn restore_ssp(
        self,
        vcpu: &VcpuFd,
        one_reg_supported: bool,
        ssp: Option<u64>,
    ) -> Result<(), CetError> {
        self.validate_ssp(one_reg_supported, ssp)?;
        let Some(ssp) = ssp else {
            return Ok(());
        };
        set_guest_ssp(vcpu, ssp).map_err(CetError::SetSsp)
    }
}

pub(super) fn shadow_stack_enabled(cpuid: &CpuId) -> bool {
    cpuid
        .as_slice()
        .iter()
        .any(|entry| entry.function == 7 && entry.index == 0 && entry.ecx & (1 << 7) != 0)
}

#[cfg(test)]
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
    let mut value = 0u64;
    let register = kvm_one_reg {
        id: GUEST_SSP_REG,
        addr: std::ptr::from_mut(&mut value) as u64,
    };
    // JUSTIFICATION: kvm-ioctls exposes ONE_REG only on ARM and RISC-V.
    // SAFETY: The register ID specifies exactly eight bytes in the live value buffer.
    let result = unsafe { ioctl_with_ref(vcpu, ioctls::KVM_GET_ONE_REG(), &register) };
    if result != 0 {
        return Err(kvm_ioctls::Error::last());
    }
    Ok(value)
}

pub(super) fn set_guest_ssp(vcpu: &VcpuFd, value: u64) -> Result<(), kvm_ioctls::Error> {
    let register = kvm_one_reg {
        id: GUEST_SSP_REG,
        addr: std::ptr::from_ref(&value) as u64,
    };
    // JUSTIFICATION: kvm-ioctls exposes ONE_REG only on ARM and RISC-V.
    // SAFETY: The register ID specifies exactly eight bytes in the live value buffer.
    let result = unsafe { ioctl_with_ref(vcpu, ioctls::KVM_SET_ONE_REG(), &register) };
    if result != 0 {
        return Err(kvm_ioctls::Error::last());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use kvm_bindings::{CpuId, kvm_cpuid_entry2};

    use super::*;
    use crate::arch::x86_64::generated::msr_index::MSR_IA32_TSC;

    fn cpuid(ecx: u32, edx: u32) -> CpuId {
        CpuId::from_entries(&[kvm_cpuid_entry2 {
            function: 7,
            index: 0,
            ecx,
            edx,
            ..Default::default()
        }])
        .unwrap()
    }

    #[test]
    fn test_cet_snapshot_register_selection() {
        let available = [MSR_IA32_TSC, MSR_IA32_U_CET, MSR_IA32_PL3_SSP];
        let hidden = Features::from_cpuid(&cpuid(0, 0));
        assert_eq!(hidden.snapshot_msrs(&available).unwrap(), [MSR_IA32_TSC]);
        assert_eq!(hidden.validate_ssp(false, None), Ok(()));
        assert_eq!(
            hidden.validate_ssp(true, Some(0)),
            Err(CetError::InvalidSspState)
        );

        let ibt = Features::from_cpuid(&cpuid(0, 1 << 20));
        assert_eq!(ibt.snapshot_msrs(&available).unwrap(), available[..2]);
        assert_eq!(ibt.validate_ssp(false, None), Ok(()));

        let shstk = Features::from_cpuid(&cpuid(1 << 7, 0));
        assert_eq!(shstk.snapshot_msrs(&available).unwrap(), available);
        let supervisor = [
            MSR_IA32_TSC,
            MSR_IA32_U_CET,
            MSR_IA32_S_CET,
            MSR_IA32_PL0_SSP,
            MSR_IA32_PL1_SSP,
            MSR_IA32_PL2_SSP,
            MSR_IA32_PL3_SSP,
            MSR_IA32_INT_SSP_TAB,
        ];
        assert_eq!(hidden.snapshot_msrs(&supervisor).unwrap(), [MSR_IA32_TSC]);
        assert_eq!(ibt.snapshot_msrs(&supervisor).unwrap(), supervisor[..3]);
        assert_eq!(shstk.snapshot_msrs(&supervisor).unwrap(), supervisor);
        assert_eq!(shstk.validate_ssp(true, Some(0x3000)), Ok(()));
        assert_eq!(
            shstk.validate_ssp(false, Some(0x3000)),
            Err(CetError::OneRegUnsupported)
        );
        assert_eq!(
            shstk.validate_ssp(true, None),
            Err(CetError::InvalidSspState)
        );
    }

    #[test]
    fn test_cet_snapshot_requires_available_msrs() {
        let shstk = Features::from_cpuid(&cpuid(1 << 7, 0));
        assert_eq!(
            shstk.snapshot_msrs(&[]),
            Err(CetError::MissingMsr(MSR_IA32_U_CET))
        );
        assert_eq!(
            shstk.snapshot_msrs(&[MSR_IA32_U_CET]),
            Err(CetError::MissingMsr(MSR_IA32_PL3_SSP))
        );
        let ibt = Features::from_cpuid(&cpuid(0, 1 << 20));
        assert_eq!(
            ibt.snapshot_msrs(&[]),
            Err(CetError::MissingMsr(MSR_IA32_U_CET))
        );
    }

    #[test]
    fn test_saved_cet_msrs_must_match_guest_cpuid() {
        let hidden = Features::from_cpuid(&cpuid(0, 0));
        assert_eq!(hidden.validate_saved_msrs(&[MSR_IA32_TSC]), Ok(()));
        for index in [MSR_IA32_U_CET, MSR_IA32_PL3_SSP] {
            assert_eq!(
                hidden.validate_saved_msrs(&[index]),
                Err(CetError::UnexpectedMsr(index))
            );
        }
        let ibt = Features::from_cpuid(&cpuid(0, 1 << 20));
        assert_eq!(ibt.validate_saved_msrs(&[MSR_IA32_U_CET]), Ok(()));
        assert_eq!(
            ibt.validate_saved_msrs(&[MSR_IA32_U_CET, MSR_IA32_PL3_SSP]),
            Err(CetError::UnexpectedMsr(MSR_IA32_PL3_SSP))
        );
    }

    #[test]
    fn test_other_cpuid_subleaves_do_not_enable_cet() {
        let entries = CpuId::from_entries(&[kvm_cpuid_entry2 {
            function: 7,
            index: 1,
            ecx: 1 << 7,
            edx: 1 << 20,
            ..Default::default()
        }])
        .unwrap();
        let features = Features::from_cpuid(&entries);
        assert!(
            features
                .snapshot_msrs(&[MSR_IA32_U_CET, MSR_IA32_PL3_SSP])
                .unwrap()
                .is_empty()
        );
        assert_eq!(features.validate_ssp(false, None), Ok(()));
    }
}
