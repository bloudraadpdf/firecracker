// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Optional CRB attachment and bounded event-loop progress.

use std::io;
use std::os::fd::AsRawFd;
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use acpi_tables::{Aml, aml};
use event_manager::{EventOps, Events, MutEventSubscriber};
use utils::time::TimerFd;
use vmm_sys_util::epoll::EventSet;

use super::DeviceManager;
use crate::devices::tpm::{MMIO_BASE, MMIO_SIZE, Tpm, TpmState};
use crate::vmm_config::tpm::TpmConfig;
use crate::vstate::bus::BusDevice;
use crate::vstate::vm::KvmVm;

#[derive(Debug)]
pub(crate) struct TpmDevice {
    core: Tpm,
    config: TpmConfig,
    timer: TimerFd,
    registered: bool,
    armed: bool,
}

impl TpmDevice {
    fn schedule(&mut self) {
        let pending = self.core.pending();
        if pending != self.armed {
            self.armed = pending;
            let interval = Duration::from_millis(1);
            self.timer.arm(
                if pending { interval } else { Duration::ZERO },
                pending.then_some(interval),
            );
        }
    }

    pub(crate) fn check_registered(&self) -> io::Result<()> {
        if self.registered {
            Ok(())
        } else {
            Err(io::Error::other("TPM timer registration failed"))
        }
    }

    pub(crate) fn reset_registration(&mut self) {
        self.registered = false;
    }
}

impl BusDevice for TpmDevice {
    fn read(&mut self, _base: u64, offset: u64, data: &mut [u8]) {
        self.core.read(offset, data);
    }

    fn write(&mut self, _base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        self.core.write(offset, data);
        self.schedule();
        None
    }
}

impl MutEventSubscriber for TpmDevice {
    fn init(&mut self, ops: &mut EventOps) {
        self.registered = ops.add(Events::new(&self.timer, EventSet::IN)).is_ok();
    }

    fn process(&mut self, event: Events, _ops: &mut EventOps) {
        if event.fd() == self.timer.as_raw_fd() {
            self.timer.read();
            self.core.process();
            self.schedule();
        }
    }
}

impl DeviceManager {
    #[cfg(any(target_arch = "aarch64", test))]
    pub(crate) fn validate_security_mapping(
        base: u64,
        length: u64,
        tpm: bool,
        variables: bool,
    ) -> io::Result<()> {
        let end = length
            .checked_sub(1)
            .and_then(|last| base.checked_add(last));
        let Some(end) = end else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid MMIO mapping range",
            ));
        };
        for address in security_pages(tpm, variables) {
            if base < address + MMIO_SIZE && address <= end {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "device mapping overlaps a security page",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn attach_tpm(
        &mut self,
        vm: &KvmVm,
        config: &TpmConfig,
        state: Option<&TpmState>,
    ) -> io::Result<()> {
        config.validate()?;
        let core = match state {
            Some(state) => Tpm::restore(&config.socket_path, state)?,
            None => Tpm::new(&config.socket_path)?,
        };
        let device = Arc::new(Mutex::new(TpmDevice {
            core,
            config: config.clone(),
            timer: TimerFd::new(),
            registered: false,
            armed: false,
        }));
        vm.common
            .mmio_bus
            .insert(device.clone(), MMIO_BASE, MMIO_SIZE)
            .map_err(io::Error::other)?;
        self.tpm = Some(device);
        Ok(())
    }

    pub(crate) fn tpm_config(&self) -> Option<TpmConfig> {
        self.tpm
            .as_ref()
            .map(|device| device.lock().unwrap().config.clone())
    }

    pub(crate) fn snapshot_tpm(&self) -> io::Result<Option<TpmState>> {
        self.tpm
            .as_ref()
            .map(|device| device.lock().unwrap().core.snapshot())
            .transpose()
    }

    pub(crate) fn append_tpm_aml(&self, bytes: &mut Vec<u8>) -> Result<(), aml::AmlError> {
        if self.tpm.is_some() {
            aml::Device::new(
                "_SB_.TPM0".try_into()?,
                vec![
                    &aml::Name::new("_HID".try_into()?, &"MSFT0101")?,
                    &aml::Name::new("_STA".try_into()?, &0x0fu8)?,
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
            if self.uefi_vars.is_some() {
                super::tpm_ppi::append(bytes)?;
            }
        }
        Ok(())
    }
}

#[cfg(any(target_arch = "aarch64", test))]
fn security_pages(tpm: bool, variables: bool) -> impl Iterator<Item = u64> {
    [
        tpm.then_some(MMIO_BASE),
        variables.then_some(crate::devices::uefi_vars::MMIO_BASE),
    ]
    .into_iter()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_device_mappings_cannot_alias_selected_security_pages() {
        use crate::devices::uefi_vars;

        for base in [MMIO_BASE, uefi_vars::MMIO_BASE] {
            DeviceManager::validate_security_mapping(base, MMIO_SIZE, false, false).unwrap();
            assert!(DeviceManager::validate_security_mapping(base, MMIO_SIZE, true, true).is_err());
            assert!(DeviceManager::validate_security_mapping(base - 1, 2, true, true).is_err());
        }
        for base in [MMIO_BASE - MMIO_SIZE, MMIO_BASE + MMIO_SIZE] {
            DeviceManager::validate_security_mapping(base, MMIO_SIZE, true, false).unwrap();
        }
        assert!(DeviceManager::validate_security_mapping(u64::MAX, 2, true, true).is_err());
        assert!(DeviceManager::validate_security_mapping(0, 0, true, true).is_err());
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn security_pages_are_outside_allocators_and_all_fixed_platform_ranges() {
        use crate::arch::aarch64::layout::*;
        use crate::devices::uefi_vars;
        use crate::vstate::resources::ResourceAllocator;
        use vm_allocator::AllocPolicy;

        assert_eq!(MMIO_BASE, 0x1000_0000);
        assert_eq!(uefi_vars::MMIO_BASE, 0x1000_1000);
        let mut resources = ResourceAllocator::new();
        let ranges = [
            (
                resources.mmio32_memory.base(),
                resources.mmio32_memory.end() + 1,
            ),
            (
                resources.mmio64_memory.base(),
                resources.mmio64_memory.end() + 1,
            ),
            (
                resources.past_mmio64_memory.base(),
                resources.past_mmio64_memory.end() + 1,
            ),
            (
                resources.system_memory.base(),
                resources.system_memory.end() + 1,
            ),
            (BOOT_DEVICE_MEM_START, BOOT_DEVICE_MEM_START + MMIO_SIZE),
            (RTC_MEM_START, RTC_MEM_START + MMIO_SIZE),
            (SERIAL_MEM_START, SERIAL_MEM_START + MMIO_SIZE),
            (PCI_MMCONFIG_START, PCI_MMCONFIG_START + PCI_MMCONFIG_SIZE),
            (
                DRAM_MEM_START,
                DRAM_MEM_START + u64::try_from(DRAM_MEM_MAX_SIZE).unwrap(),
            ),
            (FIRMWARE_FDT_ADDR, FIRMWARE_FDT_ADDR + 0x10_0000),
            (FIRMWARE_SMBIOS_ADDR, FIRMWARE_SMBIOS_ADDR + MMIO_SIZE),
            (FIRMWARE_RSDP_ADDR, FIRMWARE_RSDP_ADDR + 0x1_0000),
            (FIRMWARE_START, FIRMWARE_START + FIRMWARE_MAX_SIZE),
        ];
        for base in [MMIO_BASE, uefi_vars::MMIO_BASE] {
            assert!(base.is_multiple_of(MMIO_SIZE));
            assert!(base >= 0x0040_0000 && base + MMIO_SIZE <= DRAM_MEM_START);
            for (start, end) in ranges {
                assert!(
                    base + MMIO_SIZE <= start || end <= base,
                    "{base:x} overlaps {start:x}..{end:x}"
                );
            }
        }
        assert_eq!(
            resources
                .mmio32_memory
                .allocate(MMIO_SIZE, MMIO_SIZE, AllocPolicy::FirstMatch)
                .unwrap()
                .start(),
            MEM_32BIT_DEVICES_START
        );
        assert_eq!(resources.mmio32_memory.base(), 0x4000_3000);
    }
}
