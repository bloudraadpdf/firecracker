// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Hyper-V enlightenments implemented by KVM for Windows guests.

use kvm_bindings::{CpuId, KVM_MAX_CPUID_ENTRIES, KVMIO, kvm_cpuid2, kvm_enable_cap};
use kvm_ioctls::{Kvm, VcpuFd};
use vmm_sys_util::ioctl::ioctl_with_mut_ptr;
use vmm_sys_util::ioctl_iowr_nr;

use super::generated::hyperv_tlfs::*;
use crate::cpu_config::x86_64::cpuid::{Cpuid, CpuidEntry, CpuidKey, CpuidRegisters, CpuidTrait};

const BASE: u32 = 0x4000_0000;
const INTERFACE: u32 = u32::from_le_bytes(*b"Hv#1");
// Runtime, clock, SynIC, synthetic timers, APIC, hypercalls, VP index, reference TSC,
// and frequency MSRs. No VMBus, nested Hyper-V or CPU hotplug is advertised.
const FEATURES_EAX: u32 = 0x7f | (1 << 9) | (1 << 11);
const FEATURES_EDX: u32 = (1 << 4) | (1 << 8);
const RECOMMENDATIONS: u32 = (1 << 2) | (1 << 3) | (1 << 5) | (1 << 9) | (1 << 10);

/// Errors configuring the guest Hyper-V interface.
#[derive(Debug, thiserror::Error, displaydoc::Display, PartialEq, Eq)]
pub enum HypervError {
    /// Missing KVM Hyper-V capability: {0}
    Capability(u32),
    /// Failed to query KVM Hyper-V CPUID: {0}
    Cpuid(kvm_ioctls::Error),
    /// Failed to allocate Hyper-V CPUID: {0}
    Allocation(#[from] vmm_sys_util::fam::Error),
    /// Host does not support the requested Hyper-V features in leaf {0:#x}.
    Features(u32),
    /// Failed to enable KVM Hyper-V SynIC: {0}
    Synic(kvm_ioctls::Error),
}

fn supported_cpuid(kvm: &Kvm) -> Result<CpuId, HypervError> {
    ioctl_iowr_nr!(KVM_GET_SUPPORTED_HV_CPUID, KVMIO, 0xc1, kvm_cpuid2);
    for cap in [
        kvm_bindings::KVM_CAP_SYS_HYPERV_CPUID,
        kvm_bindings::KVM_CAP_HYPERV_SYNIC2,
    ] {
        if kvm.check_extension_raw(u64::from(cap)) == 0 {
            return Err(HypervError::Capability(cap));
        }
    }
    let mut entries = CpuId::new(KVM_MAX_CPUID_ENTRIES)?;
    // JUSTIFICATION: kvm-ioctls does not wrap KVM_GET_SUPPORTED_HV_CPUID.
    // SAFETY: entries owns the complete variable-length kvm_cpuid2 allocation;
    // nent is its capacity and KVM writes at most that many entries.
    let result = unsafe {
        ioctl_with_mut_ptr(
            kvm,
            KVM_GET_SUPPORTED_HV_CPUID(),
            entries.as_mut_fam_struct_ptr(),
        )
    };
    if result != 0 {
        return Err(HypervError::Cpuid(kvm_ioctls::Error::last()));
    }
    Ok(entries)
}

fn validate_supported(supported: &CpuId) -> Result<(), HypervError> {
    for (leaf, eax, edx) in [
        (BASE + 3, FEATURES_EAX, FEATURES_EDX),
        (BASE + 4, RECOMMENDATIONS, 0),
    ] {
        let entry = supported
            .get(&CpuidKey::leaf(leaf))
            .ok_or(HypervError::Features(leaf))?;
        if entry.result.eax & eax != eax || entry.result.edx & edx != edx {
            return Err(HypervError::Features(leaf));
        }
    }
    Ok(())
}

/// Install a bounded feature set after CPU templates have been applied.
pub fn configure(cpuid: &mut Cpuid, kvm: &Kvm) -> Result<(), HypervError> {
    validate_host(kvm)?;
    install(cpuid);
    Ok(())
}

/// Check the host before booting or restoring a Hyper-V guest.
pub fn validate_host(kvm: &Kvm) -> Result<(), HypervError> {
    validate_supported(&supported_cpuid(kvm)?)
}

fn install(cpuid: &mut Cpuid) {
    // KVM's feature leaves overlap the Hyper-V interface. Remove the complete
    // hypervisor range so CPU templates cannot leave conflicting capabilities.
    cpuid
        .inner_mut()
        .retain(|key, _| !(BASE..0x5000_0000).contains(&key.leaf));
    for leaf in BASE..=BASE + 0xa {
        let result = match leaf - BASE {
            0 => CpuidRegisters {
                eax: BASE + 0xa,
                ebx: u32::from_le_bytes(*b"Linu"),
                ecx: u32::from_le_bytes(*b"x KV"),
                edx: u32::from_le_bytes(*b"M Hv"),
            },
            1 => CpuidRegisters {
                eax: INTERFACE,
                ..Default::default()
            },
            2 => CpuidRegisters {
                eax: 0x3839,
                ebx: 0x000a_0000,
                ..Default::default()
            },
            3 => CpuidRegisters {
                eax: FEATURES_EAX,
                edx: FEATURES_EDX,
                ..Default::default()
            },
            4 => CpuidRegisters {
                eax: RECOMMENDATIONS,
                ebx: 0xfff,
                ..Default::default()
            },
            _ => CpuidRegisters::default(),
        };
        cpuid.inner_mut().insert(
            CpuidKey::leaf(leaf),
            CpuidEntry {
                result,
                ..Default::default()
            },
        );
    }
}

/// Detect the interface from the guest CPUID, including a restored snapshot.
pub fn enabled(cpuid: &CpuId) -> bool {
    cpuid
        .get(&CpuidKey::leaf(BASE + 1))
        .is_some_and(|entry| entry.result.eax == INTERFACE)
}

/// Activate SynIC without clearing restored message and event pages.
pub fn enable_synic(vcpu: &VcpuFd) -> Result<(), HypervError> {
    vcpu.enable_cap(&kvm_enable_cap {
        cap: kvm_bindings::KVM_CAP_HYPERV_SYNIC2,
        ..Default::default()
    })
    .map_err(HypervError::Synic)
}

/// Complete userspace Hyper-V exits without advertising a VMBus implementation.
pub fn handle_exit(exit: &mut kvm_bindings::kvm_hyperv_exit) -> bool {
    match exit.type_ {
        kvm_bindings::KVM_EXIT_HYPERV_SYNIC => true,
        kvm_bindings::KVM_EXIT_HYPERV_HCALL => {
            // JUSTIFICATION: the KVM ABI exposes hypercall arguments through a union.
            // SAFETY: the HCALL discriminator selects this member and the caller owns it.
            let hcall = unsafe { &mut exit.u.hcall };
            crate::logger::debug!("Unsupported Hyper-V hypercall: {:#x}", hcall.input);
            hcall.result = 2; // HV_STATUS_INVALID_HYPERCALL_CODE, TLFS appendix B.
            true
        }
        _ => false,
    }
}

/// Writable Hyper-V state, including timers omitted from KVM's MSR index list.
pub fn msrs_to_save() -> Vec<u32> {
    [
        HV_X64_MSR_GUEST_OS_ID,
        HV_X64_MSR_HYPERCALL,
        HV_X64_MSR_VP_INDEX,
        HV_X64_MSR_VP_RUNTIME,
        HV_X64_MSR_REFERENCE_TSC,
        HV_X64_MSR_VP_ASSIST_PAGE,
        HV_X64_MSR_SCONTROL,
        HV_X64_MSR_SVERSION,
        HV_X64_MSR_SIEFP,
        HV_X64_MSR_SIMP,
    ]
    .into_iter()
    .chain(HV_X64_MSR_SINT0..=HV_X64_MSR_SINT15)
    .chain(HV_X64_MSR_STIMER0_CONFIG..=HV_X64_MSR_STIMER3_COUNT)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu_config::x86_64::cpuid::IntelCpuid;

    #[test]
    fn unsupported_hypercall_returns_a_guest_error() {
        let mut exit = kvm_bindings::kvm_hyperv_exit {
            type_: kvm_bindings::KVM_EXIT_HYPERV_HCALL,
            ..Default::default()
        };
        exit.u.hcall.input = 0x005c;
        exit.u.hcall.result = u64::MAX;
        assert!(handle_exit(&mut exit));
        // SAFETY: the test initialises the HCALL member above.
        assert_eq!(unsafe { exit.u.hcall.result }, 2);
        exit.type_ = kvm_bindings::KVM_EXIT_HYPERV_SYNIC;
        assert!(handle_exit(&mut exit));
        exit.type_ = u32::MAX;
        assert!(!handle_exit(&mut exit));
    }

    #[test]
    fn rejects_missing_feature_leaves() {
        let empty = CpuId::new(0).unwrap();
        assert!(!enabled(&empty));
        assert_eq!(
            validate_supported(&empty),
            Err(HypervError::Features(BASE + 3))
        );
        let mut cpuid = Cpuid::Intel(IntelCpuid(Default::default()));
        install(&mut cpuid);
        cpuid.inner_mut().remove(&CpuidKey::leaf(BASE + 4));
        assert_eq!(
            validate_supported(&CpuId::try_from(cpuid).unwrap()),
            Err(HypervError::Features(BASE + 4))
        );
    }

    #[test]
    fn replaces_conflicting_leaves_without_changing_architectural_cpuid() {
        let mut cpuid = Cpuid::Intel(IntelCpuid(Default::default()));
        cpuid
            .inner_mut()
            .insert(CpuidKey::leaf(1), CpuidEntry::default());
        cpuid
            .inner_mut()
            .insert(CpuidKey::leaf(BASE + 0x10), CpuidEntry::default());
        install(&mut cpuid);
        assert!(cpuid.get(&CpuidKey::leaf(BASE + 0x10)).is_none());
        assert_eq!(cpuid.get(&CpuidKey::leaf(1)), Some(&CpuidEntry::default()));
        let kvm_cpuid = CpuId::try_from(cpuid).unwrap();
        assert!(enabled(&kvm_cpuid));
        validate_supported(&kvm_cpuid).unwrap();
        let mut unsupported = kvm_cpuid.clone();
        unsupported
            .get_mut(&CpuidKey::leaf(BASE + 3))
            .unwrap()
            .result
            .eax &= !(1 << 3);
        assert_eq!(
            validate_supported(&unsupported),
            Err(HypervError::Features(BASE + 3))
        );
    }
}
