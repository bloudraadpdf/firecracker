// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::sync::{Arc, Mutex, Weak};

use super::controller::Controller;
use super::memory::{Memory, RAM_BYTES, ROM_BYTES, Region, VRAM_BYTES};
use super::state::ControllerState;
use crate::devices::display::Vga;
use crate::devices::display::pci::memory::FramebufferMemory;
use crate::devices::display::pci::{pio_port, validate_mappings};
use crate::vstate::vm::KvmVm;

pub mod device;

pub const BAR_SIZES: [u64; 3] = [RAM_BYTES as u64, VRAM_BYTES as u64, ROM_BYTES as u64];

#[derive(Debug)]
struct Mapping {
    region: Region,
    offset: usize,
    memory: FramebufferMemory,
}

impl Mapping {
    fn address(&self, mapped: [u64; 3], enabled: bool) -> Option<u64> {
        let index = match self.region {
            Region::Ram => 0,
            Region::Vram => 1,
            Region::Rom => 2,
        };
        enabled.then(|| mapped[index] + u64::try_from(self.offset).unwrap())
    }
}

#[derive(Debug)]
pub struct QxlPci {
    vm: Weak<KvmVm>,
    vga: Arc<Mutex<Vga>>,
    controller: Controller,
    mappings: [Mapping; 4],
    mapped: [u64; 3],
    enabled: bool,
}

impl QxlPci {
    pub fn new(vm: &Arc<KvmVm>, vga: Vga, mapped: [u64; 3]) -> io::Result<Self> {
        Self::build(vm, vga, mapped, None)
    }

    /// Restore the core against independently restored VGA backing and reserved BARs.
    pub fn restore(
        vm: &Arc<KvmVm>,
        vga: Vga,
        mapped: [u64; 3],
        state: &ControllerState,
    ) -> io::Result<Self> {
        Self::build(vm, vga, mapped, Some(state))
    }

    fn build(
        vm: &Arc<KvmVm>,
        vga: Vga,
        mapped: [u64; 3],
        state: Option<&ControllerState>,
    ) -> io::Result<Self> {
        validate_mappings(
            &vm.resource_allocator().mmio32_memory,
            mapped.into_iter().zip(BAR_SIZES),
        )?;
        let controller = match state {
            Some(state) => {
                Controller::restore(vga.framebuffer_mapping(), state, mapped[0], mapped[1])
            }
            None => Controller::new(
                Memory::new(vga.framebuffer_mapping())?,
                mapped[0],
                mapped[1],
            ),
        }
        .map_err(io::Error::other)?;
        let [ram0, ram1, vram, rom] =
            controller
                .memory()
                .regions()
                .map(|(region, offset, memory)| {
                    let memory = if region == Region::Rom {
                        FramebufferMemory::new_read_only(vm, memory)
                    } else {
                        FramebufferMemory::new(vm, memory)
                    }?;
                    Ok::<_, io::Error>(Mapping {
                        region,
                        offset,
                        memory,
                    })
                });
        Ok(Self {
            vm: Arc::downgrade(vm),
            vga: Arc::new(Mutex::new(vga)),
            controller,
            mappings: [ram0?, ram1?, vram?, rom?],
            mapped,
            enabled: false,
        })
    }

    pub fn controller(&mut self) -> &mut Controller {
        &mut self.controller
    }

    pub fn vga(&self) -> Arc<Mutex<Vga>> {
        self.vga.clone()
    }

    pub fn mapped(&self) -> [u64; 3] {
        self.mapped
    }

    fn region(&self, base: u64) -> Option<Region> {
        self.mapped
            .iter()
            .position(|address| *address == base)
            .map(|index| [Region::Ram, Region::Vram, Region::Rom][index])
    }

    /// Read a memory BAR or the shared firmware VGA aperture.
    pub fn read(&self, base: u64, offset: u64, command: u32, bytes: &mut [u8]) -> io::Result<()> {
        bytes.fill(0xff);
        if let Some(port) = pio_port(base, offset, command) {
            self.vga.lock().unwrap().read_port(port, bytes);
        } else if command & 2 != 0 {
            if base == 0xa0000 {
                self.vga.lock().unwrap().read_legacy(offset, bytes);
            } else if let Some(region) = self.region(base) {
                self.controller.memory().read(
                    region,
                    usize::try_from(offset).map_err(io::Error::other)?,
                    bytes,
                )?;
            }
        }
        Ok(())
    }

    /// Write a memory BAR or firmware VGA aperture; the protocol ROM is read-only.
    pub fn write(&self, base: u64, offset: u64, command: u32, bytes: &[u8]) -> io::Result<()> {
        if let Some(port) = pio_port(base, offset, command) {
            self.vga.lock().unwrap().write_port(port, bytes);
        } else if command & 2 != 0 {
            if base == 0xa0000 {
                self.vga.lock().unwrap().write_legacy(offset, bytes);
            } else if let Some(region @ (Region::Ram | Region::Vram)) = self.region(base) {
                self.controller.memory().write(
                    region,
                    usize::try_from(offset).map_err(io::Error::other)?,
                    bytes,
                )?;
            }
        }
        Ok(())
    }

    /// Dispatch a byte write to the revision-two synchronous I/O BAR.
    pub fn write_io(&mut self, offset: u64, bytes: &[u8], now: u64) -> io::Result<()> {
        let [value] = bytes else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "QXL I/O width"));
        };
        let port = u8::try_from(offset)
            .ok()
            .filter(|port| *port < 16)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "QXL I/O offset"))?;
        self.controller
            .io_write(port, *value, now)
            .map_err(io::Error::other)
    }

    fn remap(&mut self, mapped: [u64; 3], enabled: bool) -> io::Result<()> {
        for index in 0..self.mappings.len() {
            let address = self.mappings[index].address(mapped, enabled);
            if let Err(error) = self.mappings[index].memory.set_address(address) {
                let mut rollback = None;
                for previous in (0..index).rev() {
                    let address = self.mappings[previous].address(self.mapped, self.enabled);
                    if let Err(error) = self.mappings[previous].memory.set_address(address) {
                        rollback = Some(error);
                    }
                }
                return Err(match rollback {
                    Some(rollback) => io::Error::other(format!(
                        "QXL mapping failed: {error}; rollback failed: {rollback}"
                    )),
                    None => error,
                });
            }
        }
        self.mapped = mapped;
        self.enabled = enabled;
        Ok(())
    }

    pub fn set_decode(&mut self, enabled: bool) -> io::Result<()> {
        self.remap(self.mapped, enabled)
    }

    pub fn relocate(&mut self, mapped: [u64; 3]) -> io::Result<()> {
        let vm = self
            .vm
            .upgrade()
            .ok_or_else(|| io::Error::other("QXL VM is gone"))?;
        validate_mappings(
            &vm.resource_allocator().mmio32_memory,
            mapped.into_iter().zip(BAR_SIZES),
        )?;
        let previous = self.mapped;
        self.controller
            .relocate(mapped[0], mapped[1])
            .map_err(io::Error::other)?;
        if let Err(error) = self.remap(mapped, self.enabled) {
            self.controller
                .relocate(previous[0], previous[1])
                .map_err(io::Error::other)?;
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use vm_memory::MmapRegion;

    use super::*;
    use crate::devices::display::pci::allocate_mappings;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    fn reserve(vm: &Arc<KvmVm>) -> [u64; 3] {
        allocate_mappings(&mut vm.resource_allocator().mmio32_memory, BAR_SIZES).unwrap()
    }

    fn available(vm: &Arc<KvmVm>, address: u64) -> bool {
        FramebufferMemory::new(vm, Arc::new(MmapRegion::new(4096).unwrap()))
            .unwrap()
            .set_address(Some(address))
            .is_ok()
    }

    #[test]
    fn guest_access_obeys_independent_memory_and_io_decode() {
        let vm = Arc::new(setup_vm_with_memory(0x10000));
        let mapped = reserve(&vm);
        let pci = QxlPci::new(&vm, Vga::default(), mapped).unwrap();
        for command in 0..4 {
            let value = [u8::try_from(command + 1).unwrap()];
            for (index, region) in [Region::Ram, Region::Vram].into_iter().enumerate() {
                pci.controller.memory().write(region, 0, &[0]).unwrap();
                pci.write(mapped[index], 0, command, &value).unwrap();
                let mut actual = [0];
                pci.controller
                    .memory()
                    .read(region, 0, &mut actual)
                    .unwrap();
                assert_eq!(actual, if command & 2 != 0 { value } else { [0] });
                pci.read(mapped[index], 0, command, &mut actual).unwrap();
                assert_eq!(actual, if command & 2 != 0 { value } else { [0xff] });
            }
            pci.vga.lock().unwrap().write_port(0x1ce, &[0, 0]);
            pci.write(0x1ce, 0, command, &[4, 0]).unwrap();
            let mut actual = [0; 2];
            pci.vga.lock().unwrap().read_port(0x1ce, &mut actual);
            assert_eq!(actual, if command & 1 != 0 { [4, 0] } else { [0, 0] });
            pci.read(0x1ce, 0, command, &mut actual).unwrap();
            assert_eq!(actual, if command & 1 != 0 { [4, 0] } else { [0xff; 2] });
        }
    }

    #[test]
    fn bar_access_is_bounded_coherent_and_does_not_modify_rom() {
        let vm = Arc::new(setup_vm_with_memory(0x10000));
        let mapped = reserve(&vm);
        let mut pci = QxlPci::new(&vm, Vga::default(), mapped).unwrap();
        let boundary = u64::try_from(super::super::memory::SURFACE_BYTES).unwrap();
        pci.write(mapped[0], boundary - 2, 2, &[1, 2, 3, 4])
            .unwrap();
        let mut actual = [0; 4];
        pci.read(mapped[0], boundary - 2, 2, &mut actual).unwrap();
        assert_eq!(actual, [1, 2, 3, 4]);
        pci.vga
            .lock()
            .unwrap()
            .read_framebuffer(boundary - 2, &mut actual[..2]);
        assert_eq!(&actual[..2], &[1, 2]);
        pci.write(mapped[2], 0, 2, &[0; 4]).unwrap();
        pci.read(mapped[2], 0, 2, &mut actual).unwrap();
        assert_eq!(&actual, b"QXRO");
        for (base, size) in mapped.into_iter().zip(BAR_SIZES) {
            for offset in [size - 1, size, u64::MAX] {
                assert!(pci.read(base, offset, 2, &mut actual).is_err());
                if base != mapped[2] {
                    assert!(pci.write(base, offset, 2, &actual).is_err());
                }
            }
        }
        pci.read(0xdead, 0, 3, &mut actual).unwrap();
        assert_eq!(actual, [0xff; 4]);
        pci.write(0xdead, 0, 3, &[0]).unwrap();
        for (offset, bytes) in [
            (0, &[][..]),
            (0, &[0, 0][..]),
            (16, &[0][..]),
            (u64::MAX, &[0][..]),
        ] {
            assert_eq!(
                pci.write_io(offset, bytes, 0).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        // Slot zero is invalid; the PCI layer must retain controller errors.
        pci.write_io(8, &[0], 0).unwrap_err();
        pci.write_io(5, &[0], 0).unwrap();
    }

    #[test]
    fn partial_decode_failure_rolls_back_without_removing_another_slot() {
        let vm = Arc::new(setup_vm_with_memory(0x10000));
        let mapped = reserve(&vm);
        let mut occupied =
            FramebufferMemory::new(&vm, Arc::new(MmapRegion::new(4096).unwrap())).unwrap();
        occupied.set_address(Some(mapped[1])).unwrap();
        let mut pci = QxlPci::new(&vm, Vga::default(), mapped).unwrap();
        pci.set_decode(true).unwrap_err();
        assert!(available(&vm, mapped[0]));
        assert!(available(&vm, mapped[0] + (16 << 20)));
        assert!(!available(&vm, mapped[1]));
        assert!(available(&vm, mapped[2]));
        drop(occupied);
        pci.set_decode(true).unwrap();
        for address in mapped {
            assert!(!available(&vm, address));
        }
        pci.set_decode(false).unwrap();
        for address in mapped {
            assert!(available(&vm, address));
        }
    }

    #[test]
    fn active_primary_survives_bar_moves_and_shares_the_boot_framebuffer() {
        let vm = Arc::new(setup_vm_with_memory(0x10000));
        let mapped = reserve(&vm);
        let mut pci = QxlPci::new(&vm, Vga::default(), mapped).unwrap();
        pci.set_decode(true).unwrap();
        pci.vga().lock().unwrap().write_framebuffer(0, &[9]);
        let core = pci.controller();
        let memory = core.memory();
        let mut byte = [0];
        memory.read(Region::Ram, 0, &mut byte).unwrap();
        assert_eq!(byte, [9]);
        memory
            .write(
                Region::Ram,
                super::super::memory::MEMORY_SLOT,
                &mapped[0].to_le_bytes(),
            )
            .unwrap();
        memory
            .write(
                Region::Ram,
                super::super::memory::MEMORY_SLOT + 8,
                &(mapped[0] + 4096).to_le_bytes(),
            )
            .unwrap();
        core.io_write(8, 1, 0).unwrap();
        let mut primary = [0; 40];
        for (offset, value) in [(0, 4u32), (4, 2), (8, 16), (12, 32)] {
            primary[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        primary[32..40].copy_from_slice(&0x0101_0000_0000_0040u64.to_le_bytes());
        core.memory()
            .write(Region::Ram, super::super::memory::CREATE_SURFACE, &primary)
            .unwrap();
        core.io_write(12, 0, 0).unwrap();
        core.memory()
            .write(Region::Ram, 64, &7u32.to_le_bytes())
            .unwrap();
        let next = reserve(&vm);
        pci.relocate(next).unwrap();
        assert_eq!(pci.mapped(), next);
        assert_eq!(pci.controller().frame(0).unwrap().unwrap().pixels[0], 7);
        pci.relocate([next[0], next[0], next[2]]).unwrap_err();
        assert_eq!(pci.mapped(), next);
        assert_eq!(pci.controller().frame(0).unwrap().unwrap().pixels[0], 7);
        for address in mapped {
            assert!(available(&vm, address));
        }
        for address in next {
            assert!(!available(&vm, address));
        }
        drop(pci);
        for address in next {
            assert!(available(&vm, address));
        }
    }
}
