// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! PCI and event-loop integration for the console USB tablet.

#[cfg(feature = "windows")]
use std::io;
#[cfg(feature = "windows")]
use std::os::fd::AsRawFd;
#[cfg(feature = "windows")]
use std::sync::atomic::AtomicU32;
#[cfg(feature = "windows")]
use std::sync::{Arc, Barrier, Mutex, Weak};
#[cfg(feature = "windows")]
use std::time::Duration;

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
use crate::devices::usb::uhci::Uhci;
use crate::devices::usb::uhci::UhciState;
use crate::pci::PciSBDF;
#[cfg(feature = "windows")]
use crate::pci::configuration::PciConfiguration;
use crate::pci::configuration::{Bars, PciConfigurationState};
#[cfg(feature = "windows")]
use crate::pci::intx::{PCI_INTX_GSI, PciIntxGroup};
#[cfg(feature = "windows")]
use crate::pci::{PciClassCode, PciDevice};
#[cfg(feature = "windows")]
use crate::vstate::bus::BusDevice;
#[cfg(feature = "windows")]
use crate::vstate::vm::KvmVm;

#[cfg(feature = "windows")]
const BAR_INDEX: u8 = 4;
#[cfg(feature = "windows")]
const BAR_SIZE: u64 = 32;
#[cfg(feature = "windows")]
const FRAME: Duration = Duration::from_millis(1);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsbState {
    core: UhciState,
    config: PciConfigurationState,
    bars: Bars,
    mapped: u64,
    legacy_support: u16,
    sbdf: PciSBDF,
}

#[derive(Debug)]
#[cfg(feature = "windows")]
pub struct UsbDevice {
    core: Uhci,
    config: PciConfiguration,
    bars: Bars,
    mapped: u64,
    legacy_support: u16,
    sbdf: PciSBDF,
    vm: Weak<KvmVm>,
    intx: Arc<Mutex<PciIntxGroup>>,
    timer: TimerFd,
    running: bool,
    registered: bool,
    bound: bool,
}

#[cfg(feature = "windows")]
fn configuration() -> PciConfiguration {
    let mut config = PciConfiguration::new_type0(
        0x1d0f,
        0xfc03,
        0,
        PciClassCode::SerialBusController,
        3,
        0x1d0f,
        0xfc03,
    );
    config.set_interrupt_line(u8::try_from(PCI_INTX_GSI).unwrap());
    config
}

#[cfg(feature = "windows")]
impl UsbDevice {
    pub fn attach(
        vm: &Arc<KvmVm>,
        pci: &mut PciDevices,
        state: Option<&UsbState>,
    ) -> io::Result<Arc<Mutex<Self>>> {
        let mut config = configuration();
        if state.is_some_and(|state| state.legacy_support & !0x2000 != 0) {
            return Err(io::Error::other("invalid USB legacy support state"));
        }
        let (core, sbdf, mapped) = if let Some(state) = state {
            if state.sbdf.segment() != 0
                || state.sbdf.bus() != 0
                || state.sbdf.function() != 0
                || state.sbdf.device() == 0
            {
                return Err(io::Error::other("invalid USB PCI address"));
            }
            let saved = PciConfiguration::type0_from_state(state.config.clone())
                .map_err(io::Error::other)?;
            for index in [1, 3, 15] {
                config.write_reg(index, saved.read_reg(index));
            }
            for index in 0..1024 {
                if config.read_reg(index) != saved.read_reg(index) {
                    return Err(io::Error::other("invalid USB PCI configuration"));
                }
            }
            let allocator = vm.resource_allocator().pci_io.clone();
            if state.mapped < allocator.base()
                || !state.mapped.is_multiple_of(BAR_SIZE)
                || state
                    .mapped
                    .checked_add(BAR_SIZE - 1)
                    .is_none_or(|end| end > allocator.end())
            {
                return Err(io::Error::other("invalid USB BAR allocation"));
            }
            let mut reservations = allocator;
            reservations
                .free(
                    &RangeInclusive::new(state.mapped, state.mapped + BAR_SIZE - 1)
                        .map_err(io::Error::other)?,
                )
                .map_err(io::Error::other)?;
            (Uhci::restore(&state.core)?, state.sbdf, state.mapped)
        } else {
            let sbdf = pci
                .pci_segment
                .next_device_sbdf()
                .map_err(io::Error::other)?;
            let mapped = vm
                .resource_allocator()
                .pci_io
                .allocate(BAR_SIZE, BAR_SIZE, AllocPolicy::FirstMatch)
                .map_err(io::Error::other)?
                .start();
            (Uhci::new(), sbdf, mapped)
        };
        let mut bars = Bars::default();
        bars.set_bar_io(
            BAR_INDEX,
            u32::try_from(mapped).map_err(io::Error::other)?,
            u32::try_from(BAR_SIZE).unwrap(),
        );
        if let Some(state) = state {
            for (index, (actual, expected)) in state.bars.bars.iter().zip(&bars.bars).enumerate() {
                if actual.encoded_size != expected.encoded_size
                    || (index != usize::from(BAR_INDEX) && actual.encoded_addr != 0)
                    || (index == usize::from(BAR_INDEX)
                        && actual.encoded_addr & !expected.encoded_size != 1)
                {
                    return Err(io::Error::other("invalid USB BAR shape"));
                }
            }
            let saved = state.bars.bars[usize::from(BAR_INDEX)];
            let probe = saved.encoded_addr == saved.encoded_size | 1;
            if config.read_reg(1) & 1 != 0 && !probe && u64::from(saved.encoded_addr & !3) != mapped
            {
                return Err(io::Error::other("USB BAR differs from live mapping"));
            }
            bars = state.bars;
        }
        let intx = match pci.ensure_console_intx(vm) {
            Ok(intx) => intx,
            Err(error) => {
                if state.is_none() {
                    vm.resource_allocator()
                        .pci_io
                        .free(&RangeInclusive::new(mapped, mapped + BAR_SIZE - 1).unwrap())
                        .map_err(io::Error::other)?;
                }
                return Err(error);
            }
        };
        let device = Arc::new(Mutex::new(Self {
            core,
            config,
            bars,
            mapped,
            legacy_support: state.map_or(0x2000, |state| state.legacy_support),
            sbdf,
            vm: Arc::downgrade(vm),
            intx,
            timer: TimerFd::new(),
            running: false,
            registered: false,
            bound: false,
        }));
        let attach =
            (|| -> io::Result<()> {
                pci.pci_segment
                    .pci_bus
                    .lock()
                    .unwrap()
                    .add_device(sbdf.device(), device.clone())
                    .map_err(io::Error::other)?;
                if let Err(error) = vm.pio_bus.insert(device.clone(), mapped, BAR_SIZE) {
                    pci.pci_segment
                        .pci_bus
                        .lock()
                        .unwrap()
                        .remove_device(sbdf.device());
                    return Err(io::Error::other(error));
                }
                let mut locked = device.lock().unwrap();
                if let Err(error) = locked.intx.lock().unwrap().bind(
                    sbdf.device(),
                    Arc::new(AtomicU32::new(0)),
                    true,
                ) {
                    let _ = vm.pio_bus.remove(mapped, BAR_SIZE);
                    pci.pci_segment
                        .pci_bus
                        .lock()
                        .unwrap()
                        .remove_device(sbdf.device());
                    return Err(io::Error::other(error));
                }
                locked.bound = true;
                locked.sync_interrupt();
                Ok(())
            })();
        if let Err(error) = attach {
            if state.is_none() {
                let _ = vm
                    .resource_allocator()
                    .pci_io
                    .free(&RangeInclusive::new(mapped, mapped + BAR_SIZE - 1).unwrap());
            }
            return Err(error);
        }
        pci.pci_segment.pci_irq_slots[usize::from(sbdf.device())] =
            u8::try_from(PCI_INTX_GSI).unwrap();
        Ok(device)
    }

    pub fn save(&self) -> UsbState {
        UsbState {
            core: self.core.save(),
            config: self.config.state(),
            bars: self.bars,
            mapped: self.mapped,
            legacy_support: self.legacy_support,
            sbdf: self.sbdf,
        }
    }

    pub fn set_running(&mut self, running: bool) {
        self.running = running;
        self.timer.arm(
            if running { FRAME } else { Duration::ZERO },
            running.then_some(FRAME),
        );
        self.timer.read();
    }

    pub fn check_registered(&self) -> io::Result<()> {
        if self.registered {
            Ok(())
        } else {
            Err(io::Error::other("USB timer not registered"))
        }
    }

    pub fn reset_registration(&mut self) {
        self.registered = false;
    }
    pub fn tablet_ready(&self) -> bool {
        self.core.tablet_ready()
    }
    pub fn pointer(&mut self, x: u16, y: u16, buttons: u8, wheel: i8) -> io::Result<()> {
        self.core.pointer(x, y, buttons, wheel)
    }
    pub fn release(&mut self) -> io::Result<()> {
        self.core.release()
    }

    fn sync_interrupt(&mut self) {
        if !self.bound {
            return;
        }
        let mut group = self.intx.lock().unwrap();
        let result = (|| {
            group.mask(
                self.sbdf.device(),
                self.config.read_reg(1) & 0x400 != 0 || self.legacy_support & 0x2000 == 0,
            )?;
            if self.core.interrupt_pending() {
                group.raise(self.sbdf.device(), 1)
            } else {
                group.clear(self.sbdf.device()).map(|_| ())
            }
        })();
        if let Err(error) = result {
            crate::logger::error!("USB PCI interrupt: {error}");
        }
    }

    fn relocate(&mut self) {
        let bar = self.bars.bars[usize::from(BAR_INDEX)];
        if bar.encoded_addr == bar.encoded_size | 1 {
            return;
        }
        let next = u64::from(bar.encoded_addr & !3);
        if next == self.mapped {
            return;
        }
        let Some(vm) = self.vm.upgrade() else {
            return;
        };
        let mut allocator = vm.resource_allocator();
        let result = (|| -> io::Result<()> {
            let range = allocator
                .pci_io
                .allocate(BAR_SIZE, BAR_SIZE, AllocPolicy::ExactMatch(next))
                .map_err(io::Error::other)?;
            if let Err(error) = vm.pio_bus.move_range(self.mapped, next, BAR_SIZE) {
                let _ = allocator.pci_io.free(&range);
                return Err(io::Error::other(error));
            }
            allocator
                .pci_io
                .free(&RangeInclusive::new(self.mapped, self.mapped + BAR_SIZE - 1).unwrap())
                .map_err(io::Error::other)?;
            Ok(())
        })();
        match result {
            Ok(()) => self.mapped = next,
            Err(error) => {
                crate::logger::warn!("Cannot relocate USB BAR: {error}");
                self.bars.write(
                    BAR_INDEX,
                    0,
                    &u32::try_from(self.mapped).unwrap().to_le_bytes(),
                );
            }
        }
    }
}

#[cfg(feature = "windows")]
impl Drop for UsbDevice {
    fn drop(&mut self) {
        if self.bound {
            let _ = self.intx.lock().unwrap().unbind(self.sbdf.device());
        }
    }
}

#[cfg(feature = "windows")]
impl PciDevice for UsbDevice {
    fn read_config_register(&mut self, index: u16) -> u32 {
        if index == 0x30 {
            return u32::from(self.legacy_support)
                | if self.core.interrupt_pending() {
                    0x1000
                } else {
                    0
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
        data: &[u8],
    ) -> Option<Arc<Barrier>> {
        if index == 0x30 {
            let mut value = u32::from(self.legacy_support).to_le_bytes();
            if let Some(target) =
                value.get_mut(usize::from(offset)..usize::from(offset) + data.len())
            {
                target.copy_from_slice(data);
                self.legacy_support = u16::try_from(u32::from_le_bytes(value) & 0x2000).unwrap();
            }
        } else if (4..10).contains(&index) {
            self.bars
                .write(u8::try_from(index - 4).unwrap(), offset, data);
        } else {
            self.config.write_config_register(index, offset, data);
        }
        if self.config.read_reg(1) & 1 != 0 {
            self.relocate();
        }
        self.sync_interrupt();
        None
    }
}

#[cfg(feature = "windows")]
impl BusDevice for UsbDevice {
    fn read(&mut self, base: u64, offset: u64, data: &mut [u8]) {
        data.fill(0xff);
        if base == self.mapped && self.config.read_reg(1) & 1 != 0 {
            self.core.read(offset, data);
        }
        self.sync_interrupt();
    }
    fn write(&mut self, base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        if base == self.mapped && self.config.read_reg(1) & 1 != 0 {
            self.core.write(offset, data);
        }
        self.sync_interrupt();
        None
    }
}

#[cfg(feature = "windows")]
impl MutEventSubscriber for UsbDevice {
    fn init(&mut self, ops: &mut EventOps) {
        self.registered = ops.add(Events::new(&self.timer, EventSet::IN)).is_ok();
    }
    fn process(&mut self, event: Events, _ops: &mut EventOps) {
        if event.fd() != self.timer.as_raw_fd() {
            return;
        }
        let count = self.timer.read().min(32);
        if self.running
            && self.config.read_reg(1) & 4 != 0
            && let Some(vm) = self.vm.upgrade()
        {
            for _ in 0..count {
                self.core.tick(vm.guest_memory());
            }
        }
        self.sync_interrupt();
    }
}

#[cfg(test)]
#[cfg(feature = "windows")]
mod tests {
    use super::*;
    use crate::snapshot::Persist;
    use crate::vstate::resources::ResourceAllocator;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    fn platform() -> (Arc<KvmVm>, PciDevices) {
        let mut vm = setup_vm_with_memory(0x10000);
        vm.setup_irqchip(true).unwrap();
        let vm = Arc::new(vm);
        let pci = PciDevices::new(&vm).unwrap();
        (vm, pci)
    }

    #[test]
    fn failed_interrupt_setup_releases_new_bar() {
        let (vm, mut pci) = platform();
        let mut before = vm.resource_allocator().pci_io.clone();
        let expected = before
            .allocate(BAR_SIZE, BAR_SIZE, AllocPolicy::FirstMatch)
            .unwrap();
        vm.resource_allocator()
            .gsi_legacy_allocator
            .allocate_id_at(PCI_INTX_GSI)
            .unwrap();
        UsbDevice::attach(&vm, &mut pci, None).err().unwrap();
        let actual = vm
            .resource_allocator()
            .pci_io
            .allocate(BAR_SIZE, BAR_SIZE, AllocPolicy::FirstMatch)
            .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn pci_probe_relocation_decode_and_snapshot() {
        let (vm, mut pci) = platform();
        let usb = UsbDevice::attach(&vm, &mut pci, None).unwrap();
        let original = usb.lock().unwrap().mapped;
        assert_eq!(usb.lock().unwrap().read_config_register(2), 0x0c03_0000);
        let mut status = [0; 2];
        vm.pio_bus.read(original + 2, &mut status).unwrap();
        assert_eq!(status, [0xff; 2]);
        usb.lock()
            .unwrap()
            .write_config_register(1, 0, &5u16.to_le_bytes());
        vm.pio_bus.read(original + 2, &mut status).unwrap();
        assert_eq!(u16::from_le_bytes(status), 32);
        usb.lock()
            .unwrap()
            .write_config_register(8, 0, &u32::MAX.to_le_bytes());
        assert_eq!(usb.lock().unwrap().read_config_register(8), 0xffff_ffe1);
        usb.lock()
            .unwrap()
            .write_config_register(8, 0, &0x3001u32.to_le_bytes());
        vm.pio_bus.read(original, &mut status).unwrap_err();
        vm.pio_bus.read(0x3002, &mut status).unwrap();
        assert_eq!(u16::from_le_bytes(status), 32);
        usb.lock()
            .unwrap()
            .write_config_register(0x30, 0, &0u16.to_le_bytes());
        usb.lock().unwrap().core.write(0, &1u16.to_le_bytes());
        let state = usb.lock().unwrap().save();
        let resources = vm.resource_allocator().save();
        let (target, mut target_pci) = platform();
        *target.resource_allocator() = ResourceAllocator::restore((), &resources).unwrap();
        let restored = UsbDevice::attach(&target, &mut target_pci, Some(&state)).unwrap();
        assert_eq!(restored.lock().unwrap().read_config_register(8), 0x3001);
        assert_eq!(restored.lock().unwrap().read_config_register(0x30), 0);
        assert!(!restored.lock().unwrap().running);
        assert!(!restored.lock().unwrap().timer.is_armed());
        let mut command = [0; 2];
        restored.lock().unwrap().core.read(0, &mut command);
        assert_eq!(command, 1u16.to_le_bytes());
        target.pio_bus.read(0x3002, &mut status).unwrap();
        assert_eq!(u16::from_le_bytes(status), 0);
    }

    #[test]
    fn snapshots_reject_forged_identity_mapping_and_bar() {
        let (vm, mut pci) = platform();
        let usb = UsbDevice::attach(&vm, &mut pci, None).unwrap();
        let state = usb.lock().unwrap().save();
        for corruption in 0..4 {
            let mut bad = state.clone();
            match corruption {
                0 => bad.mapped += 1,
                1 => bad.legacy_support |= 1,
                2 => bad.bars.bars[4].encoded_size = 0,
                _ => bad.sbdf = PciSBDF::new(1, 0, 1, 0),
            }
            UsbDevice::attach(&vm, &mut pci, Some(&bad)).err().unwrap();
        }
    }

    #[test]
    fn legacy_routing_status_and_pending_interrupt_survive_restore() {
        let (vm, mut pci) = platform();
        let usb = UsbDevice::attach(&vm, &mut pci, None).unwrap();
        {
            let mut usb = usb.lock().unwrap();
            assert_eq!(usb.read_config_register(0x30), 0x2000);
            usb.core.write(8, &0x10000u32.to_le_bytes());
            usb.core.write(0, &1u16.to_le_bytes());
            usb.core.tick(vm.guest_memory());
            assert!(usb.core.interrupt_pending());
            assert_eq!(usb.read_config_register(0x30), 0x3000);
            usb.write_config_register(0x30, 0, &0xffff_dfffu32.to_le_bytes());
            assert_eq!(usb.read_config_register(0x30), 0x1000);
        }
        let state = usb.lock().unwrap().save();
        let resources = vm.resource_allocator().save();
        let (target, mut target_pci) = platform();
        *target.resource_allocator() = ResourceAllocator::restore((), &resources).unwrap();
        let restored = UsbDevice::attach(&target, &mut target_pci, Some(&state)).unwrap();
        let mut restored = restored.lock().unwrap();
        assert!(!restored.timer.is_armed());
        assert_eq!(restored.read_config_register(0x30), 0x1000);
        restored.write_config_register(0x30, 1, &[0x20]);
        assert_eq!(restored.read_config_register(0x30), 0x3000);
        restored.core.write(2, &8u16.to_le_bytes());
        restored.sync_interrupt();
        assert_eq!(restored.read_config_register(0x30), 0x2000);
    }

    #[test]
    fn pause_disarms_timer_and_device_does_not_retain_vm() {
        let (vm, mut pci) = platform();
        let usb = UsbDevice::attach(&vm, &mut pci, None).unwrap();
        usb.lock().unwrap().set_running(true);
        assert!(usb.lock().unwrap().timer.is_armed());
        usb.lock().unwrap().set_running(false);
        assert!(!usb.lock().unwrap().timer.is_armed());
        let weak = Arc::downgrade(&vm);
        drop(vm);
        assert!(weak.upgrade().is_none());
    }
}
