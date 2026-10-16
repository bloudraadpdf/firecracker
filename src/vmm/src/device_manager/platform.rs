// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Transactional event registration for platform devices.

use super::console_server::ConsoleServer;
use super::console_usb::UsbDevice;
use super::legacy::PortIODeviceManager;
use super::{DeviceManager, VirtioDevices};
use crate::EventManager;
use crate::devices::legacy::{acpi::AcpiPower, cmos::Rtc};
use crate::pci::intx::PciIntxGroup;
use event_manager::{SubscriberId, SubscriberOps};
use std::io;
use std::sync::{Arc, Mutex};

/// Rolls back the platform subscriptions if subsequent VM startup fails.
pub(crate) struct PlatformRegistration<'a> {
    events: &'a mut EventManager,
    subscriber: Option<SubscriberId>,
    usb: Option<(SubscriberId, Arc<Mutex<UsbDevice>>)>,
    intx: Option<(SubscriberId, Arc<Mutex<PciIntxGroup>>)>,
    acpi_power: Option<(SubscriberId, Arc<Mutex<AcpiPower>>)>,
    rtc: Option<(SubscriberId, Arc<Mutex<Rtc>>)>,
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
            usb: None,
            acpi_power: None,
            rtc: None,
        };
        if let Some(server) = server {
            server.lock().unwrap().check_registered()?;
        }
        Ok(registration)
    }

    fn with_legacy(mut self, legacy: Option<&PortIODeviceManager>) -> io::Result<Self> {
        if let Some(legacy) = legacy.and_then(|legacy| legacy.pc.as_ref()) {
            let id = self.events.add_subscriber(legacy.acpi_power.clone());
            self.acpi_power = Some((id, legacy.acpi_power.clone()));
            legacy.acpi_power.lock().unwrap().check_registered()?;
            let id = self.events.add_subscriber(legacy.cmos.clone());
            self.rtc = Some((id, legacy.cmos.clone()));
            legacy.cmos.lock().unwrap().check_registered()?;
        }
        Ok(self)
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

    pub(crate) fn commit(mut self) {
        self.subscriber = None;
        self.usb = None;
        self.intx = None;
        self.acpi_power = None;
        self.rtc = None;
    }
}

impl Drop for PlatformRegistration<'_> {
    fn drop(&mut self) {
        if let Some((id, usb)) = self.usb.take() {
            self.events.remove_subscriber(id).unwrap();
            usb.lock().unwrap().reset_registration();
        }
        if let Some((id, group)) = self.intx.take() {
            self.events.remove_subscriber(id).unwrap();
            group.lock().unwrap().reset_registration();
        }
        if let Some((id, rtc)) = self.rtc.take() {
            self.events.remove_subscriber(id).unwrap();
            rtc.lock().unwrap().reset_registration();
        }
        if let Some((id, power)) = self.acpi_power.take() {
            self.events.remove_subscriber(id).unwrap();
            power.lock().unwrap().reset_registration();
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
            .with_legacy(self.legacy_devices.as_ref())?
            .with_intx(intx)?
            .with_usb(self.console.as_ref().map(|console| &console.usb))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    #[test]
    fn construction_does_not_subscribe_legacy_timers() {
        let mut events = EventManager::new().unwrap();
        let mut vm = setup_vm_with_memory(1 << 20);
        vm.setup_irqchip(true).unwrap();
        let vm = Arc::new(vm);
        let manager = DeviceManager::new(
            &mut events,
            vm.vcpus_exit_evt(),
            &vm,
            None,
            None,
            false,
            true,
        )
        .unwrap();
        let legacy = manager.pc_devices().unwrap();
        let power = Arc::downgrade(&legacy.acpi_power);
        let rtc = Arc::downgrade(&legacy.cmos);
        drop(manager);
        drop(vm);
        assert!(power.upgrade().is_none());
        assert!(rtc.upgrade().is_none());
        drop(events);
    }

    #[test]
    fn legacy_timer_subscriptions_follow_startup_result() {
        for commit in [false, true] {
            let mut events = EventManager::new().unwrap();
            let manager = crate::device_manager::tests::default_device_manager_with_pc();
            let legacy = manager.pc_devices().unwrap();
            let power = Arc::downgrade(&legacy.acpi_power);
            let rtc = Arc::downgrade(&legacy.cmos);
            let startup = |events: &mut EventManager| -> io::Result<()> {
                let registration = manager.register_platform_devices(events)?;
                if commit {
                    registration.commit();
                    Ok(())
                } else {
                    Err(io::Error::other("injected vCPU startup failure"))
                }
            };
            if commit {
                startup(&mut events).unwrap();
                legacy
                    .acpi_power
                    .lock()
                    .unwrap()
                    .check_registered()
                    .unwrap();
                legacy.cmos.lock().unwrap().check_registered().unwrap();
            } else {
                startup(&mut events).unwrap_err();
                legacy
                    .acpi_power
                    .lock()
                    .unwrap()
                    .check_registered()
                    .unwrap_err();
                legacy.cmos.lock().unwrap().check_registered().unwrap_err();
            }
            drop(manager);
            assert_eq!(power.upgrade().is_some(), commit);
            assert_eq!(rtc.upgrade().is_some(), commit);
            drop(events);
            assert!(power.upgrade().is_none());
            assert!(rtc.upgrade().is_none());
        }
    }

    #[test]
    fn registration_failure_rolls_back_legacy_timer_subscriptions() {
        use event_manager::Events;
        use vmm_sys_util::epoll::EventSet;

        let manager = crate::device_manager::tests::default_device_manager_with_pc();
        let legacy = manager.pc_devices().unwrap();
        let event = Events::new(
            &legacy.acpi_power.lock().unwrap().resample_evt,
            EventSet::IN,
        );
        crate::device_manager::tests::assert_legacy_registration_failure(
            manager,
            event,
            "Failed to register ACPI event",
        );
    }
}
