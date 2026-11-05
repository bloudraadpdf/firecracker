// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use acpi_tables::arm::{ArmTable, GicV3Topology};
use acpi_tables::{Aml, Fadt, aml};

use super::{AcpiError, AcpiTableWriter};
use crate::arch::aarch64::gic::GICDevice;
use crate::arch::aarch64::layout;
use crate::device_manager::{DeviceManager, VirtioDevices};
use crate::vstate::memory::{GuestAddress, GuestMemoryMmap};
use crate::vstate::resources::ResourceAllocator;

pub(crate) fn rsdp_addr() -> GuestAddress {
    GuestAddress(layout::FIRMWARE_RSDP_ADDR)
}

pub(crate) fn setup_arch_fadt(fadt: &mut Fadt, _devices: &DeviceManager) {
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
    if let Some(gsi) = devices
        .mmio_platform_devices
        .gpio_pl061_device_info()
        .and_then(|device| device.gsi)
    {
        aml::Device::new(
            "_SB_.PWRB".try_into()?,
            vec![&aml::Name::new("_HID".try_into()?, &"PNP0C0C")?],
        )
        .append_aml_bytes(bytes)?;
        super::append_ged(
            "_SB_.PWRG".try_into()?,
            2,
            gsi,
            "\\_SB_.PWRB".try_into()?,
            bytes,
        )?;
    }
    devices.append_tpm_aml(bytes)?;
    devices.append_uefi_vars_aml(bytes)?;
    if devices.console.is_some() {
        use crate::device_manager::console_ramfb::{MMIO_BASE, MMIO_SIZE};
        aml::Device::new(
            "_SB_.FWCF".try_into()?,
            vec![
                &aml::Name::new("_HID".try_into()?, &"QEMU0002")?,
                &aml::Name::new("_STA".try_into()?, &0x0fu8)?,
                &aml::Name::new("_CCA".try_into()?, &aml::ONE)?,
                &aml::Name::new(
                    "_CRS".try_into()?,
                    &aml::ResourceTemplate::new(vec![&aml::Memory32Fixed::new(
                        true,
                        u32::try_from(MMIO_BASE).unwrap(),
                        u32::try_from(MMIO_SIZE).unwrap(),
                    )]),
                )?,
            ],
        )
        .append_aml_bytes(bytes)?;
    }
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
    let last_bus = devices
        .pci_devices()
        .unwrap()
        .pci_segment()
        .last_bus_number();
    let mut writer = AcpiTableWriter { mem };
    let dsdt = writer.build_dsdt(devices, allocator)?;
    let mut tables = vec![
        writer.build_fadt(allocator, dsdt, devices)?,
        writer.build_madt(allocator, topology.madt_records(&cpus.mpidrs), 0)?,
        writer.build_mcfg(allocator, layout::PCI_MMCONFIG_START, last_bus)?,
    ];
    if devices.tpm.is_some() {
        tables.push(writer.build_tpm2(allocator)?);
    }
    for mut table in [
        ArmTable::timer(),
        ArmTable::serial(serial.addr, irq),
        ArmTable::io_remapping(),
    ] {
        tables.push(writer.write_acpi_table(allocator, &mut table)?);
    }
    let xsdt = writer.build_xsdt(allocator, tables)?;
    writer.build_rsdp(xsdt, None)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::device_manager::mmio::{MMIO_LEN, MMIODevice, MMIODeviceInfo};
    use crate::devices::legacy::PL061Device;

    fn power_button() -> MMIODevice<PL061Device> {
        MMIODevice {
            resources: MMIODeviceInfo {
                addr: layout::GPIO_PL061_MEM_START,
                len: MMIO_LEN,
                gsi: Some(5),
            },
            inner: Arc::new(Mutex::new(PL061Device::new().unwrap())),
            sub_id: None,
        }
    }

    #[test]
    fn firmware_power_button_declares_and_executes_acpi_event() {
        let mut devices = crate::device_manager::tests::default_device_manager();
        devices.mmio_platform_devices.gpio_pl061 = Some(power_button());
        let mut bytes = Vec::new();
        setup_arch_dsdt(&mut bytes, &devices).unwrap();
        assert!(bytes.windows(7).any(|value| value == b"PNP0C0C"));
        assert!(
            bytes
                .windows(9)
                .any(|value| value == [0x89, 6, 0, 3, 1, 37, 0, 0, 0])
        );
        let output = crate::test_utils::execute_aml(bytes, "execute \\_SB_.PWRG._EVT 37");
        assert!(
            output
                .lines()
                .any(|line| line.contains("Received a Device Notify on [PWRB]")
                    && line.contains("Value 0x80")),
            "{output}"
        );
    }

    #[test]
    fn firmware_power_button_signals_without_a_gpio_driver() {
        for firmware in [false, true] {
            let mut vmm = crate::builder::tests::default_vmm();
            let device = power_button();
            let gpio = device.inner.clone();
            let before = gpio.lock().unwrap().state();
            vmm.device_manager.mmio_platform_devices.gpio_pl061 = Some(device);
            if firmware {
                vmm.boot_source_config.firmware = Some(
                    serde_json::from_value(serde_json::json!({
                        "type": "cloud-hv",
                        "system_uuid": "11223344-5566-7788-99aa-bbccddeeff01"
                    }))
                    .unwrap(),
                );
            }
            vmm.send_ctrl_alt_del().unwrap();
            let gpio = gpio.lock().unwrap();
            if firmware {
                assert_eq!(gpio.interrupt_evt.read().unwrap(), 1);
                assert_eq!(gpio.state(), before);
            } else {
                assert_eq!(
                    gpio.interrupt_evt.read().unwrap_err().kind(),
                    std::io::ErrorKind::WouldBlock
                );
                assert_ne!(gpio.state(), before);
            }
        }
    }
}
