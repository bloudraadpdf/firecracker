// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::sync::{Arc, Mutex};

use super::VirtioPciDevice;
use crate::vstate::vm::KvmVm;

const PIO_REGIONS: [(u64, u64); 2] = [(0x3b0, 0x30), (0x1ce, 4)];

impl VirtioPciDevice {
    pub(crate) fn vga(&self) -> Option<Arc<Mutex<crate::devices::display::Vga>>> {
        self.vga.as_ref().map(|vga| vga.vga.clone())
    }

    pub(crate) fn register_vga(device: &Arc<Mutex<Self>>, vm: &KvmVm) -> io::Result<()> {
        let regions: Vec<_> = {
            let device = device.lock().unwrap();
            let Some(vga) = &device.vga else {
                return Ok(());
            };
            vga.mmio_regions()
                .map(|(base, size)| (false, base, size))
                .chain(PIO_REGIONS.map(|(base, size)| (true, base, size)))
                .collect()
        };
        let mut inserted = Vec::new();
        let result = (|| {
            for &(pio, base, size) in &regions {
                let bus = if pio {
                    &vm.pio_bus
                } else {
                    &vm.common.mmio_bus
                };
                bus.insert(device.clone(), base, size)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                inserted.push((pio, base, size));
            }
            let mut device = device.lock().unwrap();
            let enabled = device.memory_space_enabled();
            device.vga.as_mut().unwrap().set_decode(enabled)?;
            device.vga_registered = true;
            Ok(())
        })();
        if result.is_err() {
            for (pio, base, size) in inserted {
                let bus = if pio {
                    &vm.pio_bus
                } else {
                    &vm.common.mmio_bus
                };
                let _ = bus.remove(base, size);
            }
        }
        result
    }

    pub(crate) fn unregister_vga(device: &Arc<Mutex<Self>>, vm: &KvmVm) -> io::Result<()> {
        let regions: Vec<_> = {
            let device = device.lock().unwrap();
            if !device.vga_registered {
                return Ok(());
            }
            device.vga.as_ref().unwrap().mmio_regions().collect()
        };
        for (base, size) in regions {
            vm.common
                .mmio_bus
                .remove(base, size)
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        for (base, size) in PIO_REGIONS {
            vm.pio_bus
                .remove(base, size)
                .map_err(|error| io::Error::other(error.to_string()))?;
        }
        let mut device = device.lock().unwrap();
        device.vga.as_mut().unwrap().set_decode(false)?;
        device.vga_registered = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_manager::pci_mngr::PciDevices;
    use crate::devices::virtio::device::VirtioDeviceType;
    use crate::devices::virtio::gpu::{GPU_DEV_ID, Gpu};
    use crate::pci::PciDevice;
    use crate::vstate::bus::BusDevice;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    fn vm() -> Arc<KvmVm> {
        let mut vm = setup_vm_with_memory(1 << 20);
        vm.setup_irqchip(true).unwrap();
        Arc::new(vm)
    }

    #[test]
    fn vga_bar_decode_relocation_and_detach() {
        let vm = vm();
        let mut pci = PciDevices::new(&vm).unwrap();
        let gpu = Arc::new(Mutex::new(Gpu::new(1024, 768).unwrap()));
        let mut events = crate::EventManager::new().unwrap();
        pci.attach_pci_virtio_device(&vm, GPU_DEV_ID.to_owned(), gpu, &mut events)
            .unwrap();
        let transport = pci
            .get_virtio_device(VirtioDeviceType::Gpu, GPU_DEV_ID)
            .unwrap()
            .clone();
        let base = transport.lock().unwrap().vga.as_ref().unwrap().mapped[0];
        transport
            .lock()
            .unwrap()
            .write_config_register(1, 0, &3u16.to_le_bytes());
        vm.common.mmio_bus.write(base, &[17, 29, 53]).unwrap();
        let mut bytes = [0; 3];
        vm.common.mmio_bus.read(base, &mut bytes).unwrap();
        assert_eq!(bytes, [17, 29, 53]);
        vm.pio_bus.write(0x1ce, &1u16.to_le_bytes()).unwrap();
        vm.pio_bus.write(0x1d0, &640u16.to_le_bytes()).unwrap();
        let registers = transport.lock().unwrap().vga.as_ref().unwrap().mapped[1];
        let mut width = [0; 2];
        vm.common
            .mmio_bus
            .read(registers + 0x502, &mut width)
            .unwrap();
        assert_eq!(u16::from_le_bytes(width), 640);
        let destination = base + 2 * crate::devices::display::FRAMEBUFFER_SIZE as u64;
        transport.lock().unwrap().write_config_register(
            4,
            0,
            &u32::try_from(destination).unwrap().to_le_bytes(),
        );
        vm.common.mmio_bus.read(base, &mut bytes).unwrap_err();
        vm.common.mmio_bus.read(destination, &mut bytes).unwrap();
        assert_eq!(bytes, [17, 29, 53]);
        let saved = transport.lock().unwrap().state();
        assert_eq!(saved.vga.unwrap().mapped[0], destination);
        drop(transport);
        pci.detach_pci_virtio_device(
            &vm,
            (VirtioDeviceType::Gpu, GPU_DEV_ID.to_owned()),
            &mut events,
        )
        .unwrap();
        vm.common
            .mmio_bus
            .read(destination, &mut bytes)
            .unwrap_err();
        vm.pio_bus.read(0x1ce, &mut bytes).unwrap_err();
    }

    #[test]
    fn failed_vga_registration_releases_only_its_own_resources() {
        struct Occupied;
        impl BusDevice for Occupied {}
        let vm = vm();
        let occupied = Arc::new(Mutex::new(Occupied));
        vm.pio_bus.insert(occupied.clone(), 0x3b0, 0x30).unwrap();
        let before = vm.resource_allocator().clone();
        let mut pci = PciDevices::new(&vm).unwrap();
        let mut events = crate::EventManager::new().unwrap();
        let gpu = Arc::new(Mutex::new(Gpu::new(1024, 768).unwrap()));
        pci.attach_pci_virtio_device(&vm, GPU_DEV_ID.to_owned(), gpu, &mut events)
            .unwrap_err();
        assert!(
            pci.get_virtio_device(VirtioDeviceType::Gpu, GPU_DEV_ID)
                .is_none()
        );
        assert_eq!(vm.resource_allocator().mmio32_memory, before.mmio32_memory);
        assert_eq!(vm.resource_allocator().mmio64_memory, before.mmio64_memory);
        vm.common.mmio_bus.read(0xa0000, &mut [0]).unwrap_err();
        vm.pio_bus.read(0x3b0, &mut [0]).unwrap();
    }
}
