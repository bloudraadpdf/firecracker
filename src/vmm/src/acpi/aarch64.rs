// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use acpi_tables::arm::{ArmTable, GicV3Topology};
use acpi_tables::{Aml, Fadt, aml};

use super::{AcpiError, AcpiTableWriter};
use crate::arch::aarch64::{gic::GICDevice, layout};
use crate::device_manager::{DeviceManager, VirtioDevices};
use crate::vstate::memory::{GuestAddress, GuestMemoryMmap};
use crate::vstate::resources::ResourceAllocator;

pub(crate) fn rsdp_addr() -> GuestAddress {
    GuestAddress(layout::FIRMWARE_RSDP_ADDR)
}

pub(crate) fn setup_arch_fadt(fadt: &mut Fadt) {
    fadt.set_arm_psci_hvc();
}

pub(crate) fn setup_arch_dsdt(
    bytes: &mut Vec<u8>,
    devices: &DeviceManager,
) -> Result<(), aml::AmlError> {
    if let VirtioDevices::Pci(pci) = &devices.virtio_devices {
        pci.append_aml_bytes(bytes)?;
    }
    devices.acpi_devices.append_aml_bytes(bytes)?;
    if let Some(serial) = devices.mmio_platform_devices.serial_device_info() {
        aml::Device::new(
            "_SB_.COM1".try_into()?,
            vec![
                &aml::Name::new("_HID".try_into()?, &aml::EisaName::new("PNP0501")?)?,
                &aml::Name::new("_UID".try_into()?, &0_u8)?,
                &aml::Name::new("_CCA".try_into()?, &aml::ONE)?,
                &aml::Name::new(
                    "_CRS".try_into()?,
                    &aml::ResourceTemplate::new(vec![
                        &aml::Memory32Fixed::new(true, serial.addr.try_into().unwrap(), 8),
                        &aml::Interrupt::new(
                            true,
                            true,
                            false,
                            false,
                            serial.gsi.unwrap() + layout::SPI_START,
                        ),
                    ]),
                )?,
            ],
        )
        .append_aml_bytes(bytes)?;
    }
    Ok(())
}

pub(crate) fn create_acpi_tables(
    mem: &GuestMemoryMmap,
    devices: &mut DeviceManager,
    allocator: &mut ResourceAllocator,
    cpus: &crate::arch::aarch64::CpuTopology,
    gic: &GICDevice,
) -> Result<(), AcpiError> {
    if !matches!(devices.virtio_devices, VirtioDevices::Pci(_)) || !matches!(gic, GICDevice::V3(_))
    {
        return Err(AcpiError::UnsupportedArmPlatform);
    }
    let serial = *devices
        .mmio_platform_devices
        .serial_device_info()
        .ok_or(AcpiError::UnsupportedArmPlatform)?;
    let irq = serial.gsi.ok_or(AcpiError::UnsupportedArmPlatform)? + layout::SPI_START;
    let its = gic
        .msi_properties()
        .ok_or(AcpiError::UnsupportedArmPlatform)?[0];
    let properties = gic.device_properties();
    let topology = GicV3Topology {
        distributor: properties[0],
        redistributors: properties[2],
        redistributors_size: properties[3].try_into().unwrap(),
        its,
        pmu_irq: cpus.pmu_irq,
    };
    let mut writer = AcpiTableWriter { mem };
    let dsdt = writer.build_dsdt(devices, allocator)?;
    let mut tables = vec![
        writer.build_fadt(allocator, dsdt)?,
        writer.build_madt(allocator, topology.madt_records(&cpus.mpidrs), 0)?,
        writer.build_mcfg(allocator, layout::PCI_MMCONFIG_START)?,
    ];
    for mut table in [
        ArmTable::timer(),
        ArmTable::serial(serial.addr, irq),
        ArmTable::io_remapping(),
    ] {
        tables.push(writer.write_acpi_table(allocator, &mut table)?);
    }
    let xsdt = writer.build_xsdt(allocator, tables)?;
    writer.build_rsdp(xsdt)
}
