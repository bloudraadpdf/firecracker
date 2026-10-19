// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Optional graphical console and installation devices.

#[cfg(feature = "windows")]
use std::io;
#[cfg(feature = "windows")]
use std::sync::{Arc, Mutex};

#[cfg(target_arch = "x86_64")]
#[cfg(feature = "windows")]
use acpi_tables::{Aml, aml};
use serde::{Deserialize, Serialize};

#[cfg(target_arch = "x86_64")]
use super::console_a20::Port92State;
#[cfg(target_arch = "x86_64")]
#[cfg(feature = "windows")]
use super::console_a20::{Port92, SYSTEM_CONTROL_PORT};
#[cfg(target_arch = "x86_64")]
use super::console_bridge::BridgeState;
#[cfg(target_arch = "x86_64")]
#[cfg(feature = "windows")]
use super::console_bridge::ConsoleBridge;
#[cfg(feature = "windows")]
use super::console_display::DisplayDevice;
use super::console_display::DisplayState;
#[cfg(feature = "windows")]
use super::console_input::ConsoleInput;
#[cfg(target_arch = "x86_64")]
#[cfg(feature = "windows")]
use super::console_optical::OpticalDevice;
#[cfg(target_arch = "x86_64")]
use super::console_optical::OpticalState;
#[cfg(feature = "windows")]
use super::console_server::ConsoleServer;
#[cfg(target_arch = "x86_64")]
#[cfg(feature = "windows")]
use super::console_usb::UsbDevice;
#[cfg(target_arch = "x86_64")]
use super::console_usb::UsbState;
#[cfg(feature = "windows")]
use super::{DeviceManager, VirtioDevices};
#[cfg(target_arch = "x86_64")]
use crate::devices::legacy::I8042State;
#[cfg(all(feature = "windows", target_arch = "aarch64"))]
use crate::devices::virtio::device::VirtioDevice;
#[cfg(feature = "windows")]
use crate::devices::virtio::device::VirtioDeviceType;
#[cfg(all(feature = "windows", target_arch = "x86_64"))]
use crate::devices::virtio::gpu::{GPU_DEV_ID, Gpu};
#[cfg(target_arch = "aarch64")]
#[cfg(feature = "windows")]
use crate::devices::virtio::input::{INPUT_DEV_ID, Input};
use crate::vmm_config::console::ConsoleConfig;
#[cfg(all(feature = "windows", target_arch = "x86_64"))]
use crate::vmm_config::console::DisplayModel;
#[cfg(feature = "windows")]
use crate::vstate::vm::KvmVm;

#[derive(Clone, Debug, Serialize, Deserialize)]
enum ConsoleDisplayState {
    Vga(Box<DisplayState>),
    #[cfg(target_arch = "x86_64")]
    VirtioGpu,
}

#[cfg(feature = "windows")]
impl ConsoleDisplayState {
    fn vga(&self) -> Option<&DisplayState> {
        match self {
            Self::Vga(state) => Some(state),
            #[cfg(target_arch = "x86_64")]
            Self::VirtioGpu => None,
        }
    }

    #[cfg(all(feature = "windows", target_arch = "x86_64"))]
    fn has_rom(&self) -> bool {
        self.vga().is_some_and(DisplayState::has_rom)
    }
}

#[cfg(feature = "windows")]
#[derive(Debug)]
enum ConsoleDisplay {
    Vga(Arc<Mutex<DisplayDevice>>),
    #[cfg(target_arch = "x86_64")]
    VirtioGpu(Arc<Mutex<crate::devices::display::Vga>>),
}

#[cfg(feature = "windows")]
impl ConsoleDisplay {
    fn vga(&self) -> Arc<Mutex<crate::devices::display::Vga>> {
        match self {
            Self::Vga(device) => device.lock().unwrap().vga(),
            #[cfg(target_arch = "x86_64")]
            Self::VirtioGpu(vga) => vga.clone(),
        }
    }

    fn save(&self) -> ConsoleDisplayState {
        match self {
            Self::Vga(device) => ConsoleDisplayState::Vga(Box::new(device.lock().unwrap().save())),
            #[cfg(target_arch = "x86_64")]
            Self::VirtioGpu(_) => ConsoleDisplayState::VirtioGpu,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsoleState {
    config: ConsoleConfig,
    #[cfg(target_arch = "x86_64")]
    system_control: Port92State,
    #[cfg(target_arch = "x86_64")]
    bridge: BridgeState,
    display: ConsoleDisplayState,
    #[cfg(target_arch = "x86_64")]
    input: I8042State,
    #[cfg(target_arch = "x86_64")]
    held: Vec<(u8, bool)>,
    #[cfg(target_arch = "x86_64")]
    release_pending: bool,
    #[cfg(target_arch = "x86_64")]
    usb: UsbState,
    #[cfg(target_arch = "x86_64")]
    optical: Option<OpticalState>,
}

#[cfg(feature = "windows")]
impl ConsoleState {
    pub(crate) fn validate_for_firmware(
        &self,
        firmware: Option<&crate::vmm_config::boot_source::FirmwareConfig>,
    ) -> io::Result<()> {
        self.config.validate_for_firmware(firmware)?;
        #[cfg(target_arch = "x86_64")]
        self.validate_display(&self.config)?;
        #[cfg(target_arch = "x86_64")]
        if self.config.vga_rom_path.is_some() != self.display.has_rom() {
            return Err(io::Error::other(
                "restored VGA ROM configuration and state disagree",
            ));
        }
        Ok(())
    }
}

#[cfg(target_arch = "x86_64")]
#[cfg(feature = "windows")]
impl ConsoleState {
    fn validate_display(&self, config: &ConsoleConfig) -> io::Result<()> {
        if (config.display == DisplayModel::VirtioGpu)
            != matches!(self.display, ConsoleDisplayState::VirtioGpu)
        {
            return Err(io::Error::other(
                "restored console display model and state disagree",
            ));
        }
        Ok(())
    }

    pub(super) fn uses_gpu(&self) -> bool {
        matches!(self.display, ConsoleDisplayState::VirtioGpu)
    }

    pub(super) fn has_optical(&self) -> bool {
        self.optical.is_some()
    }
}

#[cfg(target_arch = "aarch64")]
#[cfg(feature = "windows")]
impl ConsoleState {
    pub(super) fn validate(&self) -> io::Result<()> {
        self.config.validate()
    }

    pub(super) fn mmio_regions(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        let ConsoleDisplayState::Vga(display) = &self.display;
        display.mmio_regions()
    }
}

#[cfg(any(target_arch = "aarch64", test))]
#[cfg(feature = "windows")]
pub(super) fn validate_arm_input(
    console: bool,
    pci: bool,
    modern: bool,
    input: Option<&str>,
) -> io::Result<()> {
    let consistent = if console {
        pci && modern && input == Some(crate::devices::virtio::input::INPUT_DEV_ID)
    } else {
        input.is_none()
    };
    if !consistent {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ARM console/input snapshot topology disagrees",
        ));
    }
    Ok(())
}

#[derive(Debug)]
#[cfg(feature = "windows")]
pub struct ConsoleHardware {
    pub config: ConsoleConfig,
    #[cfg(target_arch = "x86_64")]
    system_control: Arc<Mutex<Port92>>,
    #[cfg(target_arch = "x86_64")]
    bridge: Arc<Mutex<ConsoleBridge>>,
    display: ConsoleDisplay,
    pub(super) server: Arc<Mutex<ConsoleServer>>,
    #[cfg(target_arch = "x86_64")]
    pub(super) usb: Arc<Mutex<UsbDevice>>,
    #[cfg(target_arch = "x86_64")]
    optical: Option<Arc<Mutex<OpticalDevice>>>,
}

#[cfg(all(test, feature = "windows"))]
impl ConsoleHardware {
    pub(crate) fn fill_test_framebuffer(&self) {
        let bytes: Vec<u8> = (0..=255)
            .cycle()
            .take(crate::devices::display::FRAMEBUFFER_SIZE)
            .collect();
        self.display
            .vga()
            .lock()
            .unwrap()
            .write_framebuffer(0, &bytes);
    }
}

#[cfg(feature = "windows")]
impl DeviceManager {
    #[cfg(target_arch = "x86_64")]
    pub(crate) fn set_console_running(&self, running: bool) {
        if let Some(console) = &self.console {
            console.usb.lock().unwrap().set_running(running);
        }
    }

    pub fn console_config(&self) -> Option<ConsoleConfig> {
        self.console.as_ref().map(|console| console.config.clone())
    }

    #[cfg(target_arch = "x86_64")]
    pub fn attach_console(
        &mut self,
        vm: &Arc<KvmVm>,
        config: &ConsoleConfig,
        state: Option<&ConsoleState>,
        events: &mut crate::EventManager,
    ) -> io::Result<()> {
        config.validate()?;
        if let Some(state) = state {
            state.validate_display(config)?;
        }
        if state.is_some_and(|state| config.vga_rom_path.is_some() != state.display.has_rom()) {
            return Err(io::Error::other(
                "restored VGA ROM configuration and state disagree",
            ));
        }
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
        let (display, renderer) = match config.display {
            DisplayModel::Vga => (
                ConsoleDisplay::Vga(DisplayDevice::attach(
                    vm,
                    pci,
                    config.vga_rom_path.as_deref(),
                    state.and_then(|state| state.display.vga()),
                )?),
                None,
            ),
            DisplayModel::VirtioGpu => {
                if state.is_none() {
                    let gpu = Arc::new(Mutex::new(Gpu::new(1024, 768).map_err(io::Error::other)?));
                    pci.attach_pci_virtio_device(vm, GPU_DEV_ID.to_owned(), gpu.clone(), events)
                        .map_err(io::Error::other)?;
                    gpu.lock().unwrap().check_registered()?;
                }
                let device = pci
                    .get_device(VirtioDeviceType::Gpu, GPU_DEV_ID)
                    .ok_or_else(|| io::Error::other("restored console requires its saved GPU"))?;
                let renderer = device
                    .lock()
                    .unwrap()
                    .as_any()
                    .downcast_ref::<Gpu>()
                    .unwrap()
                    .renderer();
                let transport = pci
                    .get_virtio_device(VirtioDeviceType::Gpu, GPU_DEV_ID)
                    .unwrap();
                let vga = transport
                    .lock()
                    .unwrap()
                    .vga()
                    .ok_or_else(|| io::Error::other("GPU has no boot VGA"))?;
                (ConsoleDisplay::VirtioGpu(vga), Some(renderer))
            }
        };
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
            display.vga(),
            ConsoleInput::Legacy {
                keyboard: legacy.i8042.clone(),
                tablet: Some(usb.clone()),
            },
            optical.clone(),
        )?;
        if let Some(renderer) = renderer {
            server.use_gpu(renderer);
        }
        if let Some(state) = state {
            server.restore_held(&state.held)?;
            server.restore_release_pending(state.release_pending);
        }
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

    #[cfg(target_arch = "aarch64")]
    pub fn attach_console(
        &mut self,
        vm: &Arc<KvmVm>,
        config: &ConsoleConfig,
        state: Option<&ConsoleState>,
        events: &mut crate::EventManager,
    ) -> io::Result<()> {
        config.validate()?;
        let VirtioDevices::Pci(pci) = &mut self.virtio_devices else {
            return Err(io::Error::other("graphical console requires PCI"));
        };
        if !vm.resource_allocator().pci_transport.is_modern() {
            return Err(io::Error::other("ARM console requires modern PCI"));
        }
        let input = if state.is_some() {
            pci.get_device(VirtioDeviceType::Input, INPUT_DEV_ID)
                .ok_or_else(|| io::Error::other("restored ARM console requires its saved input"))?
        } else {
            let input: Arc<Mutex<dyn VirtioDevice>> =
                Arc::new(Mutex::new(Input::new().map_err(io::Error::other)?));
            pci.attach_pci_virtio_device(vm, INPUT_DEV_ID.to_owned(), input.clone(), events)
                .map_err(io::Error::other)?;
            input
        };
        let display = ConsoleDisplay::Vga(DisplayDevice::attach(
            vm,
            pci,
            state.and_then(|state| state.display.vga()),
        )?);
        let server = ConsoleServer::new(
            &config.socket_path,
            display.vga(),
            ConsoleInput::Virtio(input),
        )?;
        self.console = Some(ConsoleHardware {
            config: config.clone(),
            display,
            server: Arc::new(Mutex::new(server)),
        });
        Ok(())
    }

    pub(crate) fn save_console(&self) -> Option<ConsoleState> {
        self.console.as_ref().map(|console| {
            #[cfg(target_arch = "x86_64")]
            let (held, release_pending) = {
                let server = console.server.lock().unwrap();
                (server.held_keys(), server.release_pending())
            };
            ConsoleState {
                config: console.config.clone(),
                #[cfg(target_arch = "x86_64")]
                system_control: console.system_control.lock().unwrap().save(),
                #[cfg(target_arch = "x86_64")]
                bridge: console.bridge.lock().unwrap().save(),
                display: console.display.save(),
                #[cfg(target_arch = "x86_64")]
                held,
                #[cfg(target_arch = "x86_64")]
                release_pending,
                #[cfg(target_arch = "x86_64")]
                usb: console.usb.lock().unwrap().save(),
                #[cfg(target_arch = "x86_64")]
                input: self
                    .legacy_devices
                    .as_ref()
                    .unwrap()
                    .i8042
                    .lock()
                    .unwrap()
                    .save_state(),
                #[cfg(target_arch = "x86_64")]
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
        events: &mut crate::EventManager,
    ) -> io::Result<()> {
        self.attach_console(vm, &state.config, Some(state), events)
    }

    #[cfg(target_arch = "x86_64")]
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
#[cfg(feature = "windows")]
#[cfg(target_arch = "x86_64")]
mod registration_tests {
    use super::*;
    use crate::EventManager;
    use crate::pci::intx::PciIntxGroup;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    #[test]
    fn virtio_console_restores_one_gpu_and_its_boot_display() {
        use crate::device_manager::DeviceRestoreArgs;
        use crate::resources::VmResources;
        use crate::snapshot::Persist;
        use crate::vstate::resources::ResourceAllocator;
        use vmm_sys_util::tempdir::TempDir;
        let directory = TempDir::new_with_prefix("./gpu-console-").unwrap();
        let config = ConsoleConfig {
            socket_path: directory.as_path().join("console.sock"),
            display: DisplayModel::VirtioGpu,
            vga_rom_path: None,
            optical_image_path: None,
        };
        let mut source = setup_vm_with_memory(128 << 20);
        source.setup_irqchip(true).unwrap();
        let source = Arc::new(source);
        let mut events = EventManager::new().unwrap();
        let mut manager = DeviceManager::new(
            &mut events,
            source.vcpus_exit_evt(),
            &source,
            None,
            None,
            true,
            true,
        )
        .unwrap();
        manager.attach_vmgenid_device(&source).unwrap();
        manager
            .attach_console(&source, &config, None, &mut events)
            .unwrap();
        let pci = crate::device_manager::tests::pci_devices(&manager);
        let transport = pci
            .get_virtio_device(VirtioDeviceType::Gpu, GPU_DEV_ID)
            .unwrap();
        assert!(Arc::ptr_eq(
            &manager.console.as_ref().unwrap().display.vga(),
            &transport.lock().unwrap().vga().unwrap()
        ));
        let gpu = pci.get_device(VirtioDeviceType::Gpu, GPU_DEV_ID).unwrap();
        gpu.lock()
            .unwrap()
            .as_mut_any()
            .downcast_mut::<Gpu>()
            .unwrap()
            .resize(1280, 720)
            .unwrap();
        let state: crate::device_manager::DevicesState =
            bitcode::deserialize(&bitcode::serialize(&manager.save().unwrap()).unwrap()).unwrap();
        assert!(matches!(
            state.console.as_ref().unwrap().display,
            ConsoleDisplayState::VirtioGpu
        ));
        let allocations = source.resource_allocator().save();
        drop(gpu);
        drop(manager);
        drop(events);
        drop(source);
        let mut target = setup_vm_with_memory(128 << 20);
        target.setup_irqchip(true).unwrap();
        let target = Arc::new(target);
        *target.resource_allocator() = ResourceAllocator::restore((), &allocations).unwrap();
        let mut events = EventManager::new().unwrap();
        let mut resources = VmResources::default();
        let mut restore = |state| {
            DeviceManager::restore(
                DeviceRestoreArgs {
                    mem: target.guest_memory(),
                    vm: &target,
                    event_manager: &mut events,
                    vcpus_exit_evt: target.vcpus_exit_evt(),
                    vm_resources: &mut resources,
                    instance_id: "gpu-console-test",
                    vmgenid_notification: Default::default(),
                    vmclock_notification: Default::default(),
                },
                state,
            )
        };
        let mut missing_gpu = state.clone();
        let crate::device_manager::VirtioDevicesState::Pci(pci) = &mut missing_gpu.virtio_state
        else {
            panic!("expected PCI");
        };
        pci.gpu_device = None;
        assert!(
            restore(&missing_gpu)
                .unwrap_err()
                .to_string()
                .contains("GPU snapshot topology")
        );
        let manager = restore(&state).unwrap();
        let pci = crate::device_manager::tests::pci_devices(&manager);
        let gpu = pci.get_device(VirtioDeviceType::Gpu, GPU_DEV_ID).unwrap();
        assert_eq!(
            gpu.lock()
                .unwrap()
                .as_any()
                .downcast_ref::<Gpu>()
                .unwrap()
                .renderer()
                .lock()
                .unwrap()
                .preferred_dimensions(),
            (1280, 720)
        );
        let transport = pci
            .get_virtio_device(VirtioDeviceType::Gpu, GPU_DEV_ID)
            .unwrap();
        assert!(Arc::ptr_eq(
            &manager.console.as_ref().unwrap().display.vga(),
            &transport.lock().unwrap().vga().unwrap()
        ));
    }

    #[test]
    fn mismatched_optical_snapshot_is_rejected_before_attaching_devices() {
        use std::io::Write;
        use vmm_sys_util::tempdir::TempDir;
        use vmm_sys_util::tempfile::TempFile;

        let directory = TempDir::new_with_prefix("./console-").unwrap();
        let rom = TempFile::new().unwrap();
        rom.as_file().write_all(&[0x55, 0xaa, 1]).unwrap();
        rom.as_file().set_len(512).unwrap();
        let iso = TempFile::new().unwrap();
        iso.as_file().set_len(2048).unwrap();
        let config = ConsoleConfig {
            socket_path: directory.as_path().join("console.sock"),
            #[cfg(target_arch = "x86_64")]
            display: Default::default(),
            vga_rom_path: Some(rom.as_path().to_owned()),
            optical_image_path: Some(iso.as_path().to_owned()),
        };
        let mut vm = setup_vm_with_memory(0x1000);
        vm.setup_irqchip(true).unwrap();
        let vm = Arc::new(vm);
        let mut events = EventManager::new().unwrap();
        let mut source = DeviceManager::new(
            &mut events,
            vm.vcpus_exit_evt(),
            &vm,
            None,
            None,
            true,
            true,
        )
        .unwrap();
        source
            .attach_console(&vm, &config, None, &mut events)
            .unwrap();
        let saved = source.save_console().unwrap();

        saved.validate_for_firmware(None).unwrap();
        let mut missing_rom = saved.clone();
        missing_rom.config.vga_rom_path = None;
        missing_rom.validate_for_firmware(None).unwrap_err();
        missing_rom
            .validate_for_firmware(Some(
                &crate::vmm_config::boot_source::FirmwareConfig::CloudHv {
                    system_uuid: "11223344-5566-7788-99aa-bbccddeeff01"
                        .to_string()
                        .try_into()
                        .unwrap(),
                },
            ))
            .unwrap_err();
        let error = source
            .attach_console(&vm, &missing_rom.config, Some(&missing_rom), &mut events)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "restored VGA ROM configuration and state disagree"
        );

        for missing_state in [false, true] {
            let mut state = saved.clone();
            if missing_state {
                state.optical = None;
            } else {
                state.config.optical_image_path = None;
            }
            let mut target = setup_vm_with_memory(0x1000);
            target.setup_irqchip(true).unwrap();
            let target = Arc::new(target);
            let mut events = EventManager::new().unwrap();
            let mut manager = DeviceManager::new(
                &mut events,
                target.vcpus_exit_evt(),
                &target,
                None,
                None,
                true,
                true,
            )
            .unwrap();
            let before = target.resource_allocator().pci_io.clone();
            let error = manager
                .restore_console(&target, &state, &mut events)
                .unwrap_err();
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
        let mut vm = setup_vm_with_memory(1 << 20);
        vm.setup_irqchip(true).unwrap();
        let vm = Arc::new(vm);
        let group = Arc::new(Mutex::new(PciIntxGroup::new(vm.clone()).unwrap()));
        let mut events = EventManager::new().unwrap();
        group.lock().unwrap().check_registered().unwrap_err();
        {
            let _registration =
                crate::device_manager::platform::PlatformRegistration::new(&mut events, None)
                    .unwrap()
                    .with_intx(Some(group.clone()))
                    .unwrap();
            group.lock().unwrap().check_registered().unwrap();
            assert_eq!(Arc::strong_count(&group), 3);
        }
        assert_eq!(Arc::strong_count(&group), 1);
        group.lock().unwrap().check_registered().unwrap_err();
        crate::device_manager::platform::PlatformRegistration::new(&mut events, None)
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

#[cfg(test)]
#[cfg(feature = "windows")]
mod arm_topology_tests {
    use super::validate_arm_input;

    #[test]
    fn arm_console_requires_its_single_modern_pci_input_and_rejects_strays() {
        for console in [false, true] {
            for pci in [false, true] {
                for modern in [false, true] {
                    for input in [None, Some("input"), Some("wrong-id")] {
                        let result = validate_arm_input(console, pci, modern, input);
                        if if console {
                            pci && modern && input == Some("input")
                        } else {
                            input.is_none()
                        } {
                            result.unwrap();
                        } else {
                            result.unwrap_err();
                        }
                    }
                }
            }
        }
    }
}
