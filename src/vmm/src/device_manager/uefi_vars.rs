// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Protected-variable transport attachment and fail-closed lifecycle.

use super::DeviceManager;
use crate::devices::uefi_vars::{MMIO_BASE, MMIO_SIZE, UefiVars, UefiVarsState};
use crate::vmm_config::uefi_vars::{UefiVarsConfig, UefiVarsRestoreConfig};
use crate::vstate::bus::BusDevice;
use crate::vstate::vm::KvmVm;
use acpi_tables::{Aml, aml};
use event_manager::{EventOps, Events, MutEventSubscriber};
use std::io;
use std::os::fd::AsRawFd;
use std::sync::{Arc, Barrier, Mutex, Weak};
use std::time::Duration;
use utils::time::TimerFd;
use vmm_sys_util::epoll::EventSet;

#[derive(Debug)]
pub(crate) struct UefiVarsDevice {
    core: UefiVars,
    config: UefiVarsConfig,
    vm: Weak<KvmVm>,
    timer: TimerFd,
    registered: bool,
    armed: bool,
    failed: bool,
}

impl UefiVarsDevice {
    fn schedule(&mut self) {
        let pending = self.core.pending() && !self.failed;
        if pending != self.armed {
            self.armed = pending;
            let interval = Duration::from_millis(1);
            self.timer.arm(
                if pending { interval } else { Duration::ZERO },
                pending.then_some(interval),
            );
        }
    }
    fn fail(&mut self, error: &io::Error) {
        self.failed = true;
        crate::logger::error!("UEFI variable service failed: {error}");
        if let Some(vm) = self.vm.upgrade()
            && let Err(error) = vm.vcpus_exit_evt().write(1)
        {
            crate::logger::error!("Cannot signal variable-service failure: {error}");
        }
        self.schedule();
    }
    pub(crate) fn check_registered(&self) -> io::Result<()> {
        if self.registered {
            Ok(())
        } else {
            Err(io::Error::other("UEFI variable timer registration failed"))
        }
    }
    pub(crate) fn reset_registration(&mut self) {
        self.registered = false;
    }
    fn snapshot(&mut self) -> io::Result<UefiVarsState> {
        let result = self.core.snapshot();
        if let Err(error) = &result
            && error.kind() != io::ErrorKind::WouldBlock
        {
            self.fail(error);
        }
        result
    }
}

impl BusDevice for UefiVarsDevice {
    fn read(&mut self, _base: u64, offset: u64, data: &mut [u8]) {
        self.core.read(offset, data);
    }
    fn write(&mut self, _base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        if let Some(vm) = self.vm.upgrade() {
            self.core.write(offset, data, vm.guest_memory());
        }
        if self.core.failed() {
            self.fail(&io::Error::other(
                "variable-service request could not be queued",
            ));
        }
        self.schedule();
        None
    }
}
impl MutEventSubscriber for UefiVarsDevice {
    fn init(&mut self, ops: &mut EventOps) {
        self.registered = ops.add(Events::new(&self.timer, EventSet::IN)).is_ok();
    }
    fn process(&mut self, event: Events, _ops: &mut EventOps) {
        if event.fd() != self.timer.as_raw_fd() {
            return;
        }
        self.timer.read();
        if let Some(vm) = self.vm.upgrade()
            && let Err(error) = self.core.process(vm.guest_memory())
        {
            self.fail(&error);
        }
        self.schedule();
    }
}

impl DeviceManager {
    pub(crate) fn attach_uefi_vars(
        &mut self,
        vm: &Arc<KvmVm>,
        config: &UefiVarsConfig,
        restore: Option<(&UefiVarsRestoreConfig, &UefiVarsState)>,
    ) -> io::Result<()> {
        config.validate()?;
        let core = match restore {
            Some((restore, state)) => {
                UefiVars::restore(&config.socket_path, restore.expected_generation, state)?
            }
            None => UefiVars::new(&config.socket_path)?,
        };
        let device = Arc::new(Mutex::new(UefiVarsDevice {
            core,
            config: config.clone(),
            vm: Arc::downgrade(vm),
            timer: TimerFd::new(),
            registered: false,
            armed: false,
            failed: false,
        }));
        vm.common
            .mmio_bus
            .insert(device.clone(), MMIO_BASE, MMIO_SIZE)
            .map_err(io::Error::other)?;
        self.uefi_vars = Some(device);
        Ok(())
    }
    pub(crate) fn uefi_vars_config(&self) -> Option<UefiVarsConfig> {
        self.uefi_vars
            .as_ref()
            .map(|device| device.lock().unwrap().config.clone())
    }
    pub(crate) fn snapshot_uefi_vars(&self) -> io::Result<Option<UefiVarsState>> {
        self.uefi_vars
            .as_ref()
            .map(|device| device.lock().unwrap().snapshot())
            .transpose()
    }
    pub(crate) fn uefi_vars_failed(&self) -> bool {
        self.uefi_vars
            .as_ref()
            .is_some_and(|device| device.lock().unwrap().failed)
    }
    pub(crate) fn append_uefi_vars_aml(&self, bytes: &mut Vec<u8>) -> Result<(), aml::AmlError> {
        if self.uefi_vars.is_some() {
            aml::Device::new(
                "_SB_.FVAR".try_into()?,
                vec![
                    &aml::Name::new("_HID".try_into()?, &aml::EisaName::new("PNP0C02")?)?,
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
        }
        Ok(())
    }
}
