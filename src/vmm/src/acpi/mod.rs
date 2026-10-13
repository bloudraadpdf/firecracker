// Copyright 2024 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use acpi_tables::fadt::{FADT_F_HW_REDUCED_ACPI, FADT_F_PWR_BUTTON, FADT_F_SLP_BUTTON};
use acpi_tables::{Dsdt, Fadt, Madt, Mcfg, RootTable, RootTableEntries, Rsdp, Sdt, aml};
use vm_allocator::AllocPolicy;

#[cfg(target_arch = "aarch64")]
pub(crate) use self::aarch64::create_acpi_tables;
#[cfg(target_arch = "aarch64")]
use self::aarch64::{rsdp_addr, setup_arch_dsdt, setup_arch_fadt};
#[cfg(target_arch = "x86_64")]
use crate::Vcpu;
#[cfg(target_arch = "x86_64")]
use crate::acpi::x86_64::{
    apic_addr, rsdp_addr, setup_arch_dsdt, setup_arch_fadt, setup_interrupt_controllers,
};
#[cfg(target_arch = "x86_64")]
use crate::arch::x86_64::layout;
use crate::device_manager::DeviceManager;
use crate::logger::{debug, error};
use crate::vstate::memory::{GuestAddress, GuestMemoryMmap};
use crate::vstate::resources::ResourceAllocator;

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "x86_64")]
mod x86_64;

// Our (Original Equipment Manufacturer" (OEM) name. OEM is how ACPI names the manufacturer of the
// hardware that is exposed to the OS, through ACPI tables. The OEM name is passed in every ACPI
// table, to let the OS know that we are the owner of the table.
const OEM_ID: [u8; 6] = *b"FIRECK";

// In reality the OEM revision is per table and it defines the revision of the OEM's implementation
// of the particular ACPI table. For our purpose, we can set it to a fixed value for all the tables
const OEM_REVISION: u32 = 0;

// This is needed for an entry in the FADT table. Populating this entry in FADT is a way to let the
// guest know that it runs within a Firecracker microVM.
const HYPERVISOR_VENDOR_ID: [u8; 8] = *b"FIRECKVM";

#[derive(Debug, thiserror::Error, displaydoc::Display)]
/// Error type for ACPI related operations
pub enum AcpiError {
    /// Arm firmware requires PCI, a serial port and GICv3 with ITS
    UnsupportedArmPlatform,
    /// Could not allocate resources: {0}
    VmAllocator(#[from] vm_allocator::Error),
    /// ACPI tables error: {0}
    AcpiTables(#[from] acpi_tables::AcpiError),
    /// Error creating AML bytecode: {0}
    AmlError(#[from] aml::AmlError),
}

/// Helper type that holds the guest memory in which we write the tables in and a resource
/// allocator for allocating space for the tables
struct AcpiTableWriter<'a> {
    mem: &'a GuestMemoryMmap,
}

impl AcpiTableWriter<'_> {
    /// Write a table in guest memory
    ///
    /// This will allocate enough space inside guest memory and write the table in the allocated
    /// buffer. It returns the address in which it wrote the table.
    fn write_acpi_table<S>(
        &mut self,
        resource_allocator: &mut ResourceAllocator,
        table: &mut S,
    ) -> Result<u64, AcpiError>
    where
        S: Sdt,
    {
        let addr = resource_allocator
            .system_memory
            .allocate(
                table.len().try_into().unwrap(),
                table.alignment(),
                AllocPolicy::FirstMatch,
            )?
            .start();

        table
            .write_to_guest(self.mem, GuestAddress(addr))
            .inspect_err(|err| error!("acpi: Could not write table in guest memory: {err}"))?;

        debug!(
            "acpi: Wrote table ({} bytes) at address: {:#010x}",
            table.len(),
            addr
        );

        Ok(addr)
    }

    /// Build the DSDT table for the guest
    fn build_dsdt(
        &mut self,
        device_manager: &mut DeviceManager,
        resource_allocator: &mut ResourceAllocator,
    ) -> Result<u64, AcpiError> {
        let mut dsdt_data = Vec::new();

        // Device DSDT data.
        #[cfg(target_arch = "x86_64")]
        device_manager.append_aml_bytes(&mut dsdt_data)?;

        // Architecture specific DSDT data
        setup_arch_dsdt(&mut dsdt_data, device_manager)?;

        let mut dsdt = Dsdt::new(OEM_ID, *b"FCVMDSDT", OEM_REVISION, dsdt_data);
        self.write_acpi_table(resource_allocator, &mut dsdt)
    }

    /// Build the FADT table for the guest
    ///
    /// This includes a pointer with the location of the DSDT in guest memory
    fn build_fadt(
        &mut self,
        resource_allocator: &mut ResourceAllocator,
        dsdt_addr: u64,
    ) -> Result<u64, AcpiError> {
        let mut fadt = Fadt::new(OEM_ID, *b"FCVMFADT", OEM_REVISION);
        fadt.set_hypervisor_vendor_id(HYPERVISOR_VENDOR_ID);
        fadt.set_x_dsdt(dsdt_addr);
        fadt.set_flags(
            (1 << FADT_F_HW_REDUCED_ACPI) | (1 << FADT_F_PWR_BUTTON) | (1 << FADT_F_SLP_BUTTON),
        );
        setup_arch_fadt(&mut fadt);
        #[cfg(target_arch = "x86_64")]
        {
            let facs_addr = self.write_acpi_table(resource_allocator, &mut acpi_tables::Facs)?;
            fadt.set_facs(facs_addr);
        }
        self.write_acpi_table(resource_allocator, &mut fadt)
    }

    /// Build the MADT table for the guest
    ///
    /// This includes information about the interrupt controllers supported in the platform
    fn build_madt(
        &mut self,
        resource_allocator: &mut ResourceAllocator,
        interrupt_controllers: Vec<u8>,
        apic_address: u32,
    ) -> Result<u64, AcpiError> {
        let mut madt = Madt::new(
            OEM_ID,
            *b"FCVMMADT",
            OEM_REVISION,
            apic_address,
            cfg!(target_arch = "x86_64"),
            interrupt_controllers,
        );
        self.write_acpi_table(resource_allocator, &mut madt)
    }

    /// Build the XSDT table for the guest
    fn build_xsdt(
        &mut self,
        resource_allocator: &mut ResourceAllocator,
        tables: Vec<u64>,
    ) -> Result<u64, AcpiError> {
        let mut xsdt = RootTable::new(
            OEM_ID,
            *b"FCMVXSDT",
            OEM_REVISION,
            RootTableEntries::Xsdt(tables),
        );
        self.write_acpi_table(resource_allocator, &mut xsdt)
    }

    /// Build the MCFG table for the guest.
    fn build_mcfg(
        &mut self,
        resource_allocator: &mut ResourceAllocator,
        pci_mmio_config_addr: u64,
    ) -> Result<u64, AcpiError> {
        let mut mcfg = Mcfg::new(OEM_ID, *b"FCMVMCFG", OEM_REVISION, pci_mmio_config_addr);
        self.write_acpi_table(resource_allocator, &mut mcfg)
    }

    /// Build the RSDP pointer for the guest.
    ///
    /// This will build the RSDP pointer which points to the root tables and write it in guest
    /// memory. The address in which we write RSDP is pre-determined for every architecture.
    /// We will not allocate arbitrary memory for it
    fn build_rsdp(&mut self, xsdt_addr: u64, rsdt_addr: Option<u32>) -> Result<(), AcpiError> {
        let mut rsdp = Rsdp::new(OEM_ID, xsdt_addr, rsdt_addr);
        rsdp.write_to_guest(self.mem, rsdp_addr())
            .inspect_err(|err| error!("acpi: Could not write RSDP in guest memory: {err}"))?;

        debug!(
            "acpi: Wrote RSDP ({} bytes) at address: {:#010x}",
            rsdp.len(),
            rsdp_addr().0
        );
        Ok(())
    }
}

/// Create ACPI tables for the guest
///
/// This will create the ACPI tables needed to describe to the guest OS the available hardware,
/// such as interrupt controllers, vCPUs and VirtIO devices.
#[cfg(target_arch = "x86_64")]
pub(crate) fn create_acpi_tables(
    mem: &GuestMemoryMmap,
    device_manager: &mut DeviceManager,
    resource_allocator: &mut ResourceAllocator,
    vcpus: &[Vcpu],
) -> Result<(), AcpiError> {
    let mut writer = AcpiTableWriter { mem };
    let dsdt_addr = writer.build_dsdt(device_manager, resource_allocator)?;

    let fadt_addr = writer.build_fadt(resource_allocator, dsdt_addr)?;
    let madt_addr = writer.build_madt(
        resource_allocator,
        setup_interrupt_controllers(vcpus.len().try_into().unwrap()),
        apic_addr(),
    )?;
    let mcfg_addr = writer.build_mcfg(resource_allocator, layout::PCI_MMCONFIG_START)?;
    let xsdt_addr = writer.build_xsdt(resource_allocator, vec![fadt_addr, madt_addr, mcfg_addr])?;
    let tables = [fadt_addr, madt_addr, mcfg_addr]
        .into_iter()
        .map(u32::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| acpi_tables::AcpiError::InvalidGuestAddress)?;
    let mut rsdt = RootTable::new(
        OEM_ID,
        *b"FCVMRSDT",
        OEM_REVISION,
        RootTableEntries::Rsdt(tables),
    );
    let rsdt_addr = u32::try_from(writer.write_acpi_table(resource_allocator, &mut rsdt)?)
        .map_err(|_| acpi_tables::AcpiError::InvalidGuestAddress)?;
    writer.build_rsdp(xsdt_addr, Some(rsdt_addr))
}

#[cfg(test)]
#[cfg(target_arch = "x86_64")]
mod tests {
    use acpi_tables::Sdt;
    use vm_memory::Bytes;

    use crate::acpi::{AcpiError, AcpiTableWriter};
    use crate::arch::x86_64::layout::{SYSTEM_MEM_SIZE, SYSTEM_MEM_START};
    use crate::builder::tests::default_vmm;
    use crate::utils::u64_to_usize;
    use crate::vstate::resources::ResourceAllocator;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    struct MockSdt(Vec<u8>);

    #[test]
    fn x86_root_tables_include_32_bit_addresses() {
        let mut vmm = default_vmm();
        let vm = vmm.vm.as_kvm().unwrap().clone();
        super::create_acpi_tables(
            vm.guest_memory(),
            &mut vmm.device_manager,
            &mut vm.resource_allocator(),
            &[],
        )
        .unwrap();
        let read = |address, length| {
            let mut bytes = vec![0u8; length];
            vm.guest_memory()
                .read_slice(&mut bytes, vm_memory::GuestAddress(address))
                .unwrap();
            bytes
        };
        let rsdp = read(super::rsdp_addr().0, 36);
        let rsdt_address = u32::from_le_bytes(rsdp[16..20].try_into().unwrap());
        assert_ne!(rsdt_address, 0);
        let xsdt_address = u64::from_le_bytes(rsdp[24..32].try_into().unwrap());
        let rsdt = read(u64::from(rsdt_address), 48);
        let xsdt = read(xsdt_address, 60);
        assert_eq!(&rsdt[..4], b"RSDT");
        assert_eq!(&xsdt[..4], b"XSDT");
        for table in [&rsdt[..], &xsdt[..]] {
            assert_eq!(
                u32::from_le_bytes(table[4..8].try_into().unwrap()) as usize,
                table.len()
            );
        }
        for bytes in [&rsdp[..20], &rsdp[..], &rsdt[..], &xsdt[..]] {
            assert_eq!(
                bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
                0
            );
        }
        for (legacy, extended) in rsdt[36..]
            .as_chunks::<4>()
            .0
            .iter()
            .zip(xsdt[36..].as_chunks::<8>().0)
        {
            assert_eq!(
                u64::from(u32::from_le_bytes(*legacy)),
                u64::from_le_bytes(*extended)
            );
        }
    }

    #[test]
    fn test_conventional_fadt_and_aligned_facs() {
        let vmm = default_vmm();
        let mut writer = AcpiTableWriter {
            mem: vmm.vm.as_kvm().unwrap().guest_memory(),
        };
        let mut allocator = vmm.vm.as_kvm().unwrap().resource_allocator();
        writer
            .write_acpi_table(&mut allocator, &mut MockSdt(vec![0; 3]))
            .unwrap();
        let address = writer.build_fadt(&mut allocator, 0xf1000).unwrap();
        let mut table = [0u8; 276];
        writer
            .mem
            .read_slice(&mut table, vm_memory::GuestAddress(address))
            .unwrap();
        assert_eq!(
            table.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
            0
        );
        let flags = u32::from_le_bytes(table[112..116].try_into().unwrap());
        assert_eq!(flags & (1 << 20), 0);
        assert_ne!(flags & (1 << 8), 0);
        assert_eq!(u16::from_le_bytes(table[46..48].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(table[56..60].try_into().unwrap()), 0x602);
        assert_eq!(u32::from_le_bytes(table[64..68].try_into().unwrap()), 0x606);
        assert_eq!(u32::from_le_bytes(table[76..80].try_into().unwrap()), 0x608);
        assert_eq!(&table[88..92], &[4, 2, 0, 4]);
        let facs_address = u64::from(u32::from_le_bytes(table[36..40].try_into().unwrap()));
        assert_ne!(facs_address, 0);
        assert_eq!(facs_address % 64, 0);
        assert_eq!(u64::from_le_bytes(table[132..140].try_into().unwrap()), 0);
        let mut facs = [0u8; 64];
        writer
            .mem
            .read_slice(&mut facs, vm_memory::GuestAddress(facs_address))
            .unwrap();
        assert_eq!(&facs[..8], b"FACS\x40\0\0\0");
        assert_eq!(facs[32], 2);
    }

    #[test]
    fn test_madt_declares_legacy_pic() {
        let vmm = default_vmm();
        let mut writer = AcpiTableWriter {
            mem: vmm.vm.as_kvm().unwrap().guest_memory(),
        };
        let address = writer
            .build_madt(
                &mut vmm.vm.as_kvm().unwrap().resource_allocator(),
                super::setup_interrupt_controllers(1),
                super::apic_addr(),
            )
            .unwrap();
        let mut table = [0u8; 74];
        writer
            .mem
            .read_slice(&mut table, vm_memory::GuestAddress(address))
            .unwrap();
        assert_eq!(&table[..4], b"APIC");
        assert_eq!(u32::from_le_bytes(table[4..8].try_into().unwrap()), 74);
        assert_eq!(u32::from_le_bytes(table[40..44].try_into().unwrap()), 1);
        assert_eq!(&table[64..], &[2, 10, 0, 3, 3, 0, 0, 0, 15, 0]);
        assert_eq!(
            table.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
            0
        );
    }

    impl Sdt for MockSdt {
        fn len(&self) -> usize {
            self.0.len()
        }

        fn write_to_guest<M: vm_memory::GuestMemory>(
            &mut self,
            mem: &M,
            address: vm_memory::GuestAddress,
        ) -> acpi_tables::Result<()> {
            mem.write_slice(&self.0, address)?;
            Ok(())
        }
    }

    // Currently we are allocating up to SYSTEM_MEM_SIZE memory for ACPI tables. We are allocating
    // using the FirstMatch policy, with an 1 byte alignment. This test checks that we are able to
    // allocate up to this size, and get back the expected addresses.
    #[test]
    fn test_write_acpi_table_memory_allocation() {
        // A mocke Vmm object with 128MBs of memory
        let vmm = default_vmm();
        let mut writer = AcpiTableWriter {
            mem: vmm.vm.as_kvm().unwrap().guest_memory(),
        };
        let mut resource_allocator = vmm.vm.as_kvm().unwrap().resource_allocator();

        // This should succeed
        let mut sdt = MockSdt(vec![0; 4096]);
        let addr = writer
            .write_acpi_table(&mut resource_allocator, &mut sdt)
            .unwrap();
        assert_eq!(addr, SYSTEM_MEM_START);

        // Let's try to write two 4K pages plus one byte
        let mut sdt = MockSdt(vec![0; usize::try_from(SYSTEM_MEM_SIZE + 1).unwrap()]);
        let err = writer
            .write_acpi_table(&mut resource_allocator, &mut sdt)
            .unwrap_err();
        assert!(
            matches!(
                err,
                AcpiError::VmAllocator(vm_allocator::Error::ResourceNotAvailable)
            ),
            "{:?}",
            err
        );

        // We are allocating memory for tables with alignment of 1 byte. All of these should
        // succeed.
        let mut sdt = MockSdt(vec![0; 5]);
        let addr = writer
            .write_acpi_table(&mut resource_allocator, &mut sdt)
            .unwrap();
        assert_eq!(addr, SYSTEM_MEM_START + 4096);
        let mut sdt = MockSdt(vec![0; 2]);
        let addr = writer
            .write_acpi_table(&mut resource_allocator, &mut sdt)
            .unwrap();
        assert_eq!(addr, SYSTEM_MEM_START + 4101);
        let mut sdt = MockSdt(vec![0; 4]);
        let addr = writer
            .write_acpi_table(&mut resource_allocator, &mut sdt)
            .unwrap();
        assert_eq!(addr, SYSTEM_MEM_START + 4103);
        let mut sdt = MockSdt(vec![0; 8]);
        let addr = writer
            .write_acpi_table(&mut resource_allocator, &mut sdt)
            .unwrap();
        assert_eq!(addr, SYSTEM_MEM_START + 4107);
        let mut sdt = MockSdt(vec![0; 16]);
        let addr = writer
            .write_acpi_table(&mut resource_allocator, &mut sdt)
            .unwrap();
        assert_eq!(addr, SYSTEM_MEM_START + 4115);
    }

    // If, for whatever weird reason, we end up with microVM that has less memory than the maximum
    // address we allocate for ACPI tables, we would be able to allocate the tables but we would
    // not be able to write them. This is practically impossible in our case. If we get such a
    // guest memory, we won't be able to load the guest kernel, but the function does
    // return an error on this case, so let's just check that in case any of these assumptions
    // change in the future.
    #[test]
    fn test_write_acpi_table_small_memory() {
        let vm = setup_vm_with_memory(u64_to_usize(SYSTEM_MEM_START + SYSTEM_MEM_SIZE - 4096));
        let mut writer = AcpiTableWriter {
            mem: vm.guest_memory(),
        };
        let mut resource_allocator = ResourceAllocator::new();

        let mut sdt = MockSdt(vec![0; usize::try_from(SYSTEM_MEM_SIZE).unwrap()]);
        let err = writer
            .write_acpi_table(&mut resource_allocator, &mut sdt)
            .unwrap_err();
        assert!(
            matches!(
                err,
                AcpiError::AcpiTables(acpi_tables::AcpiError::GuestMemory(
                    vm_memory::GuestMemoryError::PartialBuffer {
                        expected: 263168,  // SYSTEM_MEM_SIZE
                        completed: 259072  // SYSTEM_MEM_SIZE - 4096
                    },
                ))
            ),
            "{:?}",
            err
        );
    }
}
