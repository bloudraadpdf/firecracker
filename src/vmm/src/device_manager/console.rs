// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Optional graphical console and installation devices.

use std::io;
use std::sync::{Arc, Mutex};

use acpi_tables::{Aml, aml};
use event_manager::{SubscriberId, SubscriberOps};
use serde::{Deserialize, Serialize};

use super::console_a20::{Port92, Port92State, SYSTEM_CONTROL_PORT};
use super::console_bridge::{BridgeState, ConsoleBridge};
use super::console_display::{DisplayDevice, DisplayState};
use super::console_optical::{OpticalDevice, OpticalState};
use super::console_server::ConsoleServer;
use super::console_usb::{UsbDevice, UsbState};
use super::{DeviceManager, VirtioDevices};
use crate::EventManager;
use crate::devices::legacy::I8042State;
use crate::pci::intx::PciIntxGroup;
use crate::vmm_config::console::ConsoleConfig;
use crate::vstate::vm::KvmVm;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsoleState {
    config: ConsoleConfig,
    system_control: Port92State,
    bridge: BridgeState,
    display: DisplayState,
    input: I8042State,
    held: Vec<(u8, bool)>,
    release_pending: bool,
    usb: UsbState,
    optical: Option<OpticalState>,
}

#[derive(Debug)]
pub struct ConsoleHardware {
    pub config: ConsoleConfig,
    system_control: Arc<Mutex<Port92>>,
    bridge: Arc<Mutex<ConsoleBridge>>,
    display: Arc<Mutex<DisplayDevice>>,
    server: Arc<Mutex<ConsoleServer>>,
    usb: Arc<Mutex<UsbDevice>>,
    optical: Option<Arc<Mutex<OpticalDevice>>>,
}

/// Rolls back the console subscription if subsequent VM startup fails.
pub(crate) struct ConsoleRegistration<'a> {
    events: &'a mut EventManager,
    subscriber: Option<SubscriberId>,
    usb: Option<(SubscriberId, Arc<Mutex<UsbDevice>>)>,
    intx: Option<(SubscriberId, Arc<Mutex<PciIntxGroup>>)>,
}

impl<'a> ConsoleRegistration<'a> {
    pub(crate) fn new(
        events: &'a mut EventManager,
        server: Option<&Arc<Mutex<ConsoleServer>>>,
    ) -> io::Result<Self> {
        let registration = Self {
            subscriber: server.map(|server| events.add_subscriber(server.clone())),
            events,
            intx: None,
            usb: None,
        };
        if let Some(server) = server {
            server.lock().unwrap().check_registered()?;
        }
        Ok(registration)
    }

    fn with_intx(mut self, group: Option<Arc<Mutex<PciIntxGroup>>>) -> io::Result<Self> {
        if let Some(group) = group {
            let id = self.events.add_subscriber(group.clone());
            self.intx = Some((id, group.clone()));
            group.lock().unwrap().check_registered()?;
        }
        Ok(self)
    }

    fn with_usb(mut self, usb: Option<&Arc<Mutex<UsbDevice>>>) -> io::Result<Self> {
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
    }
}

impl Drop for ConsoleRegistration<'_> {
    fn drop(&mut self) {
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
    pub(crate) fn set_console_running(&self, running: bool) {
        if let Some(console) = &self.console {
            console.usb.lock().unwrap().set_running(running);
        }
    }

    pub fn console_config(&self) -> Option<ConsoleConfig> {
        self.console.as_ref().map(|console| console.config.clone())
    }

    pub fn attach_console(
        &mut self,
        vm: &Arc<KvmVm>,
        config: &ConsoleConfig,
        state: Option<&ConsoleState>,
    ) -> io::Result<()> {
        config.validate()?;
        if state.is_some_and(|state| config.optical_image_path.is_some() != state.optical.is_some())
        {
            return Err(io::Error::other(
                "restored optical configuration and state disagree",
            ));
        }
        let VirtioDevices::Pci(pci) = &mut self.virtio_devices else {
            return Err(io::Error::other("graphical console requires PCI"));
        };
        pci.pci_segment.legacy_io_enabled = true;
        pci.pci_segment.legacy_vga_enabled = true;
        let legacy = self
            .legacy_devices
            .as_ref()
            .ok_or_else(|| io::Error::other("console requires x86 legacy devices"))?;
        let system_control = Arc::new(Mutex::new(Port92::new(
            vm.vcpus_exit_evt().try_clone()?,
            state.map(|state| &state.system_control),
        )));
        vm.pio_bus
            .insert(system_control.clone(), u64::from(SYSTEM_CONTROL_PORT), 1)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let bridge = ConsoleBridge::attach(pci, state.map(|state| &state.bridge))?;
        let display = DisplayDevice::attach(
            vm,
            pci,
            &config.vga_rom_path,
            state.map(|state| &state.display),
        )?;
        let optical = match &config.optical_image_path {
            Some(path) => Some(OpticalDevice::attach(
                vm,
                pci,
                path,
                state.and_then(|state| state.optical.as_ref()),
            )?),
            None => None,
        };
        if let Some(state) = state {
            legacy
                .i8042
                .lock()
                .unwrap()
                .restore_state(&state.input)
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        let usb = UsbDevice::attach(vm, pci, state.map(|state| &state.usb))?;
        let mut server = ConsoleServer::new(
            &config.socket_path,
            display.lock().unwrap().vga.clone(),
            legacy.i8042.clone(),
            optical.clone(),
        )?;
        if let Some(state) = state {
            server.restore_held(&state.held)?;
            server.restore_release_pending(state.release_pending);
        }
        server.set_tablet(usb.clone());
        let server = Arc::new(Mutex::new(server));
        self.console = Some(ConsoleHardware {
            config: config.clone(),
            system_control,
            bridge,
            display,
            server,
            usb,
            optical,
        });
        Ok(())
    }

    pub(crate) fn register_console<'a>(
        &self,
        events: &'a mut EventManager,
    ) -> io::Result<ConsoleRegistration<'a>> {
        let intx = self
            .console
            .as_ref()
            .and_then(|_| match &self.virtio_devices {
                VirtioDevices::Pci(pci) => pci.unregistered_intx(),
                VirtioDevices::Mmio(_) => None,
            });
        ConsoleRegistration::new(events, self.console.as_ref().map(|console| &console.server))?
            .with_intx(intx)?
            .with_usb(self.console.as_ref().map(|console| &console.usb))
    }

    pub(crate) fn save_console(&self) -> Option<ConsoleState> {
        self.console.as_ref().map(|console| {
            let (held, release_pending) = {
                let server = console.server.lock().unwrap();
                (server.held_keys(), server.release_pending())
            };
            ConsoleState {
                config: console.config.clone(),
                system_control: console.system_control.lock().unwrap().save(),
                bridge: console.bridge.lock().unwrap().save(),
                display: console.display.lock().unwrap().save(),
                held,
                release_pending,
                usb: console.usb.lock().unwrap().save(),
                input: self
                    .legacy_devices
                    .as_ref()
                    .unwrap()
                    .i8042
                    .lock()
                    .unwrap()
                    .save_state(),
                optical: console
                    .optical
                    .as_ref()
                    .map(|device| device.lock().unwrap().save()),
            }
        })
    }

    pub(crate) fn restore_console(
        &mut self,
        vm: &Arc<KvmVm>,
        state: &ConsoleState,
    ) -> io::Result<()> {
        self.attach_console(vm, &state.config, Some(state))
    }

    pub(crate) fn append_console_aml(&self, bytes: &mut Vec<u8>) -> Result<(), aml::AmlError> {
        if self.console.is_none() {
            return Ok(());
        }
        aml::Device::new(
            self.acpi_device_scope().device_path("A20_")?,
            vec![
                &aml::Name::new("_HID".try_into()?, &aml::EisaName::new("PNP0C02")?)?,
                &aml::Name::new("_UID".try_into()?, &"SYSTEM-CONTROL")?,
                &aml::Name::new(
                    "_CRS".try_into()?,
                    &aml::ResourceTemplate::new(vec![&aml::Io::new(
                        SYSTEM_CONTROL_PORT,
                        SYSTEM_CONTROL_PORT,
                        1,
                        1,
                    )]),
                )?,
            ],
        )
        .append_aml_bytes(bytes)
    }
}

#[cfg(test)]
mod registration_tests {
    use super::*;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    #[test]
    fn mismatched_optical_snapshot_is_rejected_before_attaching_devices() {
        use std::io::Write;
        use vmm_sys_util::tempdir::TempDir;
        use vmm_sys_util::tempfile::TempFile;

        let directory = TempDir::new().unwrap();
        let rom = TempFile::new().unwrap();
        rom.as_file().write_all(&[0x55, 0xaa, 1]).unwrap();
        rom.as_file().set_len(512).unwrap();
        let iso = TempFile::new().unwrap();
        iso.as_file().set_len(2048).unwrap();
        let config = ConsoleConfig {
            socket_path: directory.as_path().join("console.sock"),
            vga_rom_path: rom.as_path().to_owned(),
            optical_image_path: Some(iso.as_path().to_owned()),
        };
        let vm = Arc::new(setup_vm_with_memory(0x1000));
        vm.setup_irqchip().unwrap();
        let mut events = EventManager::new().unwrap();
        let mut source =
            DeviceManager::new(&mut events, vm.vcpus_exit_evt(), &vm, None, None, true).unwrap();
        source.attach_console(&vm, &config, None).unwrap();
        let saved = source.save_console().unwrap();

        for missing_state in [false, true] {
            let mut state = saved.clone();
            if missing_state {
                state.optical = None;
            } else {
                state.config.optical_image_path = None;
            }
            let target = Arc::new(setup_vm_with_memory(0x1000));
            target.setup_irqchip().unwrap();
            let mut events = EventManager::new().unwrap();
            let mut manager = DeviceManager::new(
                &mut events,
                target.vcpus_exit_evt(),
                &target,
                None,
                None,
                true,
            )
            .unwrap();
            let before = target.resource_allocator().pci_io.clone();
            let error = manager.restore_console(&target, &state).unwrap_err();
            assert_eq!(
                error.to_string(),
                "restored optical configuration and state disagree"
            );
            assert_eq!(target.resource_allocator().pci_io, before);
            target
                .pio_bus
                .read(u64::from(SYSTEM_CONTROL_PORT), &mut [0])
                .unwrap_err();
            let VirtioDevices::Pci(pci) = &manager.virtio_devices else {
                panic!()
            };
            assert!(!pci.pci_segment.legacy_io_enabled);
            assert_eq!(pci.pci_segment.next_device_sbdf().unwrap().device(), 1);
        }
    }

    #[test]
    fn optical_intx_registration_rolls_back_without_virtio_devices() {
        let vm = Arc::new(setup_vm_with_memory(1 << 20));
        vm.setup_irqchip().unwrap();
        let group = Arc::new(Mutex::new(PciIntxGroup::new(vm.clone()).unwrap()));
        let mut events = EventManager::new().unwrap();
        group.lock().unwrap().check_registered().unwrap_err();
        {
            let _registration = ConsoleRegistration::new(&mut events, None)
                .unwrap()
                .with_intx(Some(group.clone()))
                .unwrap();
            group.lock().unwrap().check_registered().unwrap();
            assert_eq!(Arc::strong_count(&group), 3);
        }
        assert_eq!(Arc::strong_count(&group), 1);
        group.lock().unwrap().check_registered().unwrap_err();
        ConsoleRegistration::new(&mut events, None)
            .unwrap()
            .with_intx(Some(group.clone()))
            .unwrap()
            .commit();
        group.lock().unwrap().check_registered().unwrap();
        assert_eq!(Arc::strong_count(&group), 2);
        drop(events);
        assert_eq!(Arc::strong_count(&group), 1);
    }
}
