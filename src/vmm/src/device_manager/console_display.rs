// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! PCI and bus mappings for the optional VGA console.

mod memory;
use memory::FramebufferMemory;

use std::io;
use std::sync::{Arc, Barrier, Mutex, Weak};

use serde::{Deserialize, Serialize};
use vm_allocator::{AddressAllocator, AllocPolicy, RangeInclusive};

use super::pci_mngr::PciDevices;
use crate::devices::display::{FRAMEBUFFER_SIZE, REGISTER_SIZE, Vga};
use crate::pci::configuration::{BarPrefetchable, Bars, PciConfiguration, PciConfigurationState};
use crate::pci::{PciClassCode, PciDevice, PciSBDF};
use crate::vstate::bus::BusDevice;
use crate::vstate::vm::KvmVm;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplayState {
    config: PciConfigurationState,
    bars: Bars,
    mapped: [u64; 3],
    sbdf: PciSBDF,
    rom: Vec<u8>,
    rom_bar: u32,
    rom_probe: bool,
    vga: Vga,
}

#[derive(Debug)]
pub struct DisplayDevice {
    framebuffer: FramebufferMemory,
    vm: Weak<KvmVm>,
    config: PciConfiguration,
    bars: Bars,
    mapped: [u64; 3],
    sbdf: PciSBDF,
    rom: Vec<u8>,
    rom_bar: u32,
    rom_probe: bool,
    pub vga: Arc<Mutex<Vga>>,
}

fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn configuration() -> PciConfiguration {
    let mut config = PciConfiguration::new_type0(
        0x1234,
        0x1111,
        1,
        PciClassCode::DisplayController,
        0,
        0x1234,
        0x1111,
    );
    config.write_reg(1, 3);
    config
}

fn sizes(rom: &[u8]) -> [u64; 3] {
    [
        FRAMEBUFFER_SIZE as u64,
        REGISTER_SIZE,
        rom.len().next_power_of_two().max(2048) as u64,
    ]
}

fn allocate_mappings(allocator: &mut AddressAllocator, sizes: [u64; 3]) -> io::Result<[u64; 3]> {
    let mut next = allocator.clone();
    let mut mapped = [0; 3];
    for (base, size) in mapped.iter_mut().zip(sizes) {
        *base = next
            .allocate(size, size, AllocPolicy::FirstMatch)
            .map_err(io_error)?
            .start();
    }
    validate_mappings(&next, mapped, sizes)?;
    *allocator = next;
    Ok(mapped)
}

fn validate_mappings(
    allocator: &AddressAllocator,
    mapped: [u64; 3],
    sizes: [u64; 3],
) -> io::Result<()> {
    let start = crate::arch::MEM_32BIT_DEVICES_START;
    let end = start + crate::arch::MEM_32BIT_DEVICES_SIZE - 1;
    if allocator.base() != start || allocator.end() != end {
        return Err(io_error("invalid display allocator window"));
    }
    let mut reservations = allocator.clone();
    for (base, size) in mapped.into_iter().zip(sizes) {
        let last = base
            .checked_add(size - 1)
            .ok_or_else(|| io_error("display BAR overflow"))?;
        if base < start || last > end || base % size != 0 {
            return Err(io_error("invalid display BAR mapping"));
        }
        // Freeing a clone verifies that the exact, non-overlapping reservation exists.
        reservations
            .free(&RangeInclusive::new(base, last).map_err(io_error)?)
            .map_err(io_error)?;
    }
    Ok(())
}

fn restore_configuration(
    state: &DisplayState,
    allocator: &AddressAllocator,
    rom: &[u8],
) -> io::Result<PciConfiguration> {
    if state.rom != rom
        || !state.vga.validate()
        || state.sbdf.device() == 0
        || state.sbdf.function() != 0
        || state.sbdf.bus() != 0
        || state.sbdf.segment() != 0
    {
        return Err(io_error("invalid display snapshot identity"));
    }
    let saved = PciConfiguration::type0_from_state(state.config.clone()).map_err(io_error)?;
    let mut config = configuration();
    for register in [1, 3, 15] {
        config.write_reg(register, saved.read_reg(register));
    }
    if (0..1024).any(|register| config.read_reg(register) != saved.read_reg(register)) {
        return Err(io_error("invalid display PCI configuration"));
    }
    let sizes = sizes(rom);
    validate_mappings(allocator, state.mapped, sizes)?;
    for (index, bar) in state.bars.bars.iter().enumerate() {
        let (slot, flags) = match index {
            0 => (0, 8),
            2 => (1, 0),
            _ if bar.encoded_addr == 0 && bar.encoded_size == 0 => continue,
            _ => return Err(io_error("unexpected display BAR")),
        };
        let mask = !(u32::try_from(sizes[slot]).unwrap() - 1);
        if bar.encoded_size != mask || bar.encoded_addr & !mask != flags {
            return Err(io_error("invalid display BAR type or size"));
        }
        let address = u64::from(bar.encoded_addr & mask);
        if config.read_reg(1) & 2 != 0
            && address != state.mapped[slot]
            && address != u64::from(mask)
        {
            return Err(io_error("display BAR differs from its mapping"));
        }
    }
    let mask = !(u32::try_from(sizes[2]).unwrap() - 1);
    if state.rom_bar & !(mask | 1) != 0
        || (state.rom_bar & 1 != 0 && u64::from(state.rom_bar & mask) != state.mapped[2])
    {
        return Err(io_error("display ROM BAR differs from its mapping"));
    }
    Ok(config)
}

impl DisplayDevice {
    pub fn attach(
        vm: &Arc<KvmVm>,
        pci: &mut PciDevices,
        rom_path: &std::path::Path,
        state: Option<&DisplayState>,
    ) -> io::Result<Arc<Mutex<Self>>> {
        let rom = std::fs::read(rom_path)?;
        if rom.len() < 512
            || rom.len() > 0x10000
            || rom[..2] != [0x55, 0xaa]
            || usize::from(rom[2]) * 512 != rom.len()
            || rom.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) != 0
        {
            return Err(io_error("invalid VGA option ROM"));
        }
        let rom_size = rom.len().next_power_of_two().max(2048) as u64;
        let config = match state {
            Some(state) => {
                restore_configuration(state, &vm.resource_allocator().mmio32_memory, &rom)?
            }
            None => configuration(),
        };
        let vga = Arc::new(Mutex::new(
            state.map_or_else(Vga::default, |state| state.vga.clone()),
        ));
        let framebuffer = FramebufferMemory::new(vm, vga.lock().unwrap().framebuffer_mapping())?;
        let device = if let Some(state) = state {
            Self {
                framebuffer,
                vm: Arc::downgrade(vm),
                config,
                bars: state.bars,
                mapped: state.mapped,
                sbdf: state.sbdf,
                rom,
                rom_bar: state.rom_bar,
                rom_probe: state.rom_probe,
                vga,
            }
        } else {
            let sbdf = pci.pci_segment.next_device_sbdf().map_err(io_error)?;
            let mut allocator = vm.resource_allocator();
            let [framebuffer_base, registers, rom_base] =
                allocate_mappings(&mut allocator.mmio32_memory, sizes(&rom))?;
            let mut bars = Bars::default();
            bars.set_bar_32(
                0,
                u32::try_from(framebuffer_base).map_err(io_error)?,
                u32::try_from(FRAMEBUFFER_SIZE).unwrap(),
                BarPrefetchable::Yes,
            );
            bars.set_bar_32(
                2,
                u32::try_from(registers).map_err(io_error)?,
                u32::try_from(REGISTER_SIZE).unwrap(),
                BarPrefetchable::No,
            );
            Self {
                framebuffer,
                vm: Arc::downgrade(vm),
                config,
                bars,
                mapped: [framebuffer_base, registers, rom_base],
                rom,
                rom_bar: u32::try_from(rom_base).map_err(io_error)?,
                rom_probe: false,
                sbdf,
                vga,
            }
        };
        let mapped = device.mapped;
        let sbdf = device.sbdf;
        let regions = [
            (mapped[0], FRAMEBUFFER_SIZE as u64),
            (mapped[1], REGISTER_SIZE),
            (mapped[2], rom_size),
            (0xa0000, 0x20000),
        ];
        let device = Arc::new(Mutex::new(device));
        let mut pci_inserted = false;
        let mut mmio_inserted = Vec::new();
        let mut pio_inserted = Vec::new();
        let result = (|| -> io::Result<()> {
            pci.pci_segment
                .pci_bus
                .lock()
                .unwrap()
                .add_device(sbdf.device(), device.clone())
                .map_err(io_error)?;
            pci_inserted = true;
            for (base, size) in regions {
                vm.common
                    .mmio_bus
                    .insert(device.clone(), base, size)
                    .map_err(io_error)?;
                mmio_inserted.push((base, size));
            }
            {
                let mut device = device.lock().unwrap();
                let enabled = device.config.read_reg(1) & 2 != 0;
                device
                    .framebuffer
                    .set_address(enabled.then_some(mapped[0]))?;
            }
            for (base, size) in [(0x3b0, 0x30), (0x1ce, 4)] {
                vm.pio_bus
                    .insert(device.clone(), base, size)
                    .map_err(io_error)?;
                pio_inserted.push((base, size));
            }
            Ok(())
        })();
        if let Err(error) = result {
            for (base, size) in pio_inserted {
                let _ = vm.pio_bus.remove(base, size);
            }
            for (base, size) in mmio_inserted {
                let _ = vm.common.mmio_bus.remove(base, size);
            }
            if pci_inserted {
                pci.pci_segment
                    .pci_bus
                    .lock()
                    .unwrap()
                    .remove_device(sbdf.device());
            }
            if state.is_none() {
                let mut allocator = vm.resource_allocator();
                for (base, size) in regions.into_iter().take(3) {
                    let _ = allocator
                        .mmio32_memory
                        .free(&RangeInclusive::new(base, base + size - 1).unwrap());
                }
            }
            return Err(error);
        }
        Ok(device)
    }

    pub fn save(&self) -> DisplayState {
        DisplayState {
            config: self.config.state(),
            bars: self.bars,
            mapped: self.mapped,
            sbdf: self.sbdf,
            rom: self.rom.clone(),
            rom_bar: self.rom_bar,
            rom_probe: self.rom_probe,
            vga: self.vga.lock().unwrap().clone(),
        }
    }

    fn restore_bar_address(&mut self, slot: usize) {
        if slot == 2 {
            self.rom_bar = u32::try_from(self.mapped[2]).unwrap() | (self.rom_bar & 1);
        } else {
            let bar = if slot == 0 { 0 } else { 2 };
            let flags = self.bars.bars[bar].encoded_addr & 0xf;
            self.bars.bars[bar].encoded_addr = u32::try_from(self.mapped[slot]).unwrap() | flags;
        }
    }

    fn relocate(&mut self) {
        let Some(vm) = self.vm.upgrade() else {
            return;
        };
        for (slot, new, size) in [
            (0, self.bars.get_bar_addr(0), FRAMEBUFFER_SIZE as u64),
            (1, self.bars.get_bar_addr(2), REGISTER_SIZE),
            (
                2,
                u64::from(self.rom_bar & !0x7ff),
                self.rom.len().next_power_of_two().max(2048) as u64,
            ),
        ] {
            let old = self.mapped[slot];
            if slot < 2 && self.config.read_reg(1) & 2 == 0 {
                continue;
            }
            if old == new || (slot == 2 && (self.rom_probe || self.rom_bar & 1 == 0)) {
                continue;
            }
            if slot < 2 && new == ((!u64::from(u32::try_from(size - 1).unwrap())) & 0xffff_ffff) {
                continue;
            }
            let mut allocator = vm.resource_allocator();
            let Ok(range) =
                allocator
                    .mmio32_memory
                    .allocate(size, size, AllocPolicy::ExactMatch(new))
            else {
                drop(allocator);
                self.restore_bar_address(slot);
                continue;
            };
            if vm.common.mmio_bus.move_range(old, new, size).is_err() {
                let _ = allocator.mmio32_memory.free(&range);
                drop(allocator);
                self.restore_bar_address(slot);
                continue;
            }
            if slot == 0 && self.framebuffer.set_address(Some(new)).is_err() {
                vm.common.mmio_bus.move_range(new, old, size).unwrap();
                let _ = allocator.mmio32_memory.free(&range);
                drop(allocator);
                self.restore_bar_address(slot);
                continue;
            }
            let _ = allocator
                .mmio32_memory
                .free(&RangeInclusive::new(old, old + size - 1).unwrap());
            self.mapped[slot] = new;
        }
    }
}

impl PciDevice for DisplayDevice {
    fn read_config_register(&mut self, index: u16) -> u32 {
        if index == 12 {
            return if self.rom_probe {
                !(u32::try_from(self.rom.len().next_power_of_two().max(2048)).unwrap() - 1) & !0x7ff
            } else {
                self.rom_bar
            };
        }
        if (4..10).contains(&index) {
            let mut bytes = [0; 4];
            self.bars
                .read(u8::try_from(index - 4).unwrap(), 0, &mut bytes);
            u32::from_le_bytes(bytes)
        } else {
            self.config.read_reg(index)
        }
    }

    fn write_config_register(
        &mut self,
        index: u16,
        offset: u8,
        bytes: &[u8],
    ) -> Option<Arc<Barrier>> {
        if usize::from(offset) + bytes.len() > 4 {
            return None;
        }
        let old_command = self.config.read_reg(1);
        if index == 12 {
            let mut value = self.rom_bar.to_le_bytes();
            value[usize::from(offset)..usize::from(offset) + bytes.len()].copy_from_slice(bytes);
            let value = u32::from_le_bytes(value);
            self.rom_probe = value & !0x7ff == 0xffff_f800;
            if !self.rom_probe {
                self.rom_bar = value
                    & (!(u32::try_from(self.rom.len().next_power_of_two().max(2048)).unwrap() - 1)
                        | 1);
                if self.rom_bar & 1 != 0 {
                    self.relocate();
                }
            }
        } else if (4..10).contains(&index) {
            self.bars
                .write(u8::try_from(index - 4).unwrap(), offset, bytes);
            if self.config.read_reg(1) & 2 != 0 {
                self.relocate();
            }
        } else {
            self.config.write_config_register(index, offset, bytes);
            if index == 1 && self.config.read_reg(1) & 2 != 0 {
                self.relocate();
            }
        }
        let enabled = self.config.read_reg(1) & 2 != 0;
        if let Err(error) = self
            .framebuffer
            .set_address(enabled.then_some(self.mapped[0]))
        {
            self.config.write_reg(1, old_command);
            crate::logger::error!("Failed to update framebuffer decode: {error}");
        }
        None
    }
}

fn pio_port(base: u64, offset: u64, command: u32) -> Option<u16> {
    if command & 1 == 0 || !matches!(base, 0x3b0 | 0x1ce) {
        return None;
    }
    u16::try_from(base.checked_add(offset)?).ok()
}

impl BusDevice for DisplayDevice {
    fn read(&mut self, base: u64, offset: u64, bytes: &mut [u8]) {
        bytes.fill(0xff);
        let command = self.config.read_reg(1);
        let mut vga = self.vga.lock().unwrap();
        if let Some(port) = pio_port(base, offset, command) {
            vga.read_port(port, bytes);
            return;
        }
        match base {
            0xa0000 if command & 2 != 0 => vga.read_legacy(offset, bytes),
            b if b == self.mapped[2] && command & 2 != 0 && self.rom_bar & 1 != 0 => {
                if let Ok(start) = usize::try_from(offset) {
                    for (index, byte) in bytes.iter_mut().enumerate() {
                        *byte = start
                            .checked_add(index)
                            .and_then(|at| self.rom.get(at))
                            .copied()
                            .unwrap_or(0xff);
                    }
                }
            }
            b if b == self.mapped[0] && command & 2 != 0 => vga.read_framebuffer(offset, bytes),
            b if b == self.mapped[1] && command & 2 != 0 => vga.read_registers(offset, bytes),
            _ => {}
        }
    }

    fn write(&mut self, base: u64, offset: u64, bytes: &[u8]) -> Option<Arc<Barrier>> {
        let command = self.config.read_reg(1);
        let mut vga = self.vga.lock().unwrap();
        if let Some(port) = pio_port(base, offset, command) {
            vga.write_port(port, bytes);
            return None;
        }
        match base {
            0xa0000 if command & 2 != 0 => vga.write_legacy(offset, bytes),
            b if b == self.mapped[0] && command & 2 != 0 => vga.write_framebuffer(offset, bytes),
            b if b == self.mapped[1] && command & 2 != 0 => vga.write_registers(offset, bytes),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::vstate::vm::tests::setup_vm_with_memory;
    use vmm_sys_util::tempfile::TempFile;

    fn rom() -> TempFile {
        let file = TempFile::new().unwrap();
        let mut bytes = [0; 512];
        bytes[..3].copy_from_slice(&[0x55, 0xaa, 1]);
        let checksum = bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        bytes[511] = 0u8.wrapping_sub(checksum);
        file.as_file().write_all(&bytes).unwrap();
        file
    }

    fn framebuffer_available(vm: &Arc<KvmVm>, address: u64) -> bool {
        FramebufferMemory::new(vm, Vga::default().framebuffer_mapping())
            .unwrap()
            .set_address(Some(address))
            .is_ok()
    }

    #[test]
    fn display_pci_probe_relocation_and_rom_decode() {
        let vm = Arc::new(setup_vm_with_memory(0x1000));
        vm.setup_irqchip().unwrap();
        let mut pci = PciDevices::new(&vm).unwrap();
        let rom = rom();
        let device = DisplayDevice::attach(&vm, &mut pci, rom.as_path(), None).unwrap();
        let mut locked = device.lock().unwrap();
        let original = locked.mapped[0];
        assert!(!framebuffer_available(&vm, original));
        let original_reg = locked.read_config_register(4);
        locked.write_config_register(4, 0, &u32::MAX.to_le_bytes());
        assert_eq!(locked.read_config_register(4), 0xff00_0008);
        assert_eq!(locked.mapped[0], original);
        locked.write_config_register(4, 0, &original_reg.to_le_bytes());
        let destination = original + 2 * FRAMEBUFFER_SIZE as u64;
        locked.write_config_register(4, 0, &u32::try_from(destination).unwrap().to_le_bytes());
        assert_eq!(locked.mapped[0], destination);
        // PCI memory decoding removes/reinstates the actual KVM mapping.
        assert!(framebuffer_available(&vm, original));
        assert!(!framebuffer_available(&vm, destination));
        locked.write_config_register(1, 0, &1u16.to_le_bytes());
        assert!(framebuffer_available(&vm, destination));
        locked.write_config_register(1, 0, &3u16.to_le_bytes());
        assert!(!framebuffer_available(&vm, destination));
        locked.write_config_register(4, 0, &0u32.to_le_bytes());
        assert_eq!(locked.mapped[0], destination);
        assert_eq!(
            u64::from(locked.read_config_register(4) & !0xf),
            destination
        );
        locked.write_config_register(12, 0, &0xffff_fffeu32.to_le_bytes());
        assert_eq!(locked.read_config_register(12), 0xffff_f800);
        let rom_base = locked.mapped[2];
        locked.write_config_register(12, 0, &(u32::try_from(rom_base).unwrap() | 1).to_le_bytes());
        drop(locked);
        let mut bytes = [0; 3];
        vm.common.mmio_bus.read(rom_base, &mut bytes).unwrap();
        assert_eq!(bytes, [0x55, 0xaa, 1]);
        vm.common.mmio_bus.write(destination, &[1, 2, 3]).unwrap();
        vm.common.mmio_bus.read(destination, &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3]);
        vm.common.mmio_bus.read(original, &mut bytes).unwrap_err();
    }

    #[test]
    fn restore_keeps_probe_register_and_framebuffer_mapping() {
        let source = Arc::new(setup_vm_with_memory(0x1000));
        source.setup_irqchip().unwrap();
        let mut source_pci = PciDevices::new(&source).unwrap();
        let rom = rom();
        let device = DisplayDevice::attach(&source, &mut source_pci, rom.as_path(), None).unwrap();
        let mut locked = device.lock().unwrap();
        let address = locked.mapped[0];
        locked
            .vga
            .lock()
            .unwrap()
            .write_framebuffer(0, &[17, 29, 53]);
        locked.write_config_register(4, 0, &u32::MAX.to_le_bytes());
        let state = locked.save();
        drop(locked);
        let restored_vm = Arc::new(setup_vm_with_memory(0x1000));
        restored_vm.setup_irqchip().unwrap();
        *restored_vm.resource_allocator() = source.resource_allocator().clone();
        let mut restored_pci = PciDevices::new(&restored_vm).unwrap();
        let restored =
            DisplayDevice::attach(&restored_vm, &mut restored_pci, rom.as_path(), Some(&state))
                .unwrap();
        assert_eq!(
            restored.lock().unwrap().read_config_register(4),
            0xff00_0008
        );
        let mut bytes = [0; 3];
        restored_vm
            .common
            .mmio_bus
            .read(address, &mut bytes)
            .unwrap();
        assert_eq!(bytes, [17, 29, 53]);
        assert!(!framebuffer_available(&restored_vm, address));
    }

    #[test]
    fn attached_display_does_not_keep_the_vm_alive() {
        let vm = Arc::new(setup_vm_with_memory(0x1000));
        vm.setup_irqchip().unwrap();
        let weak = Arc::downgrade(&vm);
        let mut pci = PciDevices::new(&vm).unwrap();
        let rom = rom();
        let device = DisplayDevice::attach(&vm, &mut pci, rom.as_path(), None).unwrap();
        drop(vm);
        assert!(weak.upgrade().is_none());
        assert_eq!(device.lock().unwrap().read_config_register(0), 0x1111_1234);
    }

    #[test]
    fn failed_bus_attachment_releases_only_its_own_resources() {
        struct Occupied;
        impl BusDevice for Occupied {}
        let vm = Arc::new(setup_vm_with_memory(0x1000));
        vm.setup_irqchip().unwrap();
        let occupied = Arc::new(Mutex::new(Occupied));
        vm.pio_bus.insert(occupied.clone(), 0x3b0, 0x30).unwrap();
        let mut pci = PciDevices::new(&vm).unwrap();
        let original = vm.resource_allocator().mmio32_memory.clone();
        let rom = rom();
        DisplayDevice::attach(&vm, &mut pci, rom.as_path(), None).unwrap_err();
        assert_eq!(vm.resource_allocator().mmio32_memory, original);
        // Attachment failed after creating the KVM slot: it must be gone too.
        let mut allocator = original.clone();
        let mapping = allocate_mappings(
            &mut allocator,
            sizes(&std::fs::read(rom.as_path()).unwrap()),
        )
        .unwrap();
        assert!(framebuffer_available(&vm, mapping[0]));
        assert!(
            pci.pci_segment
                .pci_bus
                .lock()
                .unwrap()
                .get_device(1)
                .is_none()
        );
        vm.common.mmio_bus.read(0xa0000, &mut [0]).unwrap_err();
        vm.pio_bus.read(0x3b0, &mut [0]).unwrap();
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::vstate::resources::ResourceAllocator;

    fn fixture() -> (DisplayState, AddressAllocator) {
        let mut allocator = ResourceAllocator::new().mmio32_memory;
        let rom = vec![0; 512];
        let mapped = allocate_mappings(&mut allocator, sizes(&rom)).unwrap();
        let mut bars = Bars::default();
        bars.set_bar_32(
            0,
            u32::try_from(mapped[0]).unwrap(),
            u32::try_from(FRAMEBUFFER_SIZE).unwrap(),
            BarPrefetchable::Yes,
        );
        bars.set_bar_32(
            2,
            u32::try_from(mapped[1]).unwrap(),
            u32::try_from(REGISTER_SIZE).unwrap(),
            BarPrefetchable::No,
        );
        (
            DisplayState {
                config: configuration().state(),
                bars,
                mapped,
                sbdf: PciSBDF::new(0, 0, 1, 0),
                rom_bar: u32::try_from(mapped[2]).unwrap(),
                rom_probe: false,
                rom,
                vga: Vga::default(),
            },
            allocator,
        )
    }

    #[test]
    fn restored_bars_require_matching_shape_and_live_addresses() {
        let (state, allocator) = fixture();
        restore_configuration(&state, &allocator, &state.rom).unwrap();
        for corrupt in [
            |value: &mut DisplayState| value.bars.bars[0].encoded_size = 0,
            |value: &mut DisplayState| value.bars.bars[0].encoded_addr |= 1,
            |value: &mut DisplayState| {
                value.bars.bars[0].encoded_addr += u32::try_from(FRAMEBUFFER_SIZE).unwrap()
            },
            |value: &mut DisplayState| value.bars.bars[1].encoded_addr = 1,
            |value: &mut DisplayState| {
                value.rom_bar = (u32::try_from(value.mapped[2]).unwrap() + 2048) | 1
            },
        ] {
            let mut corrupted = state.clone();
            corrupt(&mut corrupted);
            restore_configuration(&corrupted, &allocator, &state.rom).unwrap_err();
        }
    }

    #[test]
    fn mappings_must_be_exact_reservations_inside_the_mmio_window() {
        let (state, allocator) = fixture();
        for mapped in [
            [
                crate::arch::MEM_32BIT_DEVICES_START + crate::arch::MEM_32BIT_DEVICES_SIZE,
                state.mapped[1],
                state.mapped[2],
            ],
            [u64::MAX, state.mapped[1], state.mapped[2]],
            [state.mapped[0], state.mapped[0], state.mapped[2]],
            [state.mapped[0] + 1, state.mapped[1], state.mapped[2]],
        ] {
            validate_mappings(&allocator, mapped, sizes(&state.rom)).unwrap_err();
        }
        let mut missing = allocator.clone();
        missing
            .free(
                &RangeInclusive::new(
                    state.mapped[0],
                    state.mapped[0] + FRAMEBUFFER_SIZE as u64 - 1,
                )
                .unwrap(),
            )
            .unwrap();
        restore_configuration(&state, &missing, &state.rom).unwrap_err();
        assert_eq!(allocator, fixture().1);
    }

    #[test]
    fn snapshot_preserves_bar_probes_and_pending_disabled_reprogramming() {
        let (mut state, allocator) = fixture();
        state.bars.bars[0].encoded_addr = state.bars.bars[0].encoded_size | 8;
        state.rom_probe = true;
        restore_configuration(&state, &allocator, &state.rom).unwrap();
        let mut config = configuration();
        config.write_reg(1, 1);
        state.config = config.state();
        state.bars.bars[0].encoded_addr = 8;
        state.rom_bar = 0;
        restore_configuration(&state, &allocator, &state.rom).unwrap();
    }

    #[test]
    fn restore_uses_canonical_pci_write_masks() {
        let (mut state, allocator) = fixture();
        let mut config = serde_json::to_value(&state.config).unwrap();
        config["writable_bits"][0] = u32::MAX.into();
        state.config = serde_json::from_value(config).unwrap();
        let mut restored = restore_configuration(&state, &allocator, &state.rom).unwrap();
        restored.write_reg(0, u32::MAX);
        assert_eq!(restored.read_reg(0), 0x1111_1234);
        let mut config = serde_json::to_value(&state.config).unwrap();
        config["registers"][2] = 0u32.into();
        state.config = serde_json::from_value(config).unwrap();
        restore_configuration(&state, &allocator, &state.rom).unwrap_err();
    }

    #[test]
    fn partial_allocation_failure_leaves_allocator_unchanged() {
        let mut allocator = AddressAllocator::new(0x1000_0000, FRAMEBUFFER_SIZE as u64).unwrap();
        let original = allocator.clone();
        allocate_mappings(&mut allocator, sizes(&[0; 512])).unwrap_err();
        assert_eq!(allocator, original);
    }
}

#[cfg(test)]
mod memory_tests {
    use kvm_bindings::{kvm_regs, kvm_segment};
    use kvm_ioctls::VcpuExit;
    use vm_memory::{Bytes, GuestAddress};

    use super::*;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    #[test]
    fn framebuffer_accepts_mmx_load() {
        let vm = Arc::new(setup_vm_with_memory(1 << 20));
        vm.setup_irqchip().unwrap();
        let mut vga = Vga::default();
        vga.write_framebuffer(0, &[0x5a, 0, 0, 0]);
        let mut memory = FramebufferMemory::new(&vm, vga.framebuffer_mapping()).unwrap();
        memory.set_address(Some(0xc100_0000)).unwrap();
        let mut vcpu = vm.common.fd.create_vcpu(0).unwrap();
        let mut sregs = vcpu.get_sregs().unwrap();
        sregs.cs = kvm_segment {
            base: 0,
            limit: u32::MAX,
            selector: 8,
            type_: 11,
            present: 1,
            s: 1,
            g: 1,
            db: 1,
            ..Default::default()
        };
        sregs.ds = kvm_segment {
            selector: 16,
            type_: 3,
            ..sregs.cs
        };
        sregs.es = sregs.ds;
        sregs.ss = sregs.ds;
        sregs.cr0 = 0x11;
        sregs.cr4 = 0;
        sregs.efer = 0;
        vcpu.set_sregs(&sregs).unwrap();
        // MOV EDI, framebuffer; MOVD MM1,[EDI]; MOVD EAX,MM1; OUT 80h,AL.
        vm.guest_memory()
            .write_slice(
                &[
                    0xbf, 0, 0, 0, 0xc1, 0x0f, 0x6e, 0x0f, 0x0f, 0x7e, 0xc8, 0xe6, 0x80,
                ],
                GuestAddress(0x1000),
            )
            .unwrap();
        vcpu.set_regs(&kvm_regs {
            rip: 0x1000,
            rflags: 2,
            ..Default::default()
        })
        .unwrap();
        let exit = vcpu.run().unwrap();
        match exit {
            VcpuExit::IoOut(0x80, bytes) => assert_eq!(bytes, &[0x5a]),
            exit => panic!("MMX framebuffer access failed: {exit:?}"),
        }
    }
}
