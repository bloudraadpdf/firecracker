// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Transactional event registration for optional platform devices.

use super::console_server::ConsoleServer;
use super::console_usb::UsbDevice;
use super::{DeviceManager, VirtioDevices};
use crate::EventManager;
use crate::pci::intx::PciIntxGroup;
use event_manager::{SubscriberId, SubscriberOps};
use std::io;
use std::sync::{Arc, Mutex};

/// Rolls back the platform subscriptions if subsequent VM startup fails.
pub(crate) struct PlatformRegistration<'a> {
    events: &'a mut EventManager,
    subscriber: Option<SubscriberId>,
    uefi_vars: Option<(SubscriberId, Arc<Mutex<super::uefi_vars::UefiVarsDevice>>)>,
    tpm: Option<(SubscriberId, Arc<Mutex<super::tpm::TpmDevice>>)>,
    usb: Option<(SubscriberId, Arc<Mutex<UsbDevice>>)>,
    intx: Option<(SubscriberId, Arc<Mutex<PciIntxGroup>>)>,
}

impl<'a> PlatformRegistration<'a> {
    pub(crate) fn new(
        events: &'a mut EventManager,
        server: Option<&Arc<Mutex<ConsoleServer>>>,
    ) -> io::Result<Self> {
        let registration = Self {
            subscriber: server.map(|server| events.add_subscriber(server.clone())),
            events,
            intx: None,
            tpm: None,
            uefi_vars: None,
            usb: None,
        };
        if let Some(server) = server {
            server.lock().unwrap().check_registered()?;
        }
        Ok(registration)
    }

    pub(super) fn with_intx(mut self, group: Option<Arc<Mutex<PciIntxGroup>>>) -> io::Result<Self> {
        if let Some(group) = group {
            let id = self.events.add_subscriber(group.clone());
            self.intx = Some((id, group.clone()));
            group.lock().unwrap().check_registered()?;
        }
        Ok(self)
    }

    pub(super) fn with_usb(mut self, usb: Option<&Arc<Mutex<UsbDevice>>>) -> io::Result<Self> {
        if let Some(usb) = usb {
            let id = self.events.add_subscriber(usb.clone());
            self.usb = Some((id, usb.clone()));
            usb.lock().unwrap().check_registered()?;
        }
        Ok(self)
    }

    pub(super) fn with_tpm(
        mut self,
        tpm: Option<&Arc<Mutex<super::tpm::TpmDevice>>>,
    ) -> io::Result<Self> {
        if let Some(tpm) = tpm {
            let id = self.events.add_subscriber(tpm.clone());
            self.tpm = Some((id, tpm.clone()));
            tpm.lock().unwrap().check_registered()?;
        }
        Ok(self)
    }

    fn with_uefi_vars(
        mut self,
        variables: Option<&Arc<Mutex<super::uefi_vars::UefiVarsDevice>>>,
    ) -> io::Result<Self> {
        if let Some(variables) = variables {
            let id = self.events.add_subscriber(variables.clone());
            self.uefi_vars = Some((id, variables.clone()));
            variables.lock().unwrap().check_registered()?;
        }
        Ok(self)
    }

    pub(crate) fn commit(mut self) {
        self.subscriber = None;
        self.tpm = None;
        self.uefi_vars = None;
        self.usb = None;
        self.intx = None;
    }
}

impl Drop for PlatformRegistration<'_> {
    fn drop(&mut self) {
        if let Some((id, variables)) = self.uefi_vars.take() {
            self.events.remove_subscriber(id).unwrap();
            variables.lock().unwrap().reset_registration();
        }
        if let Some((id, tpm)) = self.tpm.take() {
            self.events.remove_subscriber(id).unwrap();
            tpm.lock().unwrap().reset_registration();
        }
        if let Some((id, usb)) = self.usb.take() {
            self.events.remove_subscriber(id).unwrap();
            usb.lock().unwrap().reset_registration();
        }
        if let Some((id, group)) = self.intx.take() {
            self.events.remove_subscriber(id).unwrap();
            group.lock().unwrap().reset_registration();
        }
        if let Some(subscriber) = self.subscriber.take() {
            self.events.remove_subscriber(subscriber).unwrap();
        }
    }
}

impl DeviceManager {
    pub(crate) fn register_platform_devices<'a>(
        &self,
        events: &'a mut EventManager,
    ) -> io::Result<PlatformRegistration<'a>> {
        let intx = self
            .console
            .as_ref()
            .and_then(|_| match &self.virtio_devices {
                VirtioDevices::Pci(pci) => pci.unregistered_intx(),
                VirtioDevices::Mmio(_) => None,
            });
        PlatformRegistration::new(events, self.console.as_ref().map(|console| &console.server))?
            .with_intx(intx)?
            .with_usb(self.console.as_ref().map(|console| &console.usb))?
            .with_tpm(self.tpm.as_ref())?
            .with_uefi_vars(self.uefi_vars.as_ref())
    }
}
