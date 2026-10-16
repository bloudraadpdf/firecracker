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
        }
        Ok(())
    }
}
