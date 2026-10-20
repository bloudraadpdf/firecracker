// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! QXL PCI configuration and transactional resource relocation.

use std::io;
use std::sync::{Arc, Barrier};

use vm_allocator::{AllocPolicy, RangeInclusive};

use super::{BAR_SIZES, QxlPci};
use crate::devices::display::Vga;
use crate::devices::qxl::state::PciState;
use crate::pci::configuration::{BarPrefetchable, Bars, PciConfiguration};
use crate::pci::intx::PCI_INTX_GSI;
use crate::pci::{PciClassCode, PciDevice};
use crate::vstate::vm::KvmVm;

/// Revision-two synchronous ports, excluding later asynchronous extensions.
pub const IO_SIZE: u64 = 16;

/// Revision-two QXL PCI configuration and live BAR ownership.
#[derive(Debug)]
pub struct QxlDevice {
    graphics: QxlPci,
    config: PciConfiguration,
    bars: Bars,
    io_base: u64,
}

fn configuration() -> PciConfiguration {
    let mut config = PciConfiguration::new_type0(
        0x1b36,
        0x0100,
        2,
        PciClassCode::DisplayController,
        0,
        0x1af4,
        0x1100,
    );
    config.set_interrupt_line(u8::try_from(PCI_INTX_GSI).unwrap());
    config
}

impl QxlDevice {
    /// Use memory and I/O ranges already reserved by the console owner.
    pub fn new(mut graphics: QxlPci, io_base: u64) -> io::Result<Self> {
        let vm = graphics
            .vm
            .upgrade()
            .ok_or_else(|| io::Error::other("QXL VM is gone"))?;
        let mut reservations = vm.resource_allocator().pci_io.clone();
        let end = io_base
            .checked_add(IO_SIZE - 1)
            .filter(|end| *end <= reservations.end() && *end <= u64::from(u16::MAX))
            .ok_or_else(|| io::Error::other("invalid QXL I/O range"))?;
        if io_base < reservations.base() || !io_base.is_multiple_of(IO_SIZE) {
            return Err(io::Error::other("invalid QXL I/O base"));
        }
        reservations
            .free(&RangeInclusive::new(io_base, end).map_err(io::Error::other)?)
            .map_err(io::Error::other)?;
        graphics.set_decode(false)?;
        let mut bars = Bars::default();
        for (index, (base, size)) in graphics.mapped().into_iter().zip(BAR_SIZES).enumerate() {
            bars.set_bar_32(
                u8::try_from(index).unwrap(),
                u32::try_from(base).map_err(io::Error::other)?,
                u32::try_from(size).unwrap(),
                BarPrefetchable::No,
            );
        }
        bars.set_bar_io(
            3,
            u32::try_from(io_base).map_err(io::Error::other)?,
            u32::try_from(IO_SIZE).unwrap(),
        );
        Ok(Self {
            graphics,
            config: configuration(),
            bars,
            io_base,
        })
    }

    /// Save PCI registers, private banks and the shared VGA framebuffer once.
    pub fn save(&self) -> io::Result<PciState> {
        Ok(PciState {
            config: self.config.state(),
            bars: self.bars,
            mapped: self.graphics.mapped(),
            io_base: self.io_base,
            vga: self.graphics.vga.lock().unwrap().save(),
            controller: self.graphics.controller.save().map_err(io::Error::other)?,
        })
    }

    /// Validate PCI identity, BAR shape and controller metadata before enabling KVM decode.
    pub fn restore(vm: &Arc<KvmVm>, state: &PciState) -> io::Result<Self> {
        let saved =
            PciConfiguration::type0_from_state(state.config.clone()).map_err(io::Error::other)?;
        let mut config = configuration();
        for register in [1, 3, 15] {
            config.write_reg(register, saved.read_reg(register));
        }
        if (0..1024).any(|register| config.read_reg(register) != saved.read_reg(register)) {
            return Err(io::Error::other("invalid QXL PCI configuration"));
        }
        let graphics = QxlPci::restore(
            vm,
            Vga::restore(&state.vga)?,
            state.mapped,
            &state.controller,
        )?;
        let mut device = Self::new(graphics, state.io_base)?;
        for (index, (actual, expected)) in state.bars.bars.iter().zip(device.bars.bars).enumerate()
        {
            let flags = expected.encoded_addr & !expected.encoded_size;
            if actual.encoded_size != expected.encoded_size
                || actual.encoded_addr & !expected.encoded_size != flags
            {
                return Err(io::Error::other("invalid QXL BAR type or size"));
            }
            if index < 4 {
                let decode = if index == 3 { 1 } else { 2 };
                let probe = actual.encoded_addr == expected.encoded_size | flags;
                if config.read_reg(1) & decode != 0
                    && !probe
                    && actual.encoded_addr != expected.encoded_addr
                {
                    return Err(io::Error::other("QXL BAR differs from its live mapping"));
                }
            }
        }
        device.config = config;
        device.bars = state.bars;
        device.graphics.set_decode(device.command() & 2 != 0)?;
        Ok(device)
    }

    /// Access the controller and shared boot framebuffer.
    pub fn graphics(&mut self) -> &mut QxlPci {
        &mut self.graphics
    }

    /// Current PCI command bits, including independent memory and I/O decode.
    pub fn command(&self) -> u32 {
        self.config.read_reg(1)
    }

    /// Live I/O BAR address; size probes do not change this mapping.
    pub fn io_base(&self) -> u64 {
        self.io_base
    }

    fn relocate(&mut self, index: usize) -> io::Result<()> {
        let io = index == 3;
        let bar = self.bars.bars[index];
        let (old, size, decode) = if io {
            (self.io_base, IO_SIZE, 1)
        } else {
            (self.graphics.mapped()[index], BAR_SIZES[index], 2)
        };
        let next = u64::from(bar.encoded_addr & bar.encoded_size);
        if self.command() & decode == 0 || next == old || next == u64::from(bar.encoded_size) {
            return Ok(());
        }
        if io && next + IO_SIZE - 1 > u64::from(u16::MAX) {
            return Err(io::Error::other("QXL I/O BAR exceeds port space"));
        }
        let vm = self
            .graphics
            .vm
            .upgrade()
            .ok_or_else(|| io::Error::other("QXL VM is gone"))?;
        let bus = if io { &vm.pio_bus } else { &vm.common.mmio_bus };
        let previous = self.graphics.mapped();
        let old_range = RangeInclusive::new(old, old + size - 1).map_err(io::Error::other)?;
        let range = {
            let mut resources = vm.resource_allocator();
            let allocator = if io {
                &mut resources.pci_io
            } else {
                &mut resources.mmio32_memory
            };
            allocator
                .clone()
                .free(&old_range)
                .map_err(io::Error::other)?;
            allocator
                .allocate(size, size, AllocPolicy::ExactMatch(next))
                .map_err(io::Error::other)?
        };
        if let Err(error) = bus.move_range(old, next, size) {
            let mut resources = vm.resource_allocator();
            let allocator = if io {
                &mut resources.pci_io
            } else {
                &mut resources.mmio32_memory
            };
            allocator.free(&range).map_err(io::Error::other)?;
            return Err(io::Error::other(error));
        }
        let result = (|| {
            if !io {
                let mut mapped = previous;
                mapped[index] = next;
                self.graphics.relocate(mapped)?;
            }
            let mut resources = vm.resource_allocator();
            let allocator = if io {
                &mut resources.pci_io
            } else {
                &mut resources.mmio32_memory
            };
            allocator.free(&old_range).map_err(io::Error::other)
        })();
        if let Err(error) = result {
            let rollback = (|| -> io::Result<()> {
                if !io {
                    self.graphics.relocate(previous)?;
                }
                bus.move_range(next, old, size).map_err(io::Error::other)?;
                let mut resources = vm.resource_allocator();
                let allocator = if io {
                    &mut resources.pci_io
                } else {
                    &mut resources.mmio32_memory
                };
                allocator.free(&range).map_err(io::Error::other)
            })();
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback) => io::Error::other(format!(
                    "QXL relocation failed: {error}; rollback failed: {rollback}"
                )),
            });
        }
        if io {
            self.io_base = next;
        }
        Ok(())
    }

    fn update_mappings(&mut self) {
        for index in 0..4 {
            let old = if index == 3 {
                self.io_base
            } else {
                self.graphics.mapped()[index]
            };
            if let Err(error) = self.relocate(index) {
                let flags =
                    self.bars.bars[index].encoded_addr & !self.bars.bars[index].encoded_size;
                self.bars.bars[index].encoded_addr = u32::try_from(old).unwrap() | flags;
                crate::logger::error!("QXL BAR relocation: {error}");
            }
        }
    }
}

impl PciDevice for QxlDevice {
    fn write_config_register(
        &mut self,
        index: u16,
        offset: u8,
        bytes: &[u8],
    ) -> Option<Arc<Barrier>> {
        if bytes.is_empty() || usize::from(offset) + bytes.len() > 4 {
            return None;
        }
        let old_command = self.command();
        if (4..10).contains(&index) {
            self.bars
                .write(u8::try_from(index - 4).unwrap(), offset, bytes);
        } else {
            self.config.write_config_register(index, offset, bytes);
        }
        if index == 1 || (4..8).contains(&index) {
            self.update_mappings();
            if let Err(error) = self.graphics.set_decode(self.command() & 2 != 0) {
                self.config.write_reg(1, old_command);
                crate::logger::error!("QXL memory decode: {error}");
            }
        }
        None
    }

    fn read_config_register(&mut self, index: u16) -> u32 {
        if (4..10).contains(&index) {
            self.bars.bars[usize::from(index - 4)].encoded_addr
        } else {
            self.config.read_reg(index)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::devices::display::Vga;
    use crate::devices::display::pci::allocate_mappings;
    use crate::devices::display::pci::memory::FramebufferMemory;
    use crate::vstate::bus::BusDevice;
    use crate::vstate::vm::{KvmVm, tests::setup_vm_with_memory};
    use vm_allocator::AddressAllocator;
    use vm_memory::MmapRegion;

    struct Marker(u8);

    impl BusDevice for Marker {
        fn read(&mut self, _: u64, _: u64, bytes: &mut [u8]) {
            bytes.fill(self.0);
        }
    }

    struct Fixture {
        vm: Arc<KvmVm>,
        device: QxlDevice,
        _marker: Arc<Mutex<Marker>>,
    }

    impl Fixture {
        fn new() -> Self {
            let vm = Arc::new(setup_vm_with_memory(0x10000));
            let mapped =
                allocate_mappings(&mut vm.resource_allocator().mmio32_memory, BAR_SIZES).unwrap();
            let io = vm
                .resource_allocator()
                .pci_io
                .allocate(IO_SIZE, IO_SIZE, AllocPolicy::FirstMatch)
                .unwrap()
                .start();
            let graphics = QxlPci::new(&vm, Vga::default(), mapped).unwrap();
            let device = QxlDevice::new(graphics, io).unwrap();
            let marker = Arc::new(Mutex::new(Marker(0x5a)));
            for (base, size) in mapped.into_iter().zip(BAR_SIZES) {
                vm.common
                    .mmio_bus
                    .insert(marker.clone(), base, size)
                    .unwrap();
            }
            vm.pio_bus.insert(marker.clone(), io, IO_SIZE).unwrap();
            Self {
                vm,
                device,
                _marker: marker,
            }
        }

        fn allocator(&self, index: usize) -> (AddressAllocator, u64) {
            let resources = self.vm.resource_allocator();
            if index == 3 {
                (resources.pci_io.clone(), IO_SIZE)
            } else {
                (resources.mmio32_memory.clone(), BAR_SIZES[index])
            }
        }

        fn bus(&self, index: usize) -> &crate::vstate::bus::Bus {
            if index == 3 {
                &self.vm.pio_bus
            } else {
                &self.vm.common.mmio_bus
            }
        }

        fn candidate(&self, index: usize) -> u64 {
            let (mut allocator, size) = self.allocator(index);
            allocator
                .allocate(size, size, AllocPolicy::FirstMatch)
                .unwrap()
                .start()
        }

        fn write(&mut self, index: u16, value: u32) {
            assert!(
                self.device
                    .write_config_register(index, 0, &value.to_le_bytes())
                    .is_none()
            );
        }

        fn program_bar(&mut self, index: usize, base: u64) -> u32 {
            let register = u16::try_from(index + 4).unwrap();
            self.write(register, u32::try_from(base).unwrap());
            self.device.read_config_register(register)
        }

        fn memory_available(&self, base: u64) -> bool {
            FramebufferMemory::new(&self.vm, Arc::new(MmapRegion::new(4096).unwrap()))
                .unwrap()
                .set_address(Some(base))
                .is_ok()
        }
    }

    #[test]
    fn identity_and_size_probes_match_revision_two_driver_resources() {
        let mut fixture = Fixture::new();
        for (register, value) in [
            (0, 0x0100_1b36),
            (2, 0x0300_0002),
            (11, 0x1100_1af4),
            (15, 0x110),
        ] {
            assert_eq!(fixture.device.read_config_register(register), value);
            fixture.write(register, u32::MAX);
            assert_eq!(
                fixture.device.read_config_register(register),
                if register == 15 { 0x1ff } else { value }
            );
        }
        let mapped = fixture.device.graphics.mapped();
        let io = fixture.device.io_base;
        for index in 0..6 {
            fixture.write(index + 4, u32::MAX);
            let expected = match index {
                0..=2 => !(u32::try_from(BAR_SIZES[usize::from(index)]).unwrap() - 1),
                3 => 0xffff_fff1,
                _ => 0,
            };
            assert_eq!(fixture.device.read_config_register(index + 4), expected);
        }
        fixture.write(1, 3);
        assert_eq!(fixture.device.graphics.mapped(), mapped);
        assert_eq!(fixture.device.io_base, io);
        assert!(!fixture.memory_available(mapped[0]));
        let before = fixture.device.read_config_register(4);
        fixture.device.write_config_register(4, 3, &[0; 2]);
        assert_eq!(fixture.device.read_config_register(4), before);
    }

    #[test]
    fn enabled_bar_moves_update_bus_kvm_and_reservations() {
        let mut fixture = Fixture::new();
        fixture.write(1, 3);
        fixture
            .device
            .graphics
            .write(fixture.device.graphics.mapped()[0], 64, 2, &[7])
            .unwrap();
        for index in 0..4 {
            let old = if index == 3 {
                fixture.device.io_base
            } else {
                fixture.device.graphics.mapped()[index]
            };
            let next = fixture.candidate(index);
            assert_eq!(
                fixture.program_bar(index, next),
                u32::try_from(next).unwrap() | u32::from(index == 3)
            );
            let mut byte = [0];
            let bus = fixture.bus(index);
            bus.read(next, &mut byte).unwrap();
            assert_eq!(byte, [0x5a]);
            assert!(bus.read(old, &mut byte).is_err());
            let (mut allocator, size) = fixture.allocator(index);
            allocator
                .allocate(size, size, AllocPolicy::ExactMatch(old))
                .unwrap();
            allocator
                .allocate(size, size, AllocPolicy::ExactMatch(next))
                .unwrap_err();
            if index != 3 {
                assert!(fixture.memory_available(old));
                assert!(!fixture.memory_available(next));
            }
        }
        let mut byte = [0];
        fixture
            .device
            .graphics
            .read(fixture.device.graphics.mapped()[0], 64, 2, &mut byte)
            .unwrap();
        assert_eq!(byte, [7]);
    }

    #[test]
    fn disabled_bar_updates_wait_for_their_own_decode_bit() {
        let mut fixture = Fixture::new();
        let old = fixture.device.graphics.mapped()[0];
        let memory = fixture.candidate(0);
        let io = fixture.candidate(3);
        fixture.write(4, u32::try_from(memory).unwrap());
        fixture.write(7, u32::try_from(io).unwrap());
        assert_eq!(fixture.device.graphics.mapped()[0], old);
        fixture.write(1, 1);
        assert_eq!(fixture.device.graphics.mapped()[0], old);
        assert_eq!(fixture.device.io_base, io);
        assert!(fixture.memory_available(old));
        fixture.write(1, 2);
        assert_eq!(fixture.device.graphics.mapped()[0], memory);
        assert!(!fixture.memory_available(memory));
        fixture.write(1, 0);
        assert!(fixture.memory_available(memory));
    }

    #[test]
    fn bus_and_kvm_conflicts_preserve_live_mappings_and_other_devices() {
        let mut fixture = Fixture::new();
        fixture.write(1, 3);
        let old = fixture.device.graphics.mapped()[0];
        let next = fixture.candidate(0);
        let other = Arc::new(Mutex::new(Marker(0x7b)));
        fixture
            .vm
            .common
            .mmio_bus
            .insert(other.clone(), next, BAR_SIZES[0])
            .unwrap();
        fixture.write(4, u32::try_from(next).unwrap());
        assert_eq!(
            fixture.device.read_config_register(4),
            u32::try_from(old).unwrap()
        );
        let mut byte = [0];
        fixture.vm.common.mmio_bus.read(next, &mut byte).unwrap();
        assert_eq!(byte, [0x7b]);
        fixture
            .vm
            .common
            .mmio_bus
            .remove(next, BAR_SIZES[0])
            .unwrap();
        let mut occupied =
            FramebufferMemory::new(&fixture.vm, Arc::new(MmapRegion::new(4096).unwrap())).unwrap();
        occupied.set_address(Some(next)).unwrap();
        fixture.write(4, u32::try_from(next).unwrap());
        assert_eq!(fixture.device.graphics.mapped()[0], old);
        assert_eq!(
            fixture.device.read_config_register(4),
            u32::try_from(old).unwrap()
        );
        assert!(fixture.vm.common.mmio_bus.read(next, &mut byte).is_err());
        fixture.vm.common.mmio_bus.read(old, &mut byte).unwrap();
        assert_eq!(byte, [0x5a]);
        assert!(!fixture.memory_available(next));
        assert!(!fixture.memory_available(old));
        assert_eq!(fixture.candidate(0), next);
    }

    #[test]
    fn decode_failure_restores_command_and_releases_partial_kvm_mappings() {
        let mut fixture = Fixture::new();
        let mapped = fixture.device.graphics.mapped();
        let mut occupied =
            FramebufferMemory::new(&fixture.vm, Arc::new(MmapRegion::new(4096).unwrap())).unwrap();
        occupied.set_address(Some(mapped[1])).unwrap();
        fixture.write(1, 3);
        assert_eq!(fixture.device.command(), 0);
        assert!(fixture.memory_available(mapped[0]));
        assert!(!fixture.memory_available(mapped[1]));
        drop(occupied);
        fixture.write(1, 3);
        assert_eq!(fixture.device.command(), 3);
        assert!(!fixture.memory_available(mapped[0]));
    }

    #[test]
    fn occupied_allocations_reject_memory_and_io_bar_moves() {
        let mut fixture = Fixture::new();
        fixture.write(1, 3);
        for index in [0, 3] {
            let next = fixture.candidate(index);
            let old = fixture
                .device
                .read_config_register(u16::try_from(index + 4).unwrap());
            {
                let mut resources = fixture.vm.resource_allocator();
                let (allocator, size) = if index == 3 {
                    (&mut resources.pci_io, IO_SIZE)
                } else {
                    (&mut resources.mmio32_memory, BAR_SIZES[index])
                };
                allocator
                    .allocate(size, size, AllocPolicy::ExactMatch(next))
                    .unwrap();
            }
            assert_eq!(fixture.program_bar(index, next), old);
            let bus = fixture.bus(index);
            let mut byte = [0];
            bus.read(u64::from(old & !3), &mut byte).unwrap();
            assert_eq!(byte, [0x5a]);
        }
    }

    #[test]
    fn constructor_requires_an_exact_aligned_io_reservation() {
        let fixture = Fixture::new();
        let io = fixture.device.io_base;
        for invalid in [0, io + 1, io + IO_SIZE, 0x1_0000, u64::MAX] {
            let graphics = QxlPci::new(
                &fixture.vm,
                Vga::default(),
                fixture.device.graphics.mapped(),
            )
            .unwrap();
            QxlDevice::new(graphics, invalid).unwrap_err();
        }
        let larger = fixture
            .vm
            .resource_allocator()
            .pci_io
            .allocate(32, 32, AllocPolicy::FirstMatch)
            .unwrap()
            .start();
        let graphics = QxlPci::new(
            &fixture.vm,
            Vga::default(),
            fixture.device.graphics.mapped(),
        )
        .unwrap();
        QxlDevice::new(graphics, larger).unwrap_err();
    }

    #[test]
    fn adapter_checkpoint_restores_pci_probes_shared_vga_and_private_banks() {
        let mut fixture = Fixture::new();
        fixture.write(1, 3);
        let mapped = fixture.device.graphics.mapped();
        fixture
            .device
            .graphics
            .write(mapped[0], 64, 2, &[0x12, 0x34])
            .unwrap();
        fixture
            .device
            .graphics
            .write(mapped[1], 128, 2, &[0xa5])
            .unwrap();
        fixture.write(4, u32::MAX);
        let state = fixture.device.save().unwrap();
        let encoded = bitcode::serialize(&state).unwrap();
        assert!(encoded.len() < 128 * 1024 * 1024);
        let decoded = bitcode::deserialize(&encoded).unwrap();
        let vm = Arc::new(setup_vm_with_memory(0x10000));
        *vm.resource_allocator() = fixture.vm.resource_allocator().clone();
        let mut restored = QxlDevice::restore(&vm, &decoded).unwrap();
        assert_eq!(restored.command(), 3);
        assert_eq!(
            restored.read_config_register(4),
            !(u32::try_from(BAR_SIZES[0]).unwrap() - 1)
        );
        assert_eq!(restored.graphics.mapped(), mapped);
        let mut bytes = [0; 2];
        restored
            .graphics
            .vga
            .lock()
            .unwrap()
            .read_framebuffer(64, &mut bytes);
        assert_eq!(bytes, [0x12, 0x34]);
        restored
            .graphics
            .read(mapped[0], 64, 2, &mut bytes)
            .unwrap();
        assert_eq!(bytes, [0x12, 0x34]);
        restored
            .graphics
            .read(mapped[1], 128, 2, &mut bytes[..1])
            .unwrap();
        assert_eq!(bytes[0], 0xa5);
        restored
            .graphics
            .vga
            .lock()
            .unwrap()
            .write_framebuffer(64, &[0x56]);
        restored
            .graphics
            .read(mapped[0], 64, 2, &mut bytes)
            .unwrap();
        assert_eq!(bytes, [0x56, 0x34]);
    }

    #[test]
    fn adapter_checkpoint_rejects_pci_identity_shape_and_live_address_mismatches() {
        let mut fixture = Fixture::new();
        fixture.write(1, 3);
        let mut state = fixture.device.save().unwrap();
        let vm = Arc::new(setup_vm_with_memory(0x10000));
        *vm.resource_allocator() = fixture.vm.resource_allocator().clone();
        let config = state.config.clone();
        state.config = PciConfiguration::new_type0(
            0x1234,
            0x1111,
            1,
            PciClassCode::DisplayController,
            0,
            0x1234,
            0x1111,
        )
        .state();
        QxlDevice::restore(&vm, &state).unwrap_err();
        state.config = config;
        let bars = state.bars;
        for case in 0..5 {
            match case {
                0 => state.bars.bars[4].encoded_addr = 0x1000,
                1 => state.bars.bars[0].encoded_size = 0,
                2 => state.bars.bars[0].encoded_addr |= 8,
                3 => state.bars.bars[0].encoded_addr = u32::try_from(fixture.candidate(0)).unwrap(),
                _ => state.bars.bars[3].encoded_addr += 16,
            }
            QxlDevice::restore(&vm, &state).unwrap_err();
            state.bars = bars;
        }
        state.bars.bars[0].encoded_addr = u32::try_from(fixture.candidate(0)).unwrap();
        let mut config = configuration();
        config.write_reg(1, 0);
        state.config = config.state();
        let restored = QxlDevice::restore(&vm, &state).unwrap();
        assert_eq!(restored.graphics.mapped(), state.mapped);
        assert_eq!(restored.command(), 0);
    }
}
