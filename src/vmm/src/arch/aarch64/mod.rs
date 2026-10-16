// Copyright 2019 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

pub(crate) mod cache_info;
mod fdt;
/// Module for the global interrupt controller configuration.
pub mod gic;
/// Architecture specific KVM-related code
pub mod kvm;
/// Layout for this aarch64 system.
pub mod layout;
/// Logic for configuring aarch64 registers.
pub mod regs;
/// Architecture specific vCPU code
pub mod vcpu;
/// Architecture specific VM state code
pub mod vm;

use std::cmp::min;
use std::fmt::Debug;
use std::fs::File;
use std::os::unix::fs::FileExt;

use linux_loader::loader::pe::PE as Loader;
use linux_loader::loader::{Cmdline, KernelLoader};
use vm_memory::{GuestMemoryBackend, GuestMemoryError, GuestMemoryRegion};

use crate::arch::{BootProtocol, EntryPoint, arch_memory_regions_with_gap};
use crate::cpu_config::aarch64::{CpuConfiguration, CpuConfigurationError};
use crate::cpu_config::templates::CustomCpuTemplate;
use crate::initrd::InitrdConfig;
use zerocopy::IntoBytes;

use crate::logger::warn;
use crate::utils::{u64_to_usize, usize_to_u64};
use crate::vmm_config::machine_config::MachineConfig;
use crate::vstate::memory::{Address, Bytes, GuestAddress, GuestMemoryMmap, GuestRegionType};
use crate::vstate::vcpu::KvmVcpuError;
use crate::vstate::vm::KvmVm;
use crate::{DeviceManager, Kvm, Vcpu, VcpuConfig, align_up, logger};

/// Errors thrown while configuring aarch64 system.
#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum ConfigurationError {
    /// Failed to create a Flattened Device Tree for this aarch64 microVM: {0}
    SetupFDT(#[from] fdt::FdtError),
    /// Failed to write to guest memory.
    MemoryError(#[from] GuestMemoryError),
    /// Cannot copy kernel file fd
    KernelFile,
    /// Cannot load kernel due to invalid memory configuration or invalid kernel image: {0}
    KernelLoader(#[from] linux_loader::loader::Error),
    /// Error creating vcpu configuration: {0}
    VcpuConfig(#[from] CpuConfigurationError),
    /// Error configuring the vcpu: {0}
    VcpuConfigure(#[from] KvmVcpuError),
    /// Failed to read host cache information: {0}
    CacheInfo(#[from] cache_info::CacheInfoError),
    /// Failed to read firmware: {0}
    FirmwareIo(#[from] std::io::Error),
    /// Invalid firmware image size
    FirmwareSize,
    /// Invalid firmware volume header or checksum
    FirmwareFormat,
    /// Failed to reserve firmware handoff memory: {0}
    FirmwareMemory(#[from] vm_allocator::Error),
    /// Firmware requires a Firecracker build with the windows feature.
    WindowsCapability,
    /// ACPI table construction failed: {0}
    #[cfg(feature = "windows")]
    Acpi(#[from] crate::acpi::AcpiError),
}

/// Returns a Vec of the valid memory addresses for aarch64.
/// See [`layout`](layout) module for a drawing of the specific memory model for this platform.
pub fn arch_memory_regions(size: usize) -> Vec<(GuestAddress, usize)> {
    assert!(size > 0, "Attempt to allocate guest memory of length 0");

    let dram_size = min(size, layout::DRAM_MEM_MAX_SIZE);

    if dram_size != size {
        logger::warn!(
            "Requested memory size {} exceeds architectural maximum (1022GiB). Size has been \
             truncated to {}",
            size,
            dram_size
        );
    }

    let mut regions = vec![];
    if let Some((offset, remaining)) = arch_memory_regions_with_gap(
        &mut regions,
        u64_to_usize(layout::DRAM_MEM_START),
        dram_size,
        u64_to_usize(layout::MMIO64_MEM_START),
        u64_to_usize(layout::MMIO64_MEM_SIZE),
    ) {
        regions.push((GuestAddress(offset as u64), remaining));
    }

    regions
}

/// Configures the system for booting Linux.
#[allow(clippy::too_many_arguments)]
pub fn configure_system_for_boot(
    kvm: &Kvm,
    vm: &KvmVm,
    device_manager: &mut DeviceManager,
    vcpus: &mut [Vcpu],
    machine_config: &MachineConfig,
    cpu_template: &CustomCpuTemplate,
    entry_point: EntryPoint,
    initrd: &Option<InitrdConfig>,
    boot_cmdline: Cmdline,
    _firmware: Option<&crate::vmm_config::boot_source::FirmwareConfig>,
) -> Result<(), ConfigurationError> {
    #[cfg(not(feature = "windows"))]
    if entry_point.protocol == BootProtocol::FirmwareBoot || _firmware.is_some() {
        return Err(ConfigurationError::WindowsCapability);
    }
    // Construct the base CpuConfiguration to apply CPU template onto.
    let cpu_config = CpuConfiguration::new(cpu_template, vcpus)?;

    // Apply CPU template to the base CpuConfiguration.
    let cpu_config = CpuConfiguration::apply_template(cpu_config, cpu_template);

    let vcpu_config = VcpuConfig {
        vcpu_count: machine_config.vcpu_count,
        smt: machine_config.smt,
        cpu_config,
    };

    let optional_capabilities = kvm.optional_capabilities();
    // Configure vCPUs with normalizing and setting the generated CPU configuration.
    for vcpu in vcpus.iter_mut() {
        vcpu.kvm_vcpu.configure(
            vm.guest_memory(),
            entry_point,
            &vcpu_config,
            &optional_capabilities,
        )?;
    }

    // Override CLIDR_EL1 ctype/LoC fields on each vCPU to match the host's
    // real cache topology. See `override_clidr` for details.
    override_clidr(vcpus)?;

    let vcpu_mpidr = vcpus
        .iter_mut()
        .map(|cpu| cpu.kvm_vcpu.get_mpidr())
        .collect::<Result<Vec<_>, _>>()
        .map_err(KvmVcpuError::ConfigureRegisters)?;
    let topology = CpuTopology {
        mpidrs: vcpu_mpidr,
        pmu_irq: vcpus[0].kvm_vcpu.has_pmu().then_some(layout::PMU_IRQ),
    };
    let cmdline = boot_cmdline
        .as_cstring()
        .expect("Cannot create cstring from cmdline string");

    #[cfg(feature = "windows")]
    if entry_point.protocol == BootProtocol::FirmwareBoot {
        crate::acpi::create_acpi_tables(
            vm.guest_memory(),
            device_manager,
            &mut vm.resource_allocator(),
            &topology,
            vm.get_irqchip(),
        )?;
    }

    let fdt = fdt::create_fdt(
        vm.guest_memory(),
        &topology,
        cmdline,
        device_manager,
        vm.get_irqchip(),
        initrd,
        entry_point.protocol,
    )?;

    let fdt_address = GuestAddress(get_fdt_addr(vm.guest_memory()));
    vm.guest_memory().write_slice(fdt.as_slice(), fdt_address)?;

    #[cfg(feature = "windows")]
    if entry_point.protocol == BootProtocol::FirmwareBoot {
        if fdt.len() > 0x1_0000 {
            return Err(ConfigurationError::FirmwareSize);
        }
        vm.guest_memory()
            .write_slice(&fdt, GuestAddress(layout::FIRMWARE_FDT_ADDR))?;
    }

    Ok(())
}

/// Override CLIDR_EL1 ctype/LoC fields on each vCPU to match the host's real
/// cache topology.
///
/// Since host kernel 6.3 (commit 7af0c2534f4c), KVM fabricates CLIDR_EL1
/// instead of passing through the host's real value. This can cause the guest
/// to see fewer cache levels than actually exist. Guest kernels >= 6.1.156
/// backported `init_of_cache_level()` which counts cache leaves from the DT,
/// while `populate_cache_leaves()` uses CLIDR_EL1. If the DT (built from host
/// sysfs) describes different cache entries than CLIDR_EL1, the mismatch
/// causes cache sysfs entries to not be created.
///
/// We read the current (possibly fabricated) CLIDR_EL1, replace only the ctype
/// and LoC fields with values derived from sysfs, and preserve all other fields
/// (LoUU, LoUIS, ICB, Ttype). This is safe on pre-6.3 kernels where CLIDR
/// already matches sysfs — the write is skipped as a no-op.
fn override_clidr(vcpus: &[Vcpu]) -> Result<(), ConfigurationError> {
    let mut l1_caches = Vec::new();
    let mut non_l1_caches = Vec::new();
    cache_info::read_cache_config(&mut l1_caches, &mut non_l1_caches)?;

    // If sysfs reports no L1 caches, we cannot build a meaningful CLIDR.
    // Writing an all-zero CLIDR would tell the guest there are no caches,
    // which is worse than whatever KVM fabricated. Leave it alone.
    if l1_caches.is_empty() {
        warn!("No L1 caches found in sysfs, skipping CLIDR override");
        return Ok(());
    }

    let sysfs_clidr = cache_info::build_clidr_from_caches(&l1_caches, &non_l1_caches);

    let mut cur_clidr: u64 = 0;
    // Reading/writing CLIDR_EL1 via KVM_SET_ONE_REG may not be supported on
    // older kernels (pre-6.3). In that case KVM passes through the real host
    // CLIDR and the override is unnecessary, so we warn and continue.
    if let Err(e) = vcpus[0]
        .kvm_vcpu
        .fd
        .get_one_reg(regs::CLIDR_EL1, cur_clidr.as_mut_bytes())
    {
        warn!("Failed to read CLIDR_EL1, skipping override: {e}");
        return Ok(());
    }

    let new_clidr = cache_info::merge_clidr(cur_clidr, sysfs_clidr);

    if new_clidr != cur_clidr {
        for vcpu in vcpus.iter() {
            if let Err(e) = vcpu
                .kvm_vcpu
                .fd
                .set_one_reg(regs::CLIDR_EL1, new_clidr.as_bytes())
            {
                warn!(
                    "Failed to set CLIDR_EL1 to {:#x} on vCPU {}, skipping override: {e}",
                    new_clidr, vcpu.kvm_vcpu.index
                );
                return Ok(());
            }
        }
    }

    Ok(())
}

/// Returns the memory address where the kernel could be loaded.
pub fn get_kernel_start() -> u64 {
    layout::SYSTEM_MEM_START + layout::SYSTEM_MEM_SIZE
}

/// Returns the memory address where the initrd could be loaded.
pub fn initrd_load_addr(guest_mem: &GuestMemoryMmap, initrd_size: usize) -> Option<u64> {
    let rounded_size = align_up!(
        usize_to_u64(initrd_size),
        usize_to_u64(super::GUEST_PAGE_SIZE)
    );
    GuestAddress(get_fdt_addr(guest_mem))
        .checked_sub(rounded_size)
        .filter(|&addr| guest_mem.address_in_range(addr))
        .map(|addr| addr.raw_value())
}

// Auxiliary function to get the address where the device tree blob is loaded.
fn get_fdt_addr(mem: &GuestMemoryMmap) -> u64 {
    // Find the first (and only) DRAM region.
    let dram_region = mem
        .iter()
        .find(|region| region.region_type == GuestRegionType::Dram)
        .unwrap();

    // If the memory allocated is smaller than the size allocated for the FDT,
    // we return the start of the DRAM so that
    // we allow the code to try and load the FDT.
    dram_region
        .last_addr()
        .checked_sub(layout::FDT_MAX_SIZE as u64 - 1)
        .filter(|&addr| mem.address_in_range(addr))
        .map(|addr| addr.raw_value())
        .unwrap_or(layout::DRAM_MEM_START)
}

/// CPU identifiers and optional performance-monitor interrupt for firmware tables.
#[derive(Debug)]
pub struct CpuTopology {
    /// CPU affinities in vCPU order.
    pub mpidrs: Vec<u64>,
    /// Shared PPI used by each vCPU's performance monitor.
    pub pmu_irq: Option<u32>,
}

/// Load a Linux kernel or CloudHv firmware into guest memory.
pub fn load_kernel(
    kernel: &File,
    guest_memory: &GuestMemoryMmap,
) -> Result<EntryPoint, ConfigurationError> {
    let mut signature = [0; 4];
    if kernel.read_at(&mut signature, 0x1028)? == 4 && signature == *b"_FVH" {
        #[cfg(feature = "windows")]
        return load_firmware(kernel, guest_memory);
        #[cfg(not(feature = "windows"))]
        return Err(ConfigurationError::WindowsCapability);
    }
    // Need to clone the File because reading from it
    // mutates it.
    let mut kernel_file = kernel
        .try_clone()
        .map_err(|_| ConfigurationError::KernelFile)?;

    let entry_addr = Loader::load(
        guest_memory,
        Some(GuestAddress(get_kernel_start())),
        &mut kernel_file,
        None,
    )?;

    Ok(EntryPoint {
        entry_addr: entry_addr.kernel_load,
        protocol: BootProtocol::LinuxBoot,
    })
}

#[cfg(feature = "windows")]
fn load_firmware(
    kernel: &File,
    guest_memory: &GuestMemoryMmap,
) -> Result<EntryPoint, ConfigurationError> {
    let size = kernel.metadata()?.len();
    if !(0x2000..=layout::FIRMWARE_MAX_SIZE).contains(&size) {
        return Err(ConfigurationError::FirmwareSize);
    }
    let mut firmware = vec![0; usize::try_from(size).unwrap()];
    kernel.read_exact_at(&mut firmware, 0)?;
    let volume = &firmware[0x1000..];
    let volume_len = u64::from_le_bytes(volume[32..40].try_into().unwrap());
    let header_len = usize::from(u16::from_le_bytes(volume[48..50].try_into().unwrap()));
    let header = volume
        .get(..header_len)
        .filter(|h| h.len() >= 56 && h.len().is_multiple_of(2))
        .ok_or(ConfigurationError::FirmwareFormat)?;
    if volume[55] != 2
        || volume_len > volume.len() as u64
        || volume_len < header_len as u64
        || header.as_chunks::<2>().0.iter().fold(0_u16, |sum, word| {
            sum.wrapping_add(u16::from_le_bytes(*word))
        }) != 0
    {
        return Err(ConfigurationError::FirmwareFormat);
    }
    guest_memory.write_slice(&firmware, GuestAddress(layout::FIRMWARE_START))?;
    Ok(EntryPoint {
        entry_addr: GuestAddress(layout::FIRMWARE_START),
        protocol: BootProtocol::FirmwareBoot,
    })
}

#[cfg(feature = "windows")]
fn reserve_firmware_handoff(
    allocator: &mut vm_allocator::AddressAllocator,
) -> Result<(), vm_allocator::Error> {
    use vm_allocator::AllocPolicy;
    let mut next = allocator.clone();
    for (address, size) in [
        (layout::DRAM_MEM_START, 0x10_0000),
        (layout::FIRMWARE_RSDP_ADDR, 0x1_0000),
    ] {
        next.allocate(size, 1, AllocPolicy::ExactMatch(address))?;
    }
    *allocator = next;
    Ok(())
}

/// Keep firmware handoff pages separate from allocated ACPI data.
#[cfg(feature = "windows")]
pub fn reserve_firmware_memory(vm: &KvmVm) -> Result<(), ConfigurationError> {
    reserve_firmware_handoff(&mut vm.resource_allocator().system_memory)?;
    Ok(())
}

#[cfg(test)]
#[cfg(feature = "windows")]
mod handoff_tests {
    use super::*;
    use vm_allocator::{AddressAllocator, AllocPolicy};

    #[test]
    fn failed_handoff_reservations_are_atomic() {
        let mut allocator = AddressAllocator::new(layout::DRAM_MEM_START, 0x20_0000).unwrap();
        let collision = layout::FIRMWARE_RSDP_ADDR;
        allocator
            .allocate(4096, 1, AllocPolicy::ExactMatch(collision))
            .unwrap();
        reserve_firmware_handoff(&mut allocator).unwrap_err();
        allocator
            .allocate(
                0x10_0000,
                1,
                AllocPolicy::ExactMatch(layout::DRAM_MEM_START),
            )
            .unwrap();
        allocator
            .allocate(4096, 1, AllocPolicy::ExactMatch(collision))
            .unwrap_err();
    }
}

#[cfg(kani)]
mod verification {
    use crate::arch::aarch64::layout::{
        DRAM_MEM_MAX_SIZE, DRAM_MEM_START, FIRST_ADDR_PAST_64BITS_MMIO, MMIO64_MEM_START,
    };
    use crate::arch::arch_memory_regions;

    #[kani::proof]
    #[kani::unwind(3)]
    fn verify_arch_memory_regions() {
        let len: usize = kani::any::<usize>();
        kani::assume(len > 0);

        let regions = arch_memory_regions(len);

        for region in &regions {
            println!(
                "region: [{:x}:{:x})",
                region.0.0,
                region.0.0 + region.1 as u64
            );
        }

        // On Arm we have one MMIO gap that might fall within addressable ranges,
        // so we can get either 1 or 2 regions.
        assert!(regions.len() >= 1);
        assert!(regions.len() <= 2);

        // The total length of all regions cannot exceed DRAM_MEM_MAX_SIZE
        let actual_len = regions.iter().map(|&(_, len)| len).sum::<usize>();
        assert!(actual_len <= DRAM_MEM_MAX_SIZE);
        // The total length is smaller or equal to the length we asked
        assert!(actual_len <= len);
        // If it's smaller, it's because we asked more than the the maximum possible.
        if (actual_len) < len {
            assert!(len > DRAM_MEM_MAX_SIZE);
        }

        // No region overlaps the 64-bit MMIO gap
        assert!(
            regions
                .iter()
                .all(|&(start, len)| start.0 >= FIRST_ADDR_PAST_64BITS_MMIO
                    || start.0 + len as u64 <= MMIO64_MEM_START)
        );

        // All regions start after our DRAM_MEM_START
        assert!(regions.iter().all(|&(start, _)| start.0 >= DRAM_MEM_START));

        // All regions have non-zero length
        assert!(regions.iter().all(|&(_, len)| len > 0));

        // If there's two regions, they perfectly snuggle up the 64bit MMIO gap
        if regions.len() == 2 {
            kani::cover!();

            // The very first address should be DRAM_MEM_START
            assert_eq!(regions[0].0.0, DRAM_MEM_START);
            // The first region ends at the beginning of the 64 bits gap.
            assert_eq!(regions[0].0.0 + regions[0].1 as u64, MMIO64_MEM_START);
            // The second region starts exactly after the 64 bits gap.
            assert_eq!(regions[1].0.0, FIRST_ADDR_PAST_64BITS_MMIO);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch::aarch64::layout::{
        DRAM_MEM_MAX_SIZE, DRAM_MEM_START, FDT_MAX_SIZE, FIRST_ADDR_PAST_64BITS_MMIO,
        MMIO64_MEM_START,
    };
    use crate::test_utils::arch_mem;

    #[cfg(not(feature = "windows"))]
    #[test]
    fn firmware_requires_capability_before_writing_guest_memory() {
        use vmm_sys_util::tempfile::TempFile;

        let file = TempFile::new_in(&std::env::current_dir().unwrap()).unwrap();
        file.as_file().write_all_at(b"_FVH", 0x1028).unwrap();
        let mem = arch_mem(0x80_0000);
        let address = GuestAddress(layout::FIRMWARE_START);
        mem.write_obj(0x55_u8, address).unwrap();
        assert!(matches!(
            load_kernel(file.as_file(), &mem),
            Err(ConfigurationError::WindowsCapability)
        ));
        assert_eq!(mem.read_obj::<u8>(address).unwrap(), 0x55);
    }

    #[test]
    #[cfg(feature = "windows")]
    fn test_firmware_loader_bounds_and_header() {
        use vmm_sys_util::tempfile::TempFile;

        let file = TempFile::new_in(&std::env::current_dir().unwrap()).unwrap();
        let mut image = vec![0_u8; 0x2000];
        let volume = &mut image[0x1000..];
        volume[32..40].copy_from_slice(&0x1000_u64.to_le_bytes());
        volume[40..44].copy_from_slice(b"_FVH");
        volume[48..50].copy_from_slice(&72_u16.to_le_bytes());
        volume[55] = 2;
        volume[56..60].copy_from_slice(&1_u32.to_le_bytes());
        volume[60..64].copy_from_slice(&0x1000_u32.to_le_bytes());
        let checksum = volume[..72]
            .as_chunks::<2>()
            .0
            .iter()
            .fold(0_u16, |sum, word| {
                sum.wrapping_add(u16::from_le_bytes(*word))
            });
        volume[50..52].copy_from_slice(&checksum.wrapping_neg().to_le_bytes());
        file.as_file().write_all_at(&image, 0).unwrap();

        let mem = arch_mem(0x80_0000);
        let address = GuestAddress(layout::FIRMWARE_START);
        let entry = load_kernel(file.as_file(), &mem).unwrap();
        assert_eq!(entry.protocol, BootProtocol::FirmwareBoot);
        assert_eq!(entry.entry_addr, address);
        let mut loaded = vec![0; image.len()];
        mem.read_slice(&mut loaded, address).unwrap();
        assert_eq!(loaded, image);

        mem.write_obj(0x55_u8, address).unwrap();
        file.as_file().write_all_at(&[1], 0x1000).unwrap();
        assert!(matches!(
            load_kernel(file.as_file(), &mem),
            Err(ConfigurationError::FirmwareFormat)
        ));
        assert_eq!(mem.read_obj::<u8>(address).unwrap(), 0x55);

        file.as_file().write_all_at(&image, 0).unwrap();
        file.as_file()
            .write_all_at(&u16::MAX.to_le_bytes(), 0x1030)
            .unwrap();
        assert!(matches!(
            load_kernel(file.as_file(), &mem),
            Err(ConfigurationError::FirmwareFormat)
        ));
        file.as_file()
            .set_len(layout::FIRMWARE_MAX_SIZE + 1)
            .unwrap();
        assert!(matches!(
            load_kernel(file.as_file(), &mem),
            Err(ConfigurationError::FirmwareSize)
        ));
        file.as_file().set_len(0x102c).unwrap();
        assert!(matches!(
            load_kernel(file.as_file(), &mem),
            Err(ConfigurationError::FirmwareSize)
        ));
    }

    #[test]
    fn test_regions_lt_1024gb() {
        let regions = arch_memory_regions(1usize << 29);
        assert_eq!(1, regions.len());
        assert_eq!(GuestAddress(DRAM_MEM_START), regions[0].0);
        assert_eq!(1usize << 29, regions[0].1);
    }

    #[test]
    fn test_regions_gt_1024gb() {
        let regions = arch_memory_regions(1usize << 41);
        assert_eq!(2, regions.len());
        assert_eq!(GuestAddress(DRAM_MEM_START), regions[0].0);
        assert_eq!(MMIO64_MEM_START - DRAM_MEM_START, regions[0].1 as u64);
        assert_eq!(GuestAddress(FIRST_ADDR_PAST_64BITS_MMIO), regions[1].0);
        assert_eq!(
            DRAM_MEM_MAX_SIZE as u64 - MMIO64_MEM_START + DRAM_MEM_START,
            regions[1].1 as u64
        );
    }

    #[test]
    fn test_get_fdt_addr() {
        let mem = arch_mem(FDT_MAX_SIZE - 0x1000);
        assert_eq!(get_fdt_addr(&mem), DRAM_MEM_START);

        let mem = arch_mem(FDT_MAX_SIZE);
        assert_eq!(get_fdt_addr(&mem), DRAM_MEM_START);

        let mem = arch_mem(FDT_MAX_SIZE + 0x1000);
        assert_eq!(get_fdt_addr(&mem), 0x1000 + DRAM_MEM_START);
    }
}
