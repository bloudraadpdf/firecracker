// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! QXL console attachment, interrupt delivery and bounded queue polling.

#[cfg(feature = "windows")]
use std::io;
#[cfg(feature = "windows")]
use std::os::fd::AsRawFd;
#[cfg(feature = "windows")]
use std::path::Path;
#[cfg(feature = "windows")]
use std::sync::atomic::AtomicU32;
#[cfg(feature = "windows")]
use std::sync::{Arc, Barrier, Mutex, Weak};
#[cfg(feature = "windows")]
use std::time::{Duration, Instant};

#[cfg(feature = "windows")]
use event_manager::{EventOps, Events, MutEventSubscriber};
use serde::{Deserialize, Serialize};
#[cfg(feature = "windows")]
use utils::time::TimerFd;
#[cfg(feature = "windows")]
use vm_allocator::{AllocPolicy, RangeInclusive};
#[cfg(feature = "windows")]
use vmm_sys_util::epoll::EventSet;

#[cfg(feature = "windows")]
use super::pci_mngr::PciDevices;
#[cfg(feature = "windows")]
use crate::devices::display::pci::{allocate_mappings, validate_mappings};
use crate::devices::display::rom::ExpansionRom;
#[cfg(feature = "windows")]
use crate::devices::display::rom::validate_rom;
#[cfg(feature = "windows")]
use crate::devices::display::{Frame, Vga};
#[cfg(feature = "windows")]
use crate::devices::qxl::pci::device::{IO_SIZE, QxlDevice};
#[cfg(feature = "windows")]
use crate::devices::qxl::pci::{BAR_SIZES, QxlPci};
use crate::devices::qxl::state::PciState;
#[cfg(feature = "windows")]
use crate::pci::PciDevice;
use crate::pci::PciSBDF;
#[cfg(feature = "windows")]
use crate::pci::intx::{PCI_INTX_GSI, PciIntxGroup};
#[cfg(feature = "windows")]
use crate::vstate::bus::BusDevice;
#[cfg(feature = "windows")]
use crate::vstate::vm::KvmVm;

#[cfg(feature = "windows")]
const TICK: Duration = Duration::from_millis(1);
#[cfg(feature = "windows")]
const LEGACY_IO: [(u64, u64); 2] = [(0x3b0, 0x30), (0x1ce, 4)];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QxlState {
    core: PciState,
    sbdf: PciSBDF,
    rom: Option<ExpansionRom>,
    clock: u64,
    failed: bool,
}

#[cfg(feature = "windows")]
impl QxlState {
    pub(super) fn has_rom(&self) -> bool {
        self.rom.is_some()
    }
}

#[cfg(feature = "windows")]
#[derive(Debug)]
pub struct QxlConsole {
    core: QxlDevice,
    sbdf: PciSBDF,
    rom: Option<ExpansionRom>,
    vm: Weak<KvmVm>,
    intx: Arc<Mutex<PciIntxGroup>>,
    bound: bool,
    timer: TimerFd,
    registration: Result<(), String>,
    clock: u64,
    tick: Instant,
    running: bool,
    failed: bool,
}

#[cfg(feature = "windows")]
impl QxlConsole {
    pub fn attach(
        vm: &Arc<KvmVm>,
        pci: &mut PciDevices,
        rom_path: Option<&Path>,
        state: Option<&QxlState>,
    ) -> io::Result<Arc<Mutex<Self>>> {
        let bytes = rom_path.map(ExpansionRom::load).transpose()?;
        let (core, sbdf, rom) = if let Some(state) = state {
            if state.sbdf.device() == 0
                || state.sbdf.device() >= 32
                || state.sbdf.bus() != 0
                || state.sbdf.segment() != 0
                || state.sbdf.function() != 0
                || state.rom.as_ref().map(|rom| &rom.bytes) != bytes.as_ref()
            {
                return Err(io::Error::other("QXL snapshot identity or ROM differs"));
            }
            let mut core = QxlDevice::restore(vm, &state.core)?;
            if let Some(rom) = &state.rom {
                validate_rom(&rom.bytes)?;
                validate_mappings(
                    &vm.resource_allocator().mmio32_memory,
                    core.graphics()
                        .mapped()
                        .into_iter()
                        .zip(BAR_SIZES)
                        .chain(std::iter::once(rom.region())),
                )?;
                let mask = !(u32::try_from(rom.region().1).map_err(io::Error::other)? - 1);
                if rom.bar & !(mask | 1) != 0
                    || (rom.bar & 1 != 0 && u64::from(rom.bar & mask) != rom.mapped)
                {
                    return Err(io::Error::other("invalid QXL option ROM BAR"));
                }
            }
            (core, state.sbdf, state.rom.clone())
        } else {
            let sbdf = pci
                .pci_segment
                .next_device_sbdf()
                .map_err(io::Error::other)?;
            let mut resources = vm.resource_allocator();
            let mut memory = resources.mmio32_memory.clone();
            let mut ports = resources.pci_io.clone();
            let mapped = allocate_mappings(&mut memory, BAR_SIZES)?;
            let rom = bytes
                .map(|bytes| ExpansionRom::allocate(bytes, &mut memory))
                .transpose()?;
            let io = ports
                .allocate(IO_SIZE, IO_SIZE, AllocPolicy::FirstMatch)
                .map_err(io::Error::other)?
                .start();
            let previous = resources.clone();
            resources.mmio32_memory = memory;
            resources.pci_io = ports;
            drop(resources);
            let core = QxlPci::new(vm, Vga::default(), mapped)
                .and_then(|graphics| QxlDevice::new(graphics, io));
            match core {
                Ok(core) => (core, sbdf, rom),
                Err(error) => {
                    *vm.resource_allocator() = previous;
                    return Err(error);
                }
            }
        };
        let mut core = core;
        let mapped = core.graphics().mapped();
        let io_base = core.io_base();
        let regions: Vec<_> = mapped
            .into_iter()
            .zip(BAR_SIZES)
            .chain(rom.as_ref().map(ExpansionRom::region))
            .collect();
        let free = || {
            if state.is_none() {
                let mut resources = vm.resource_allocator();
                for (base, size) in &regions {
                    let _ = resources
                        .mmio32_memory
                        .free(&RangeInclusive::new(*base, base + size - 1).unwrap());
                }
                let _ = resources
                    .pci_io
                    .free(&RangeInclusive::new(io_base, io_base + IO_SIZE - 1).unwrap());
            }
        };
        let intx = match pci.ensure_console_intx(vm) {
            Ok(intx) => intx,
            Err(error) => {
                free();
                return Err(error);
            }
        };
        let device = Arc::new(Mutex::new(Self {
            core,
            sbdf,
            rom,
            vm: Arc::downgrade(vm),
            intx,
            bound: false,
            timer: TimerFd::new(),
            registration: Err("QXL timer not registered".to_owned()),
            clock: state.map_or(0, |state| state.clock),
            tick: Instant::now(),
            running: false,
            failed: state.is_some_and(|state| state.failed),
        }));
        let mut pci_inserted = false;
        let mut mmio_inserted = Vec::new();
        let mut pio_inserted = Vec::new();
        let result = (|| -> io::Result<()> {
            pci.pci_segment
                .pci_bus
                .lock()
                .unwrap()
                .add_device(sbdf.device(), device.clone())
                .map_err(io::Error::other)?;
            pci_inserted = true;
            for (base, size) in regions
                .iter()
                .copied()
                .chain(std::iter::once((0xa0000, 0x20000)))
            {
                vm.common
                    .mmio_bus
                    .insert(device.clone(), base, size)
                    .map_err(io::Error::other)?;
                mmio_inserted.push((base, size));
            }
            for (base, size) in LEGACY_IO
                .into_iter()
                .chain(std::iter::once((io_base, IO_SIZE)))
            {
                vm.pio_bus
                    .insert(device.clone(), base, size)
                    .map_err(io::Error::other)?;
                pio_inserted.push((base, size));
            }
            let mut locked = device.lock().unwrap();
            locked
                .intx
                .lock()
                .unwrap()
                .bind(sbdf.device(), Arc::new(AtomicU32::new(0)), true)
                .map_err(io::Error::other)?;
            locked.bound = true;
            if state.is_none() {
                locked.core.write_config_register(1, 0, &3u32.to_le_bytes());
                if locked.core.command() & 3 != 3 {
                    return Err(io::Error::other("QXL memory decode failed"));
                }
            }
            locked.sync_interrupt();
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
            drop(device);
            free();
            return Err(error);
        }
        pci.pci_segment.pci_irq_slots[usize::from(sbdf.device())] =
            u8::try_from(PCI_INTX_GSI).unwrap();
        Ok(device)
    }

    fn now(&self) -> u64 {
        self.clock.saturating_add(if self.running {
            u64::try_from(self.tick.elapsed().as_millis()).unwrap_or(u64::MAX)
        } else {
            0
        })
    }

    pub fn set_running(&mut self, running: bool) {
        self.clock = self.now();
        self.tick = Instant::now();
        self.running = running;
        self.timer.arm(
            if running { TICK } else { Duration::ZERO },
            running.then_some(TICK),
        );
        self.timer.read();
    }

    pub fn save(&self) -> io::Result<QxlState> {
        Ok(QxlState {
            core: self.core.save()?,
            sbdf: self.sbdf,
            rom: self.rom.clone(),
            clock: self.now(),
            failed: self.failed,
        })
    }

    pub fn vga(&mut self) -> Arc<Mutex<Vga>> {
        self.core.graphics().vga()
    }

    pub fn frame(&mut self) -> Frame {
        let now = self.now();
        match self.core.graphics().controller().frame(now) {
            Ok(Some(frame)) => return frame,
            Ok(None) => (),
            Err(error) => self.fail(error),
        }
        self.vga().lock().unwrap().render()
    }

    pub fn check_registered(&self) -> io::Result<()> {
        self.registration
            .as_ref()
            .copied()
            .map_err(|error| io::Error::other(error.clone()))
    }

    pub fn reset_registration(&mut self) {
        self.registration = Err("QXL timer not registered".to_owned());
    }

    fn fail(&mut self, error: impl std::fmt::Display) {
        self.failed = true;
        crate::logger::error!("QXL protocol: {error}");
        if let Err(error) = self.core.graphics().controller().memory().interrupt(1 << 3) {
            crate::logger::error!("QXL error interrupt: {error}");
        }
        self.sync_interrupt();
    }

    fn poll(&mut self) {
        if !self.failed {
            let now = self.now();
            if let Err(error) = self.core.graphics().controller().poll(now) {
                self.fail(error);
            }
        }
        self.sync_interrupt();
    }

    fn sync_interrupt(&mut self) {
        if !self.bound {
            return;
        }
        let masked = self.core.command() & 0x400 != 0;
        let pending = self.core.graphics().controller().irq_level();
        let mut group = self.intx.lock().unwrap();
        let result = (|| -> io::Result<()> {
            group
                .mask(self.sbdf.device(), masked)
                .map_err(io::Error::other)?;
            if pending.map_err(io::Error::other)? {
                group.raise(self.sbdf.device(), 1).map_err(io::Error::other)
            } else {
                group
                    .clear(self.sbdf.device())
                    .map(|_| ())
                    .map_err(io::Error::other)
            }
        })();
        if let Err(error) = result {
            crate::logger::error!("QXL PCI interrupt: {error}");
        }
    }
}

#[cfg(feature = "windows")]
impl Drop for QxlConsole {
    fn drop(&mut self) {
        if self.bound
            && let Err(error) = self.intx.lock().unwrap().unbind(self.sbdf.device())
        {
            crate::logger::error!("QXL interrupt detach: {error}");
        }
    }
}

#[cfg(feature = "windows")]
impl PciDevice for QxlConsole {
    fn read_config_register(&mut self, index: u16) -> u32 {
        if index == 12 {
            self.rom.as_ref().map_or(0, ExpansionRom::read_bar)
        } else {
            self.core.read_config_register(index)
        }
    }

    fn write_config_register(
        &mut self,
        index: u16,
        offset: u8,
        bytes: &[u8],
    ) -> Option<Arc<Barrier>> {
        if index == 12 {
            if let Some(rom) = &mut self.rom {
                rom.write_bar(offset, bytes);
                if let Some(vm) = self.vm.upgrade() {
                    rom.relocate(&vm);
                }
            }
        } else {
            self.core.write_config_register(index, offset, bytes);
        }
        self.sync_interrupt();
        None
    }
}

#[cfg(feature = "windows")]
impl BusDevice for QxlConsole {
    fn read(&mut self, base: u64, offset: u64, bytes: &mut [u8]) {
        let command = self.core.command();
        if let Err(error) = self.core.graphics().read(base, offset, command, bytes) {
            self.fail(error);
        }
        if command & 2 != 0
            && let Some(rom) = &self.rom
        {
            rom.read(base, offset, bytes);
        }
    }

    fn write(&mut self, base: u64, offset: u64, bytes: &[u8]) -> Option<Arc<Barrier>> {
        let command = self.core.command();
        let result = if base == self.core.io_base() {
            if command & 1 == 0 || (self.failed && !matches!(offset, 3 | 5)) {
                return None;
            }
            let now = self.now();
            let result = self.core.graphics().write_io(offset, bytes, now);
            if result.is_ok() && offset == 5 {
                self.failed = false;
            }
            result
        } else {
            self.core.graphics().write(base, offset, command, bytes)
        };
        if let Err(error) = result {
            self.fail(error);
        }
        self.sync_interrupt();
        None
    }
}

#[cfg(feature = "windows")]
impl MutEventSubscriber for QxlConsole {
    fn init(&mut self, ops: &mut EventOps) {
        self.registration = ops
            .add(Events::new(&self.timer, EventSet::IN))
            .map_err(|error| error.to_string());
    }
    fn process(&mut self, event: Events, _ops: &mut EventOps) {
        if event.fd() == self.timer.as_raw_fd() && event.event_set().contains(EventSet::IN) {
            self.timer.read();
            if self.running {
                self.poll();
            }
        }
    }
}

#[cfg(feature = "windows")]
#[cfg(test)]
mod tests {
    use super::*;
    use event_manager::SubscriberOps;

    use crate::devices::qxl::memory::{
        COMMAND_RING, CREATE_SURFACE, MEMORY_SLOT, RAM_HEADER, RELEASE_RING, UPDATE_AREA,
    };
    use crate::vstate::vm::tests::setup_vm_with_memory;

    const SLOT: u64 = 0x0101_0000_0000_0000;

    fn platform() -> (Arc<KvmVm>, PciDevices) {
        let mut vm = setup_vm_with_memory(0x10000);
        vm.setup_irqchip(true).unwrap();
        let vm = Arc::new(vm);
        let pci = PciDevices::new(&vm).unwrap();
        (vm, pci)
    }

    struct Fixture {
        vm: Arc<KvmVm>,
        pci: PciDevices,
        device: Arc<Mutex<QxlConsole>>,
        ram: u64,
        io: u64,
    }

    impl Fixture {
        fn new() -> Self {
            let (vm, mut pci) = platform();
            let device = QxlConsole::attach(&vm, &mut pci, None, None).unwrap();
            let (ram, io) = {
                let mut locked = device.lock().unwrap();
                (locked.core.graphics().mapped()[0], locked.core.io_base())
            };
            let fixture = Self {
                vm,
                pci,
                device,
                ram,
                io,
            };
            fixture.store(MEMORY_SLOT, &ram.to_le_bytes());
            fixture.store(MEMORY_SLOT + 8, &(ram + RAM_HEADER as u64).to_le_bytes());
            fixture.vm.pio_bus.write(io + 8, &[1]).unwrap();
            let mut surface = [0; 40];
            for (offset, value) in [(0, 4u32), (4, 2), (8, 16), (12, 32), (24, 1)] {
                surface[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }
            surface[32..40].copy_from_slice(&(SLOT | 64).to_le_bytes());
            fixture.store(CREATE_SURFACE, &surface);
            fixture.store(64, &0x123456u32.to_le_bytes());
            fixture.vm.pio_bus.write(io + 12, &[0]).unwrap();
            fixture
        }

        fn store(&self, offset: usize, bytes: &[u8]) {
            self.vm
                .common
                .mmio_bus
                .write(self.ram + offset as u64, bytes)
                .unwrap();
        }

        fn word(&self, offset: usize) -> u32 {
            let mut bytes = [0; 4];
            self.vm
                .common
                .mmio_bus
                .read(self.ram + offset as u64, &mut bytes)
                .unwrap();
            u32::from_le_bytes(bytes)
        }

        fn submit(&self, sequence: u32) {
            let offset = 4096 + sequence as usize * 16;
            self.store(offset, &(u64::from(sequence) + 100).to_le_bytes());
            let mut command = [0; 16];
            command[..8].copy_from_slice(&(SLOT | offset as u64).to_le_bytes());
            command[8..12].copy_from_slice(&4u32.to_le_bytes());
            self.store(COMMAND_RING + 20 + (sequence & 31) as usize * 16, &command);
            self.store(COMMAND_RING + 4, &(sequence + 1).to_le_bytes());
            self.vm.pio_bus.write(self.io, &[0]).unwrap();
        }
    }

    #[test]
    fn attached_device_processes_ports_frames_interrupts_and_deferred_timer_work() {
        let fixture = Fixture::new();
        assert_eq!(fixture.device.lock().unwrap().frame().pixels[0], 0x123456);
        fixture.store(RAM_HEADER + 8, &1u32.to_le_bytes());
        for sequence in 0..8 {
            fixture.submit(sequence);
        }
        assert_eq!(fixture.word(RELEASE_RING + 4), 8);
        fixture.submit(8);
        fixture.store(UPDATE_AREA, &[0; 16]);
        fixture.vm.pio_bus.write(fixture.io + 2, &[0]).unwrap();
        assert_eq!(fixture.word(COMMAND_RING + 12), 9);
        assert_eq!(fixture.word(RELEASE_RING + 4), 8);
        let mut events = crate::EventManager::new().unwrap();
        let group = fixture.pci.unregistered_intx().unwrap();
        events.add_subscriber(group.clone());
        events.add_subscriber(fixture.device.clone());
        fixture.device.lock().unwrap().check_registered().unwrap();
        let slot = fixture.device.lock().unwrap().sbdf.device();
        assert_eq!(group.lock().unwrap().clear(slot).unwrap(), 1);
        fixture.store(RELEASE_RING + 12, &4u32.to_le_bytes());
        fixture.device.lock().unwrap().set_running(true);
        events.run_with_timeout(50).unwrap();
        fixture.device.lock().unwrap().set_running(false);
        assert_eq!(fixture.word(RELEASE_RING + 4), 9);
        fixture.store(RAM_HEADER + 4, &0u32.to_le_bytes());
        fixture.vm.pio_bus.write(fixture.io + 3, &[0]).unwrap();
        assert_eq!(group.lock().unwrap().clear(slot).unwrap(), 0);
        assert!(!fixture.device.lock().unwrap().failed);
    }

    #[test]
    fn invalid_io_raises_guest_error_and_reset_recovers_the_device() {
        let fixture = Fixture::new();
        fixture.store(RAM_HEADER + 8, &8u32.to_le_bytes());
        fixture.vm.pio_bus.write(fixture.io, &[0, 0]).unwrap();
        assert!(fixture.device.lock().unwrap().failed);
        assert_ne!(fixture.word(RAM_HEADER + 4) & 8, 0);
        fixture.store(RAM_HEADER + 4, &0u32.to_le_bytes());
        fixture.vm.pio_bus.write(fixture.io + 3, &[0]).unwrap();
        {
            let device = fixture.device.lock().unwrap();
            assert!(device.failed);
            assert_eq!(
                device
                    .intx
                    .lock()
                    .unwrap()
                    .clear(device.sbdf.device())
                    .unwrap(),
                0
            );
        }
        fixture.vm.pio_bus.write(fixture.io + 5, &[0]).unwrap();
        assert!(!fixture.device.lock().unwrap().failed);
        assert_eq!(fixture.word(RAM_HEADER + 4), 0);
    }

    #[test]
    fn attached_snapshot_restores_frame_ports_and_stopped_virtual_clock() {
        let source = Fixture::new();
        let state = source.device.lock().unwrap().save().unwrap();
        let (vm, mut pci) = platform();
        pci.ensure_console_intx(&vm).unwrap();
        *vm.resource_allocator() = source.vm.resource_allocator().clone();
        let device = QxlConsole::attach(&vm, &mut pci, None, Some(&state)).unwrap();
        let mut restored = device.lock().unwrap();
        assert_eq!(restored.frame().pixels[0], 0x123456);
        assert!(!restored.running);
        assert!(!restored.timer.is_armed());
        assert_eq!(restored.now(), state.clock);
        assert_eq!(restored.core.io_base(), source.io);
        assert_eq!(restored.read_config_register(0), 0x0100_1b36);
    }

    #[test]
    fn failed_bus_attachment_keeps_existing_device_and_releases_new_resources() {
        struct Existing;
        impl BusDevice for Existing {
            fn read(&mut self, _: u64, _: u64, bytes: &mut [u8]) {
                bytes.fill(0x66);
            }
        }
        let (vm, mut pci) = platform();
        let before = vm.resource_allocator().clone();
        let existing = Arc::new(Mutex::new(Existing));
        vm.pio_bus.insert(existing.clone(), 0x3b0, 0x30).unwrap();
        QxlConsole::attach(&vm, &mut pci, None, None).unwrap_err();
        let resources = vm.resource_allocator();
        assert_eq!(
            bitcode::serialize(&resources.mmio32_memory).unwrap(),
            bitcode::serialize(&before.mmio32_memory).unwrap()
        );
        assert_eq!(
            bitcode::serialize(&resources.pci_io).unwrap(),
            bitcode::serialize(&before.pci_io).unwrap()
        );
        drop(resources);
        assert_eq!(pci.pci_segment.next_device_sbdf().unwrap().device(), 1);
        let mut byte = [0];
        vm.pio_bus.read(0x3b0, &mut byte).unwrap();
        assert_eq!(byte, [0x66]);
    }

    #[test]
    fn option_rom_probe_decode_relocation_and_identity_follow_console_policy() {
        use std::io::Write;
        use vmm_sys_util::tempfile::TempFile;
        let file = TempFile::new().unwrap();
        let mut bytes = [0; 512];
        bytes[..3].copy_from_slice(&[0x55, 0xaa, 1]);
        file.as_file().write_all(&bytes).unwrap();
        let (vm, mut pci) = platform();
        let device = QxlConsole::attach(&vm, &mut pci, Some(file.as_path()), None).unwrap();
        let mut locked = device.lock().unwrap();
        let (original, size) = locked.rom.as_ref().unwrap().region();
        locked.write_config_register(12, 0, &u32::MAX.to_le_bytes());
        assert_eq!(locked.read_config_register(12), 0xffff_f800);
        let next = vm
            .resource_allocator()
            .mmio32_memory
            .clone()
            .allocate(size, size, AllocPolicy::FirstMatch)
            .unwrap()
            .start();
        locked.write_config_register(12, 0, &(u32::try_from(next).unwrap() | 1).to_le_bytes());
        drop(locked);
        let mut header = [0; 3];
        vm.common.mmio_bus.read(original, &mut header).unwrap_err();
        vm.common.mmio_bus.read(next, &mut header).unwrap();
        assert_eq!(header, [0x55, 0xaa, 1]);
        device
            .lock()
            .unwrap()
            .write_config_register(1, 0, &1u32.to_le_bytes());
        vm.common.mmio_bus.read(next, &mut header).unwrap();
        assert_eq!(header, [0xff; 3]);
        let state = device.lock().unwrap().save().unwrap();
        QxlConsole::attach(&vm, &mut pci, None, Some(&state)).unwrap_err();
    }
}
