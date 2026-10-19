// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! PCI and bus mappings for the optional display console.

#[cfg(feature = "windows")]
use std::io;
#[cfg(target_arch = "x86_64")]
#[cfg(feature = "windows")]
use std::io::Read;
#[cfg(all(feature = "windows", target_arch = "x86_64"))]
use std::sync::Weak;
#[cfg(feature = "windows")]
use std::sync::{Arc, Barrier, Mutex};

use serde::{Deserialize, Serialize};
#[cfg(feature = "windows")]
use vm_allocator::{AddressAllocator, AllocPolicy, RangeInclusive};

#[cfg(feature = "windows")]
use super::pci_mngr::PciDevices;
use crate::devices::display::VgaState;
#[cfg(all(feature = "windows", test))]
use crate::devices::display::pci::memory::FramebufferMemory;
#[cfg(feature = "windows")]
use crate::devices::display::pci::{VgaPci, allocate_mappings, validate_mappings};
#[cfg(feature = "windows")]
use crate::devices::display::{FRAMEBUFFER_SIZE, REGISTER_SIZE, Vga};
use crate::pci::PciSBDF;
#[cfg(feature = "windows")]
use crate::pci::configuration::{BarPrefetchable, PciConfiguration};
use crate::pci::configuration::{Bars, PciConfigurationState};
#[cfg(feature = "windows")]
use crate::pci::{PciClassCode, PciDevice};
#[cfg(feature = "windows")]
use crate::vstate::bus::BusDevice;
#[cfg(feature = "windows")]
use crate::vstate::vm::KvmVm;

const MAPPINGS: usize = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplayState {
    config: PciConfigurationState,
    bars: Bars,
    mapped: [u64; MAPPINGS],
    sbdf: PciSBDF,
    #[cfg(target_arch = "x86_64")]
    rom: Option<ExpansionRom>,
    vga: VgaState,
}

#[cfg(target_arch = "x86_64")]
#[cfg(feature = "windows")]
impl DisplayState {
    pub(super) fn has_rom(&self) -> bool {
        self.rom.is_some()
    }
}

#[cfg(target_arch = "aarch64")]
#[cfg(feature = "windows")]
impl DisplayState {
    pub(super) fn mmio_regions(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.mapped.into_iter().zip(sizes())
    }
}

#[derive(Debug)]
#[cfg(feature = "windows")]
pub struct DisplayDevice {
    graphics: VgaPci,
    #[cfg(target_arch = "x86_64")]
    vm: Weak<KvmVm>,
    config: PciConfiguration,
    bars: Bars,
    sbdf: PciSBDF,
    #[cfg(target_arch = "x86_64")]
    rom: Option<ExpansionRom>,
}

#[cfg(feature = "windows")]
fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(feature = "windows")]
fn configuration() -> PciConfiguration {
    let mut config = PciConfiguration::new_type0(
        0x1234,
        0x1111,
        1,
        PciClassCode::DisplayController,
        if cfg!(target_arch = "x86_64") {
            0
        } else {
            0x80
        },
        0x1234,
        0x1111,
    );
    config.write_reg(1, if cfg!(target_arch = "x86_64") { 3 } else { 2 });
    config
}

#[cfg(target_arch = "x86_64")]
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ExpansionRom {
    bytes: Vec<u8>,
    mapped: u64,
    bar: u32,
    probe: bool,
}

#[cfg(target_arch = "x86_64")]
#[cfg(feature = "windows")]
fn validate_rom(bytes: &[u8]) -> io::Result<()> {
    if bytes.len() < 512
        || bytes.len() > 0x10000
        || bytes[..2] != [0x55, 0xaa]
        || usize::from(bytes[2]) * 512 != bytes.len()
        || bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) != 0
    {
        return Err(io_error("invalid VGA option ROM"));
    }
    Ok(())
}

#[cfg(target_arch = "x86_64")]
#[cfg(feature = "windows")]
impl ExpansionRom {
    fn region(&self) -> (u64, u64) {
        (
            self.mapped,
            self.bytes.len().next_power_of_two().max(2048) as u64,
        )
    }
}

#[cfg(feature = "windows")]
fn sizes() -> [u64; MAPPINGS] {
    [FRAMEBUFFER_SIZE as u64, REGISTER_SIZE]
}

#[cfg(feature = "windows")]
fn regions(
    mapped: [u64; MAPPINGS],
    #[cfg(target_arch = "x86_64")] rom: Option<(u64, u64)>,
) -> impl Iterator<Item = (u64, u64)> + Clone {
    let regions = mapped.into_iter().zip(sizes());
    #[cfg(target_arch = "x86_64")]
    let regions = regions.chain(rom);
    regions
}

#[cfg(feature = "windows")]
fn restore_configuration(
    state: &DisplayState,
    allocator: &AddressAllocator,
) -> io::Result<PciConfiguration> {
    if !state.vga.validate()
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
    #[cfg(target_arch = "x86_64")]
    if let Some(rom) = &state.rom {
        validate_rom(&rom.bytes)?;
    }
    let sizes = sizes();
    validate_mappings(
        allocator,
        regions(
            state.mapped,
            #[cfg(target_arch = "x86_64")]
            state.rom.as_ref().map(ExpansionRom::region),
        ),
    )?;
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
    #[cfg(target_arch = "x86_64")]
    if let Some(rom) = &state.rom {
        let mask = !(u32::try_from(rom.region().1).unwrap() - 1);
        if rom.bar & !(mask | 1) != 0
            || (rom.bar & 1 != 0 && u64::from(rom.bar & mask) != rom.mapped)
        {
            return Err(io_error("display ROM BAR differs from its mapping"));
        }
    }
    Ok(config)
}

#[cfg(feature = "windows")]
impl DisplayDevice {
    pub fn attach(
        vm: &Arc<KvmVm>,
        pci: &mut PciDevices,
        #[cfg(target_arch = "x86_64")] rom_path: Option<&std::path::Path>,
        state: Option<&DisplayState>,
    ) -> io::Result<Arc<Mutex<Self>>> {
        #[cfg(target_arch = "x86_64")]
        let rom = rom_path
            .map(|path| {
                let mut bytes = Vec::new();
                std::fs::File::open(path)?
                    .take(0x1_0001)
                    .read_to_end(&mut bytes)?;
                validate_rom(&bytes)?;
                Ok::<_, io::Error>(bytes)
            })
            .transpose()?;
        let sizes = sizes();
        let config = match state {
            Some(state) => {
                #[cfg(target_arch = "x86_64")]
                if state.rom.as_ref().map(|rom| &rom.bytes) != rom.as_ref() {
                    return Err(io_error("display ROM differs from its snapshot"));
                }
                restore_configuration(state, &vm.resource_allocator().mmio32_memory)?
            }
            None => configuration(),
        };
        let vga = match state {
            Some(state) => Vga::restore(&state.vga)?,
            None => Vga::default(),
        };
        let mut graphics = VgaPci::new(vm, vga, state.map_or([0; MAPPINGS], |state| state.mapped))?;
        let device = if let Some(state) = state {
            Self {
                graphics,
                #[cfg(target_arch = "x86_64")]
                vm: Arc::downgrade(vm),
                config,
                bars: state.bars,
                sbdf: state.sbdf,
                #[cfg(target_arch = "x86_64")]
                rom: state.rom.clone(),
            }
        } else {
            let sbdf = pci.pci_segment.next_device_sbdf().map_err(io_error)?;
            let mut allocator = vm.resource_allocator();
            let mut next = allocator.mmio32_memory.clone();
            let mapped = allocate_mappings(&mut next, sizes)?;
            #[cfg(target_arch = "x86_64")]
            let rom = rom
                .map(|bytes| {
                    let size = bytes.len().next_power_of_two().max(2048) as u64;
                    let mapped = next
                        .allocate(size, size, AllocPolicy::FirstMatch)
                        .map_err(io_error)?
                        .start();
                    Ok::<_, io::Error>(ExpansionRom {
                        bytes,
                        mapped,
                        bar: u32::try_from(mapped).map_err(io_error)?,
                        probe: false,
                    })
                })
                .transpose()?;
            allocator.mmio32_memory = next;
            graphics.mapped = mapped;
            let [framebuffer_base, registers, ..] = mapped;
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
                graphics,
                #[cfg(target_arch = "x86_64")]
                vm: Arc::downgrade(vm),
                config,
                bars,
                #[cfg(target_arch = "x86_64")]
                rom,
                sbdf,
            }
        };
        let mapped = device.graphics.mapped;
        let sbdf = device.sbdf;
        let regions = regions(
            mapped,
            #[cfg(target_arch = "x86_64")]
            device.rom.as_ref().map(ExpansionRom::region),
        );
        let bus_regions = regions.clone();
        #[cfg(target_arch = "x86_64")]
        let bus_regions = bus_regions.chain(std::iter::once((0xa0000, 0x20000)));
        let device = Arc::new(Mutex::new(device));
        let mut pci_inserted = false;
        let mut mmio_inserted = Vec::new();
        #[cfg(target_arch = "x86_64")]
        let mut pio_inserted = Vec::new();
        let result = (|| -> io::Result<()> {
            pci.pci_segment
                .pci_bus
                .lock()
                .unwrap()
                .add_device(sbdf.device(), device.clone())
                .map_err(io_error)?;
            pci_inserted = true;
            for (base, size) in bus_regions {
                vm.common
                    .mmio_bus
                    .insert(device.clone(), base, size)
                    .map_err(io_error)?;
                mmio_inserted.push((base, size));
            }
            {
                let mut device = device.lock().unwrap();
                let enabled = device.config.read_reg(1) & 2 != 0;
                device.graphics.set_decode(enabled)?;
            }
            #[cfg(target_arch = "x86_64")]
            for (base, size) in [(0x3b0, 0x30), (0x1ce, 4)] {
                vm.pio_bus
                    .insert(device.clone(), base, size)
                    .map_err(io_error)?;
                pio_inserted.push((base, size));
            }
            Ok(())
        })();
        if let Err(error) = result {
            #[cfg(target_arch = "x86_64")]
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
                for (base, size) in regions {
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
            mapped: self.graphics.mapped,
            sbdf: self.sbdf,
            #[cfg(target_arch = "x86_64")]
            rom: self.rom.clone(),
            vga: self.graphics.vga.lock().unwrap().save(),
        }
    }

    pub fn vga(&self) -> Arc<Mutex<Vga>> {
        self.graphics.vga.clone()
    }

    fn relocate(&mut self) {
        self.graphics
            .relocate(&mut self.bars, self.config.read_reg(1));
        #[cfg(target_arch = "x86_64")]
        if let Some(rom) = &mut self.rom
            && !rom.probe
            && rom.bar & 1 != 0
            && let Some(vm) = self.vm.upgrade()
        {
            let new = u64::from(rom.bar & !0x7ff);
            let (old, size) = rom.region();
            if old == new {
                return;
            }
            let mut allocator = vm.resource_allocator();
            let moved = allocator
                .mmio32_memory
                .allocate(size, size, AllocPolicy::ExactMatch(new))
                .is_ok_and(|range| {
                    if vm.common.mmio_bus.move_range(old, new, size).is_err() {
                        let _ = allocator.mmio32_memory.free(&range);
                        false
                    } else {
                        let _ = allocator
                            .mmio32_memory
                            .free(&RangeInclusive::new(old, old + size - 1).unwrap());
                        true
                    }
                });
            if moved {
                rom.mapped = new;
            } else {
                rom.bar = u32::try_from(old).unwrap() | (rom.bar & 1);
            }
        }
    }
}

#[cfg(feature = "windows")]
impl PciDevice for DisplayDevice {
    fn read_config_register(&mut self, index: u16) -> u32 {
        #[cfg(target_arch = "x86_64")]
        if index == 12 {
            return self.rom.as_ref().map_or(0, |rom| {
                if rom.probe {
                    !(u32::try_from(rom.region().1).unwrap() - 1) & !0x7ff
                } else {
                    rom.bar
                }
            });
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
        #[cfg(target_arch = "x86_64")]
        if index == 12 {
            if let Some(rom) = &mut self.rom {
                let mut value = rom.bar.to_le_bytes();
                value[usize::from(offset)..usize::from(offset) + bytes.len()]
                    .copy_from_slice(bytes);
                let value = u32::from_le_bytes(value);
                rom.probe = value & !0x7ff == 0xffff_f800;
                if !rom.probe {
                    rom.bar = value & (!(u32::try_from(rom.region().1).unwrap() - 1) | 1);
                    if rom.bar & 1 != 0 {
                        self.relocate();
                    }
                }
            }
            return None;
        }
        if (4..10).contains(&index) {
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
        if let Err(error) = self.graphics.set_decode(enabled) {
            self.config.write_reg(1, old_command);
            crate::logger::error!("Failed to update framebuffer decode: {error}");
        }
        None
    }
}

#[cfg(feature = "windows")]
impl BusDevice for DisplayDevice {
    fn read(&mut self, base: u64, offset: u64, bytes: &mut [u8]) {
        bytes.fill(0xff);
        let command = self.config.read_reg(1);
        self.graphics.read(base, offset, command, bytes);
        #[cfg(target_arch = "x86_64")]
        if command & 2 != 0
            && let Some(rom) = &self.rom
            && base == rom.mapped
            && rom.bar & 1 != 0
            && let Ok(start) = usize::try_from(offset)
        {
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = start
                    .checked_add(index)
                    .and_then(|at| rom.bytes.get(at))
                    .copied()
                    .unwrap_or(0xff);
            }
        }
    }

    fn write(&mut self, base: u64, offset: u64, bytes: &[u8]) -> Option<Arc<Barrier>> {
        self.graphics
            .write(base, offset, self.config.read_reg(1), bytes);
        None
    }
}

#[cfg(test)]
#[cfg(feature = "windows")]
#[cfg(target_arch = "x86_64")]
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
    fn oversized_rom_read_does_not_wait_for_eof() {
        use std::os::fd::AsRawFd;
        use std::sync::mpsc;
        use std::time::Duration;

        let (reader, mut writer) = std::io::pipe().unwrap();
        let path = std::path::PathBuf::from(format!("/proc/self/fd/{}", reader.as_raw_fd()));
        let mut vm = setup_vm_with_memory(0x1000);
        vm.setup_irqchip(true).unwrap();
        let vm = Arc::new(vm);
        let mut pci = PciDevices::new(&vm).unwrap();
        let (loaded_tx, loaded_rx) = mpsc::channel();
        let load = std::thread::spawn(move || {
            let result = DisplayDevice::attach(&vm, &mut pci, Some(&path), None);
            loaded_tx.send(result.map(|_| ())).unwrap();
        });
        let (written_tx, written_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let write = std::thread::spawn(move || {
            let mut bytes = vec![0; 65537];
            bytes[..3].copy_from_slice(&[0x55, 0xaa, 128]);
            bytes[65535] = 0u8.wrapping_sub(
                bytes[..65536]
                    .iter()
                    .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
            );
            bytes[65536] = 0xa5;
            let result = writer.write_all(&bytes);
            written_tx.send(result).unwrap();
            let _ = release_rx.recv();
        });
        let written = written_rx.recv_timeout(Duration::from_secs(5));
        let loaded = loaded_rx.recv_timeout(Duration::from_secs(5));
        // Release EOF and join before asserting, including the old loader's timeout path.
        release_tx.send(()).unwrap();
        drop(reader);
        write.join().unwrap();
        load.join().unwrap();
        written.unwrap().unwrap();
        let error = loaded
            .expect("ROM loader waited for EOF after the maximum size plus one byte")
            .unwrap_err();
        assert_eq!(error.to_string(), "invalid VGA option ROM");
    }

    #[test]
    fn absent_rom_has_no_resource_and_survives_full_restore() {
        let mut source = setup_vm_with_memory(0x1000);
        source.setup_irqchip(true).unwrap();
        let source = Arc::new(source);
        let mut pci = PciDevices::new(&source).unwrap();
        let device = DisplayDevice::attach(&source, &mut pci, None, None).unwrap();
        let mut locked = device.lock().unwrap();
        let address = locked.graphics.mapped[0];
        let mut expected = crate::vstate::resources::ResourceAllocator::new().mmio32_memory;
        assert_eq!(
            allocate_mappings(&mut expected, sizes()).unwrap(),
            locked.graphics.mapped
        );
        assert_eq!(source.resource_allocator().mmio32_memory, expected);
        let absent_rom_address = address + 2 * FRAMEBUFFER_SIZE as u64;
        for value in [
            u32::MAX,
            0xffff_fffe,
            u32::try_from(absent_rom_address).unwrap() | 1,
            0,
        ] {
            locked.write_config_register(12, 0, &value.to_le_bytes());
            assert_eq!(locked.read_config_register(12), 0);
            assert!(locked.rom.is_none());
        }
        source
            .common
            .mmio_bus
            .read(absent_rom_address, &mut [0])
            .unwrap_err();
        locked
            .graphics
            .vga
            .lock()
            .unwrap()
            .write_framebuffer(0, &[17, 29, 53]);
        // Reset an enabled Bochs mode without introducing a ROM.
        {
            let mut vga = locked.graphics.vga.lock().unwrap();
            for (offset, value) in [(0x502, 640u16), (0x504, 480), (0x506, 32), (0x508, 0x81)] {
                vga.write_registers(offset, &value.to_le_bytes());
            }
            let mut enabled = [0; 2];
            vga.read_registers(0x508, &mut enabled);
            assert_eq!(u16::from_le_bytes(enabled), 0x81);
            vga.write_registers(0x508, &0u16.to_le_bytes());
            vga.read_registers(0x508, &mut enabled);
            assert_eq!(enabled, [0; 2]);
        }
        locked.write_config_register(1, 0, &1u16.to_le_bytes());
        assert!(framebuffer_available(&source, address));
        locked.write_config_register(1, 0, &3u16.to_le_bytes());
        assert!(!framebuffer_available(&source, address));
        let state: DisplayState =
            bitcode::deserialize(&bitcode::serialize(&locked.save()).unwrap()).unwrap();
        drop(locked);
        let mut target = setup_vm_with_memory(0x1000);
        target.setup_irqchip(true).unwrap();
        let target = Arc::new(target);
        *target.resource_allocator() = source.resource_allocator().clone();
        let mut target_pci = PciDevices::new(&target).unwrap();
        let rom = rom();
        DisplayDevice::attach(&target, &mut target_pci, Some(rom.as_path()), Some(&state))
            .unwrap_err();
        let restored = DisplayDevice::attach(&target, &mut target_pci, None, Some(&state)).unwrap();
        assert_eq!(restored.lock().unwrap().read_config_register(12), 0);
        let mut bytes = [0; 3];
        target.common.mmio_bus.read(address, &mut bytes).unwrap();
        assert_eq!(bytes, [17, 29, 53]);
        assert!(!framebuffer_available(&target, address));
    }

    #[test]
    fn display_pci_probe_relocation_and_rom_decode() {
        let mut vm = setup_vm_with_memory(0x1000);
        vm.setup_irqchip(true).unwrap();
        let vm = Arc::new(vm);
        let mut pci = PciDevices::new(&vm).unwrap();
        let rom = rom();
        let device = DisplayDevice::attach(&vm, &mut pci, Some(rom.as_path()), None).unwrap();
        let mut locked = device.lock().unwrap();
        let original = locked.graphics.mapped[0];
        assert!(!framebuffer_available(&vm, original));
        let original_reg = locked.read_config_register(4);
        locked.write_config_register(4, 0, &u32::MAX.to_le_bytes());
        assert_eq!(locked.read_config_register(4), 0xff00_0008);
        assert_eq!(locked.graphics.mapped[0], original);
        locked.write_config_register(4, 0, &original_reg.to_le_bytes());
        let destination = original + 2 * FRAMEBUFFER_SIZE as u64;
        locked.write_config_register(4, 0, &u32::try_from(destination).unwrap().to_le_bytes());
        assert_eq!(locked.graphics.mapped[0], destination);
        // PCI memory decoding removes/reinstates the actual KVM mapping.
        assert!(framebuffer_available(&vm, original));
        assert!(!framebuffer_available(&vm, destination));
        locked.write_config_register(1, 0, &1u16.to_le_bytes());
        assert!(framebuffer_available(&vm, destination));
        locked.write_config_register(1, 0, &3u16.to_le_bytes());
        assert!(!framebuffer_available(&vm, destination));
        locked.write_config_register(4, 0, &0u32.to_le_bytes());
        assert_eq!(locked.graphics.mapped[0], destination);
        assert_eq!(
            u64::from(locked.read_config_register(4) & !0xf),
            destination
        );
        locked.write_config_register(12, 0, &0xffff_fffeu32.to_le_bytes());
        assert_eq!(locked.read_config_register(12), 0xffff_f800);
        let rom_base = locked.rom.as_ref().unwrap().mapped;
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
        let mut source = setup_vm_with_memory(0x1000);
        source.setup_irqchip(true).unwrap();
        let source = Arc::new(source);
        let mut source_pci = PciDevices::new(&source).unwrap();
        let rom = rom();
        let device =
            DisplayDevice::attach(&source, &mut source_pci, Some(rom.as_path()), None).unwrap();
        let mut locked = device.lock().unwrap();
        let address = locked.graphics.mapped[0];
        locked
            .graphics
            .vga
            .lock()
            .unwrap()
            .write_framebuffer(0, &[17, 29, 53]);
        locked.write_config_register(4, 0, &u32::MAX.to_le_bytes());
        let state = locked.save();
        drop(locked);
        let mut restored_vm = setup_vm_with_memory(0x1000);
        restored_vm.setup_irqchip(true).unwrap();
        let restored_vm = Arc::new(restored_vm);
        *restored_vm.resource_allocator() = source.resource_allocator().clone();
        let mut restored_pci = PciDevices::new(&restored_vm).unwrap();
        DisplayDevice::attach(&restored_vm, &mut restored_pci, None, Some(&state)).unwrap_err();
        let restored = DisplayDevice::attach(
            &restored_vm,
            &mut restored_pci,
            Some(rom.as_path()),
            Some(&state),
        )
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
        let mut vm = setup_vm_with_memory(0x1000);
        vm.setup_irqchip(true).unwrap();
        let vm = Arc::new(vm);
        let weak = Arc::downgrade(&vm);
        let mut pci = PciDevices::new(&vm).unwrap();
        let rom = rom();
        let device = DisplayDevice::attach(&vm, &mut pci, Some(rom.as_path()), None).unwrap();
        drop(vm);
        assert!(weak.upgrade().is_none());
        assert_eq!(device.lock().unwrap().read_config_register(0), 0x1111_1234);
    }

    #[test]
    fn failed_bus_attachment_releases_only_its_own_resources() {
        struct Occupied;
        impl BusDevice for Occupied {}
        let mut vm = setup_vm_with_memory(0x1000);
        vm.setup_irqchip(true).unwrap();
        let vm = Arc::new(vm);
        let occupied = Arc::new(Mutex::new(Occupied));
        vm.pio_bus.insert(occupied.clone(), 0x3b0, 0x30).unwrap();
        let mut pci = PciDevices::new(&vm).unwrap();
        let original = vm.resource_allocator().mmio32_memory.clone();
        let rom = rom();
        DisplayDevice::attach(&vm, &mut pci, Some(rom.as_path()), None).unwrap_err();
        assert_eq!(vm.resource_allocator().mmio32_memory, original);
        // Attachment failed after creating the KVM slot: it must be gone too.
        let mut allocator = original.clone();
        let mapping = allocate_mappings(&mut allocator, sizes()).unwrap();
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
#[cfg(feature = "windows")]
mod snapshot_tests {
    use super::*;
    use crate::vstate::resources::ResourceAllocator;

    fn fixture() -> (DisplayState, AddressAllocator) {
        let mut allocator = ResourceAllocator::new().mmio32_memory;
        #[cfg(target_arch = "x86_64")]
        let rom = {
            let mut bytes = vec![0; 512];
            bytes[..3].copy_from_slice(&[0x55, 0xaa, 1]);
            bytes[511] =
                0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
            let mapped = allocator
                .allocate(2048, 2048, AllocPolicy::FirstMatch)
                .unwrap()
                .start();
            ExpansionRom {
                bytes,
                mapped,
                bar: u32::try_from(mapped).unwrap(),
                probe: false,
            }
        };
        let mapped = allocate_mappings(&mut allocator, sizes()).unwrap();
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
                #[cfg(target_arch = "x86_64")]
                rom: Some(rom),
                vga: Vga::default().save(),
            },
            allocator,
        )
    }

    #[test]
    fn pci_identity_and_unimplemented_resource_probes() {
        let (mut state, allocator) = fixture();
        let mut config = restore_configuration(&state, &allocator).unwrap();
        assert_eq!(config.read_reg(0), 0x1111_1234);
        assert_eq!(
            config.read_reg(2),
            if cfg!(target_arch = "x86_64") {
                0x0300_0001
            } else {
                0x0380_0001
            }
        );
        assert_eq!(
            config.read_reg(1),
            if cfg!(target_arch = "x86_64") { 3 } else { 2 }
        );
        for bar in [1, 3, 4, 5] {
            state.bars.write(bar, 0, &u32::MAX.to_le_bytes());
            let mut bytes = [0xff; 4];
            state.bars.read(bar, 0, &mut bytes);
            assert_eq!(bytes, [0; 4]);
        }
        config.write_config_register(12, 0, &u32::MAX.to_le_bytes());
        assert_eq!(config.read_reg(12), 0);
        let encoded = bitcode::serialize(&state).unwrap();
        let restored: DisplayState = bitcode::deserialize(&encoded).unwrap();
        assert_eq!(restored.mapped, state.mapped);
        restore_configuration(&restored, &allocator).unwrap();
    }

    #[test]
    fn restored_bars_require_matching_shape_and_live_addresses() {
        let (state, allocator) = fixture();
        restore_configuration(&state, &allocator).unwrap();
        for corrupt in [
            |value: &mut DisplayState| value.bars.bars[0].encoded_size = 0,
            |value: &mut DisplayState| value.bars.bars[0].encoded_addr |= 1,
            |value: &mut DisplayState| {
                value.bars.bars[0].encoded_addr += u32::try_from(FRAMEBUFFER_SIZE).unwrap()
            },
            |value: &mut DisplayState| value.bars.bars[1].encoded_addr = 1,
            #[cfg(target_arch = "x86_64")]
            |value: &mut DisplayState| {
                let rom = value.rom.as_mut().unwrap();
                rom.bar = (u32::try_from(rom.mapped).unwrap() + 2048) | 1
            },
        ] {
            let mut corrupted = state.clone();
            corrupt(&mut corrupted);
            restore_configuration(&corrupted, &allocator).unwrap_err();
        }
    }

    #[test]
    fn mappings_must_be_exact_reservations_inside_the_mmio_window() {
        let (state, allocator) = fixture();
        for (slot, base) in [
            (
                0,
                crate::arch::MEM_32BIT_DEVICES_START + crate::arch::MEM_32BIT_DEVICES_SIZE,
            ),
            (0, u64::MAX),
            (1, state.mapped[0]),
            (0, state.mapped[0] + 1),
        ] {
            let mut mapped = state.mapped;
            mapped[slot] = base;
            validate_mappings(
                &allocator,
                regions(
                    mapped,
                    #[cfg(target_arch = "x86_64")]
                    state.rom.as_ref().map(ExpansionRom::region),
                ),
            )
            .unwrap_err();
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
        restore_configuration(&state, &missing).unwrap_err();
        assert_eq!(allocator, fixture().1);
    }

    #[test]
    fn snapshot_preserves_bar_probes_and_pending_disabled_reprogramming() {
        let (mut state, allocator) = fixture();
        state.bars.bars[0].encoded_addr = state.bars.bars[0].encoded_size | 8;
        #[cfg(target_arch = "x86_64")]
        {
            state.rom.as_mut().unwrap().probe = true;
        }
        restore_configuration(&state, &allocator).unwrap();
        let mut config = configuration();
        config.write_reg(1, 1);
        state.config = config.state();
        state.bars.bars[0].encoded_addr = 8;
        #[cfg(target_arch = "x86_64")]
        {
            state.rom.as_mut().unwrap().bar = 0;
        }
        restore_configuration(&state, &allocator).unwrap();
    }

    #[test]
    fn restore_uses_canonical_pci_write_masks() {
        let (mut state, allocator) = fixture();
        let mut config = serde_json::to_value(&state.config).unwrap();
        config["writable_bits"][0] = u32::MAX.into();
        state.config = serde_json::from_value(config).unwrap();
        let mut restored = restore_configuration(&state, &allocator).unwrap();
        restored.write_reg(0, u32::MAX);
        assert_eq!(restored.read_reg(0), 0x1111_1234);
        let mut config = serde_json::to_value(&state.config).unwrap();
        config["registers"][2] = 0u32.into();
        state.config = serde_json::from_value(config).unwrap();
        restore_configuration(&state, &allocator).unwrap_err();
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn restore_rejects_invalid_optional_rom_resources() {
        let (state, allocator) = fixture();
        let mut corrupted = state.clone();
        corrupted.rom.as_mut().unwrap().bytes.clear();
        restore_configuration(&corrupted, &allocator).unwrap_err();
        let mut corrupted = state.clone();
        corrupted.rom.as_mut().unwrap().mapped = state.mapped[0];
        restore_configuration(&corrupted, &allocator).unwrap_err();
        let mut corrupted = state.clone();
        corrupted.rom.as_mut().unwrap().mapped += 0x2000;
        restore_configuration(&corrupted, &allocator).unwrap_err();
        let mut absent = state;
        absent.rom = None;
        let mut config = configuration();
        let mut encoded = serde_json::to_value(config.state()).unwrap();
        encoded["registers"][12] = 0xc200_0001u32.into();
        absent.config = serde_json::from_value(encoded).unwrap();
        restore_configuration(&absent, &allocator).unwrap_err();
        config.write_config_register(12, 0, &u32::MAX.to_le_bytes());
        assert_eq!(config.read_reg(12), 0);
    }

    #[test]
    fn partial_allocation_failure_leaves_allocator_unchanged() {
        let mut allocator = AddressAllocator::new(0x1000_0000, FRAMEBUFFER_SIZE as u64).unwrap();
        let original = allocator.clone();
        allocate_mappings(&mut allocator, sizes()).unwrap_err();
        assert_eq!(allocator, original);
    }
}

#[cfg(test)]
#[cfg(feature = "windows")]
#[cfg(target_arch = "x86_64")]
mod memory_tests {
    use kvm_bindings::{kvm_regs, kvm_segment};
    use kvm_ioctls::VcpuExit;
    use vm_memory::{Bytes, GuestAddress};

    use super::*;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    #[test]
    fn framebuffer_accepts_mmx_load() {
        let mut vm = setup_vm_with_memory(1 << 20);
        vm.setup_irqchip(true).unwrap();
        let vm = Arc::new(vm);
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
