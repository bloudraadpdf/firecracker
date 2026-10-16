// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Transactional event registration for platform devices.

use super::DeviceManager;
#[cfg(target_arch = "x86_64")]
use super::VirtioDevices;
use super::console_server::ConsoleServer;
#[cfg(target_arch = "x86_64")]
use super::console_usb::UsbDevice;
#[cfg(target_arch = "x86_64")]
use super::legacy::PortIODeviceManager;
use crate::EventManager;
#[cfg(target_arch = "x86_64")]
use crate::devices::legacy::{acpi::AcpiPower, cmos::Rtc};
#[cfg(any(target_arch = "aarch64", test))]
use crate::devices::virtio::device::VirtioDevice;
#[cfg(any(target_arch = "aarch64", test))]
use crate::devices::virtio::input::Input;
#[cfg(target_arch = "x86_64")]
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
    #[cfg(target_arch = "x86_64")]
    usb: Option<(SubscriberId, Arc<Mutex<UsbDevice>>)>,
    #[cfg(any(target_arch = "aarch64", test))]
    input: Option<(SubscriberId, Arc<Mutex<dyn VirtioDevice>>)>,
    #[cfg(target_arch = "x86_64")]
    intx: Option<(SubscriberId, Arc<Mutex<PciIntxGroup>>)>,
    #[cfg(target_arch = "x86_64")]
    acpi_power: Option<(SubscriberId, Arc<Mutex<AcpiPower>>)>,
    #[cfg(target_arch = "x86_64")]
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
            #[cfg(any(target_arch = "aarch64", test))]
            input: None,
            #[cfg(target_arch = "x86_64")]
            intx: None,
            tpm: None,
            uefi_vars: None,
            #[cfg(target_arch = "x86_64")]
            usb: None,
            #[cfg(target_arch = "x86_64")]
            acpi_power: None,
            #[cfg(target_arch = "x86_64")]
            rtc: None,
        };
        if let Some(server) = server {
            server.lock().unwrap().check_registered()?;
        }
        Ok(registration)
    }

    #[cfg(target_arch = "x86_64")]
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

    #[cfg(target_arch = "x86_64")]
    pub(super) fn with_intx(mut self, group: Option<Arc<Mutex<PciIntxGroup>>>) -> io::Result<Self> {
        if let Some(group) = group {
            let id = self.events.add_subscriber(group.clone());
            self.intx = Some((id, group.clone()));
            group.lock().unwrap().check_registered()?;
        }
        Ok(self)
    }

    #[cfg(target_arch = "x86_64")]
    pub(super) fn with_usb(mut self, usb: Option<&Arc<Mutex<UsbDevice>>>) -> io::Result<Self> {
        if let Some(usb) = usb {
            let id = self.events.add_subscriber(usb.clone());
            self.usb = Some((id, usb.clone()));
            usb.lock().unwrap().check_registered()?;
        }
        Ok(self)
    }

    #[cfg(any(target_arch = "aarch64", test))]
    pub(super) fn with_input(
        mut self,
        input: Option<&Arc<Mutex<dyn VirtioDevice>>>,
    ) -> io::Result<Self> {
        if let Some(input) = input {
            let id = self.events.add_subscriber(input.clone());
            self.input = Some((id, input.clone()));
            input
                .lock()
                .unwrap()
                .as_any()
                .downcast_ref::<Input>()
                .unwrap()
                .check_registered()?;
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
        #[cfg(target_arch = "x86_64")]
        {
            self.usb = None;
        }
        #[cfg(any(target_arch = "aarch64", test))]
        {
            self.input = None;
        }
        #[cfg(target_arch = "x86_64")]
        {
            self.intx = None;
        }
        #[cfg(target_arch = "x86_64")]
        {
            self.acpi_power = None;
        }
        #[cfg(target_arch = "x86_64")]
        {
            self.rtc = None;
        }
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
        #[cfg(target_arch = "x86_64")]
        if let Some((id, usb)) = self.usb.take() {
            self.events.remove_subscriber(id).unwrap();
            usb.lock().unwrap().reset_registration();
        }
        #[cfg(target_arch = "x86_64")]
        if let Some((id, group)) = self.intx.take() {
            self.events.remove_subscriber(id).unwrap();
            group.lock().unwrap().reset_registration();
        }
        #[cfg(target_arch = "x86_64")]
        if let Some((id, rtc)) = self.rtc.take() {
            self.events.remove_subscriber(id).unwrap();
            rtc.lock().unwrap().reset_registration();
        }
        #[cfg(target_arch = "x86_64")]
        if let Some((id, power)) = self.acpi_power.take() {
            self.events.remove_subscriber(id).unwrap();
            power.lock().unwrap().reset_registration();
        }
        #[cfg(any(target_arch = "aarch64", test))]
        if let Some((id, input)) = self.input.take() {
            self.events.remove_subscriber(id).unwrap();
            input
                .lock()
                .unwrap()
                .as_mut_any()
                .downcast_mut::<Input>()
                .unwrap()
                .reset_registration();
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
        #[cfg(target_arch = "x86_64")]
        let intx = self
            .console
            .as_ref()
            .and_then(|_| match &self.virtio_devices {
                VirtioDevices::Pci(pci) => pci.unregistered_intx(),
                VirtioDevices::Mmio(_) => None,
            });
        let registration = PlatformRegistration::new(
            events,
            self.console.as_ref().map(|console| &console.server),
        )?;
        #[cfg(target_arch = "x86_64")]
        let registration = registration.with_legacy(self.legacy_devices.as_ref())?;
        #[cfg(target_arch = "x86_64")]
        let registration = registration.with_intx(intx)?;
        #[cfg(target_arch = "x86_64")]
        let registration =
            registration.with_usb(self.console.as_ref().map(|console| &console.usb))?;
        #[cfg(target_arch = "aarch64")]
        let registration = {
            let input = self.console.as_ref().and_then(|_| {
                self.get_virtio_device(
                    crate::devices::virtio::device::VirtioDeviceType::Input,
                    crate::devices::virtio::input::INPUT_DEV_ID,
                )
            });
            registration.with_input(input.as_ref())?
        };
        registration
            .with_tpm(self.tpm.as_ref())?
            .with_uefi_vars(self.uefi_vars.as_ref())
    }
}

#[cfg(test)]
#[cfg(target_arch = "x86_64")]
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
