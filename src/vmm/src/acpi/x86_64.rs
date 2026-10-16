// Copyright 2024 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::mem::size_of;

#[cfg(feature = "windows")]
use acpi_tables::fadt::IAPC_BOOT_ARG_FLAGS_8042;
use acpi_tables::fadt::IAPC_BOOT_ARG_FLAGS_VGA_NOT_PRESENT;
use acpi_tables::madt::{InterruptSourceOverride, IoAPIC, LocalAPIC};
use acpi_tables::{Aml, Fadt, GenericAddressStructure, aml};
use vm_memory::GuestAddress;
use zerocopy::IntoBytes;

use crate::arch::x86_64::layout;
use crate::devices::legacy::acpi::{
    PM1_CONTROL_PORT, PM1_EVENT_PORT, POWER_PORT, SCI_GSI, TIMER_PORT,
};

#[inline(always)]
pub(crate) fn setup_interrupt_controllers(nr_vcpus: u8, pc_enabled: bool) -> Vec<u8> {
    let mut ic =
        Vec::with_capacity(size_of::<IoAPIC>() + (nr_vcpus as usize) * size_of::<LocalAPIC>());

    ic.extend_from_slice(IoAPIC::new(0, layout::IOAPIC_ADDR).as_bytes());
    for i in 0..nr_vcpus {
        ic.extend_from_slice(LocalAPIC::new(i).as_bytes());
    }
    if pc_enabled {
        ic.extend_from_slice(
            InterruptSourceOverride::level_low(SCI_GSI.try_into().unwrap(), SCI_GSI).as_bytes(),
        );
    }
    ic
}

#[inline(always)]
pub(crate) fn setup_arch_fadt(fadt: &mut Fadt, devices: &crate::device_manager::DeviceManager) {
    let flags = 1 << IAPC_BOOT_ARG_FLAGS_VGA_NOT_PRESENT;
    #[cfg(feature = "windows")]
    let flags = if devices.console.is_some() {
        1 << IAPC_BOOT_ARG_FLAGS_8042
    } else {
        flags
    };
    fadt.setup_iapc_flags(flags);
    if !devices.has_pc_devices() {
        return;
    }
    let control = GenericAddressStructure::new(1, 8, 0, 1, POWER_PORT);
    fadt.set_reset(control, 1);
    fadt.set_pm_registers(
        SCI_GSI.try_into().unwrap(),
        PM1_EVENT_PORT.try_into().unwrap(),
        PM1_CONTROL_PORT.try_into().unwrap(),
        TIMER_PORT.try_into().unwrap(),
    );
}

#[inline(always)]
pub(crate) fn setup_arch_dsdt(
    dsdt_data: &mut Vec<u8>,
    devices: &crate::device_manager::DeviceManager,
) -> Result<(), aml::AmlError> {
    if devices.has_pc_devices() {
        aml::Name::new(
            "_S5_".try_into()?,
            &aml::Package::new(vec![&5u8, &0u8, &0u8, &0u8]),
        )?
        .append_aml_bytes(dsdt_data)?;
    }
    if let Some(legacy) = &devices.legacy_devices {
        legacy.append_aml_bytes(dsdt_data, devices.acpi_device_scope())?;
    }
    Ok(())
}

pub(crate) const fn apic_addr() -> u32 {
    layout::APIC_ADDR
}

pub(crate) const fn rsdp_addr() -> GuestAddress {
    GuestAddress(layout::RSDP_ADDR)
}
