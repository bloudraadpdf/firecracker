// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! PCI IDE discovery, mode switching and verified installation media.

#[cfg(feature = "windows")]
use std::fs::File;
#[cfg(feature = "windows")]
use std::io::{self, Read};
#[cfg(feature = "windows")]
use std::path::Path;
#[cfg(feature = "windows")]
use std::sync::atomic::AtomicU32;
#[cfg(feature = "windows")]
use std::sync::{Arc, Barrier, Mutex, Weak};

#[cfg(feature = "windows")]
use aws_lc_rs::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
#[cfg(feature = "windows")]
use vm_allocator::{AddressAllocator, AllocPolicy, RangeInclusive};
#[cfg(feature = "windows")]
use vmm_sys_util::eventfd::EventFd;

#[cfg(feature = "windows")]
use super::pci_mngr::PciDevices;
use crate::devices::legacy::atapi::AtapiState;
#[cfg(feature = "windows")]
use crate::devices::legacy::atapi::{Atapi, COMMAND_PORT, CONTROL_PORT, Channel, IRQ};
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

// Experimental identity, not a registered production PCI device ID.
#[cfg(feature = "windows")]
const VENDOR_ID: u16 = 0x1d0f;
#[cfg(feature = "windows")]
const DEVICE_ID: u16 = 0xfc01;
#[cfg(feature = "windows")]
const BAR_SIZES: [u64; 4] = [8, 4, 8, 4];
#[cfg(feature = "windows")]
const LEGACY_PORTS: [(u64, u64); 4] =
    [(COMMAND_PORT, 8), (CONTROL_PORT, 1), (0x170, 8), (0x376, 1)];

/// Media identity, transfer state and PCI resources required for restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpticalState {
    cores: [Option<AtapiState>; 2],
    config: PciConfigurationState,
    bars: Bars,
    mapped: [u64; 4],
    modes: u8,
    sbdf: PciSBDF,
}

/// Two switchable IDE channels, each with one optional read-only master.
#[derive(Debug)]
#[cfg(feature = "windows")]
pub struct OpticalDevice {
    cores: [Option<Atapi>; 2],
    irqs: [Option<EventFd>; 2],
    config: PciConfiguration,
    bars: Bars,
    mapped: [u64; 4],
    modes: u8,
    sbdf: PciSBDF,
    vm: Weak<KvmVm>,
    intx: Arc<Mutex<PciIntxGroup>>,
    bound: bool,
}

#[cfg(feature = "windows")]
fn configuration() -> PciConfiguration {
    let mut config = PciConfiguration::new_type0(
        VENDOR_ID,
        DEVICE_ID,
        0,
        PciClassCode::MassStorageController,
        1,
        VENDOR_ID,
        DEVICE_ID,
    );
    config.write_reg(1, 1);
    config.set_interrupt_line(u8::try_from(PCI_INTX_GSI).unwrap());
    config
}

#[cfg(feature = "windows")]
fn restore_configuration(state: &PciConfigurationState) -> io::Result<PciConfiguration> {
    let saved = PciConfiguration::type0_from_state(state.clone()).map_err(io::Error::other)?;
    let mut config = configuration();
    for register in [1, 3, 15] {
        config.write_reg(register, saved.read_reg(register));
    }
    for register in 0..1024 {
        if config.read_reg(register) != saved.read_reg(register) {
            return Err(io::Error::other("invalid optical PCI configuration"));
        }
    }
    Ok(config)
}

#[cfg(feature = "windows")]
fn make_bars(mapped: [u64; 4], saved: Option<&Bars>) -> io::Result<Bars> {
    let mut bars = Bars::default();
    for (index, (&base, &size)) in mapped.iter().zip(&BAR_SIZES).enumerate() {
        bars.set_bar_io(
            u8::try_from(index).unwrap(),
            u32::try_from(base).map_err(io::Error::other)?,
            u32::try_from(size).unwrap(),
        );
    }
    if let Some(saved) = saved {
        for (index, (actual, expected)) in saved.bars.iter().zip(&bars.bars).enumerate() {
            if actual.encoded_size != expected.encoded_size
                || (index < 4 && actual.encoded_addr & !expected.encoded_size != 1)
                || (index >= 4 && actual.encoded_addr != 0)
            {
                return Err(io::Error::other("invalid optical BAR shape"));
            }
        }
        for index in 0..4u8 {
            bars.write(
                index,
                0,
                &saved.bars[usize::from(index)].encoded_addr.to_le_bytes(),
            );
        }
    }
    Ok(bars)
}

#[cfg(feature = "windows")]
fn validate_mappings(allocator: &AddressAllocator, state: &OpticalState) -> io::Result<()> {
    let mut reservations = allocator.clone();
    let io_enabled = state.config_command()? & 1 != 0;
    for (index, (&base, &size)) in state.mapped.iter().zip(&BAR_SIZES).enumerate() {
        if base < allocator.base()
            || !base.is_multiple_of(size)
            || base
                .checked_add(size - 1)
                .is_none_or(|end| end > allocator.end())
        {
            return Err(io::Error::other("invalid optical BAR allocation"));
        }
        reservations
            .free(&RangeInclusive::new(base, base + size - 1).map_err(io::Error::other)?)
            .map_err(io::Error::other)?;
        let bar = state.bars.bars[index];
        let probe = bar.encoded_addr == bar.encoded_size | 1;
        if io_enabled && !probe && u64::from(bar.encoded_addr & !3) != base {
            return Err(io::Error::other("optical BAR differs from live mapping"));
        }
    }
    if state.modes & !5 != 0x0a {
        return Err(io::Error::other("invalid optical channel modes"));
    }
    Ok(())
}

#[cfg(feature = "windows")]
impl OpticalState {
    pub(super) fn matches_drive_count(&self, count: usize) -> bool {
        (1..=2).contains(&count)
            && self
                .cores
                .iter()
                .enumerate()
                .all(|(index, core)| core.is_some() == (index < count))
    }

    fn config_command(&self) -> io::Result<u32> {
        Ok(PciConfiguration::type0_from_state(self.config.clone())
            .map_err(io::Error::other)?
            .read_reg(1))
    }
}

#[cfg(feature = "windows")]
fn media_identity(media: &mut File) -> io::Result<String> {
    let mut digest = Context::new(&SHA256);
    let mut buffer = [0; 65536];
    loop {
        let count = media.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest
        .finish()
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(feature = "windows")]
fn open_media(path: Option<&Path>) -> io::Result<Option<(File, String)>> {
    path.map(|path| {
        let mut media = File::open(path)?;
        if !media.metadata()?.is_file() {
            return Err(io::Error::other("ISO must be a regular file"));
        }
        let identity = media_identity(&mut media)?;
        Ok((media, identity))
    })
    .transpose()
}

#[cfg(feature = "windows")]
impl OpticalDevice {
    /// Attaches fixed and native IDE ports; the caller reserves one IRQ per drive.
    pub fn attach(
        vm: &Arc<KvmVm>,
        pci: &mut PciDevices,
        paths: &[Option<&Path>],
        state: Option<&OpticalState>,
    ) -> io::Result<Arc<Mutex<Self>>> {
        if !(1..=2).contains(&paths.len())
            || state.is_some_and(|state| !state.matches_drive_count(paths.len()))
        {
            return Err(io::Error::other("optical drive topology disagrees"));
        }
        if let Some(state) = state {
            if state.sbdf.segment() != 0
                || state.sbdf.bus() != 0
                || state.sbdf.function() != 0
                || state.sbdf.device() == 0
            {
                return Err(io::Error::other("invalid optical PCI address"));
            }
            validate_mappings(&vm.resource_allocator().pci_io, state)?;
        }
        let mut cores = [None, None];
        let mut irqs = [None, None];
        for (channel, path) in paths.iter().enumerate() {
            let identity = if channel == 0 {
                Channel::Primary
            } else {
                Channel::Secondary
            };
            let media = open_media(*path)?;
            let irq = EventFd::new(libc::EFD_NONBLOCK)?;
            cores[channel] = Some(match state {
                Some(state) => Atapi::restore(
                    media,
                    irq.try_clone()?,
                    state.cores[channel].as_ref().unwrap(),
                    identity,
                )?,
                None => Atapi::new(media, irq.try_clone()?, identity)?,
            });
            irqs[channel] = Some(irq);
        }
        let (config, sbdf, modes) = match state {
            Some(state) => (
                restore_configuration(&state.config)?,
                state.sbdf,
                state.modes,
            ),
            None => (
                configuration(),
                pci.pci_segment
                    .next_device_sbdf()
                    .map_err(io::Error::other)?,
                0x0f,
            ),
        };
        let mapped = if let Some(state) = state {
            state.mapped
        } else {
            let mut allocator = vm.resource_allocator();
            let mut next = allocator.pci_io.clone();
            let mut mapped = [0; 4];
            for (base, size) in mapped.iter_mut().zip(BAR_SIZES) {
                *base = next
                    .allocate(size, size, AllocPolicy::FirstMatch)
                    .map_err(io::Error::other)?
                    .start();
            }
            allocator.pci_io = next;
            mapped
        };
        let release_allocations = || {
            if state.is_none() {
                for (base, size) in mapped.into_iter().zip(BAR_SIZES) {
                    let _ = vm
                        .resource_allocator()
                        .pci_io
                        .free(&RangeInclusive::new(base, base + size - 1).unwrap());
                }
            }
        };
        let setup = (|| -> io::Result<_> {
            Ok((
                make_bars(mapped, state.map(|s| &s.bars))?,
                pci.ensure_console_intx(vm)?,
            ))
        })();
        let (bars, intx) = match setup {
            Ok(value) => value,
            Err(error) => {
                release_allocations();
                return Err(error);
            }
        };
        for (channel, core) in cores.iter_mut().enumerate() {
            if let Some(core) = core {
                core.set_legacy_irq_enabled(
                    modes & (1 << (channel * 2)) == 0 && config.read_reg(1) & 1 != 0,
                );
            }
        }
        let device = Arc::new(Mutex::new(Self {
            cores,
            irqs: [None, None],
            config,
            bars,
            mapped,
            modes,
            sbdf,
            vm: Arc::downgrade(vm),
            intx,
            bound: false,
        }));
        if let Err(error) = pci
            .pci_segment
            .pci_bus
            .lock()
            .unwrap()
            .add_device(sbdf.device(), device.clone())
        {
            release_allocations();
            return Err(io::Error::other(error));
        }
        let mut inserted = Vec::new();
        let attach = (|| -> io::Result<()> {
            for (base, size) in LEGACY_PORTS
                .into_iter()
                .chain(mapped.into_iter().zip(BAR_SIZES))
            {
                vm.pio_bus
                    .insert(device.clone(), base, size)
                    .map_err(io::Error::other)?;
                inserted.push((base, size));
            }
            {
                let mut locked = device.lock().unwrap();
                locked
                    .intx
                    .lock()
                    .unwrap()
                    .bind(sbdf.device(), Arc::new(AtomicU32::new(0)), true)
                    .map_err(io::Error::other)?;
                locked.bound = true;
                locked.sync_interrupt();
            }
            for (channel, irq) in irqs.iter_mut().enumerate() {
                if let Some(irq) = irq.take() {
                    vm.register_irq(&irq, IRQ + u32::try_from(channel).unwrap())
                        .map_err(|e| io::Error::from_raw_os_error(e.errno()))?;
                    device.lock().unwrap().irqs[channel] = Some(irq);
                }
            }
            Ok(())
        })();
        if let Err(error) = attach {
            for (base, size) in inserted.into_iter().rev() {
                let _ = vm.pio_bus.remove(base, size);
            }
            pci.pci_segment
                .pci_bus
                .lock()
                .unwrap()
                .remove_device(sbdf.device());
            release_allocations();
            return Err(error);
        }
        pci.pci_segment.pci_irq_slots[usize::from(sbdf.device())] =
            u8::try_from(PCI_INTX_GSI).unwrap();
        Ok(device)
    }

    /// Saves media, in-flight transfer and PCI mappings.
    pub fn save(&self) -> OpticalState {
        OpticalState {
            cores: self
                .cores
                .each_ref()
                .map(|core| core.as_ref().map(Atapi::save)),
            config: self.config.state(),
            bars: self.bars,
            mapped: self.mapped,
            modes: self.modes,
            sbdf: self.sbdf,
        }
    }

    /// Ejects idle media subject to the guest's removal lock.
    pub fn eject(&mut self, channel: usize) -> io::Result<()> {
        self.cores
            .get_mut(channel)
            .and_then(Option::as_mut)
            .ok_or_else(|| io::Error::other("optical drive is absent"))?
            .eject()
    }

    /// Replaces one attached drive's medium after verifying its identity.
    pub fn replace_media(&mut self, channel: usize, path: Option<&Path>) -> io::Result<()> {
        self.cores
            .get_mut(channel)
            .and_then(Option::as_mut)
            .ok_or_else(|| io::Error::other("optical drive is absent"))?
            .replace_media(open_media(path)?)
    }

    fn native_pending(&self) -> bool {
        self.cores.iter().enumerate().any(|(channel, core)| {
            self.modes & (1 << (channel * 2)) != 0
                && core.as_ref().is_some_and(Atapi::interrupt_pending)
        })
    }

    fn sync_interrupt(&mut self) {
        let command = self.config.read_reg(1);
        let mut native_enabled = false;
        for (channel, core) in self.cores.iter_mut().enumerate() {
            if let Some(core) = core {
                let native = self.modes & (1 << (channel * 2)) != 0;
                core.set_legacy_irq_enabled(!native && command & 1 != 0);
                native_enabled |= native;
            }
        }
        if !self.bound {
            return;
        }
        let pending = self.native_pending();
        let mut group = self.intx.lock().unwrap();
        let result = (|| {
            group.mask(
                self.sbdf.device(),
                !native_enabled || command & 1 == 0 || command & 0x400 != 0,
            )?;
            if pending {
                group.raise(self.sbdf.device(), 1)
            } else {
                group.clear(self.sbdf.device()).map(|_| ())
            }
        })();
        if let Err(error) = result {
            crate::logger::error!("Optical PCI interrupt: {error}");
        }
    }

    fn relocate(&mut self) {
        let Some(vm) = self.vm.upgrade() else {
            return;
        };
        for (index, size) in BAR_SIZES.into_iter().enumerate() {
            let bar = self.bars.bars[index];
            if bar.encoded_addr == bar.encoded_size | 1 {
                continue;
            }
            let new = u64::from(bar.encoded_addr & !3);
            let old = self.mapped[index];
            if old == new {
                continue;
            }
            let mut allocator = vm.resource_allocator();
            let move_bar = (|| -> io::Result<()> {
                let range = allocator
                    .pci_io
                    .allocate(size, size, AllocPolicy::ExactMatch(new))
                    .map_err(io::Error::other)?;
                if let Err(error) = vm.pio_bus.move_range(old, new, size) {
                    let _ = allocator.pci_io.free(&range);
                    return Err(io::Error::other(error));
                }
                allocator
                    .pci_io
                    .free(&RangeInclusive::new(old, old + size - 1).unwrap())
                    .map_err(io::Error::other)?;
                Ok(())
            })();
            match move_bar {
                Ok(()) => self.mapped[index] = new,
                Err(error) => {
                    crate::logger::warn!("Cannot relocate optical BAR: {error}");
                    self.bars.write(
                        u8::try_from(index).unwrap(),
                        0,
                        &u32::try_from(old).unwrap().to_le_bytes(),
                    );
                }
            }
        }
    }

    fn decode(&self, base: u64, offset: u64) -> Option<(usize, bool, u64)> {
        if self.config.read_reg(1) & 1 == 0 {
            return None;
        }
        for channel in 0..2 {
            let native = self.modes & (1 << (channel * 2)) != 0;
            let command = if native {
                self.mapped[channel * 2]
            } else {
                LEGACY_PORTS[channel * 2].0
            };
            let control = if native {
                self.mapped[channel * 2 + 1]
            } else {
                LEGACY_PORTS[channel * 2 + 1].0
            };
            if base == command {
                return Some((channel, false, offset));
            }
            if base == control && offset == if native { 2 } else { 0 } {
                return Some((channel, true, 0));
            }
        }
        None
    }
}

#[cfg(feature = "windows")]
impl Drop for OpticalDevice {
    fn drop(&mut self) {
        if let Some(vm) = self.vm.upgrade() {
            for (channel, irq) in self.irqs.iter().enumerate() {
                if let Some(irq) = irq {
                    let _ = vm
                        .fd()
                        .unregister_irqfd(irq, IRQ + u32::try_from(channel).unwrap());
                }
            }
        }
        if self.bound {
            let _ = self.intx.lock().unwrap().unbind(self.sbdf.device());
        }
    }
}

#[cfg(feature = "windows")]
impl PciDevice for OpticalDevice {
    fn write_config_register(
        &mut self,
        reg_idx: u16,
        offset: u8,
        data: &[u8],
    ) -> Option<Arc<Barrier>> {
        if (4..10).contains(&reg_idx) {
            self.bars
                .write(u8::try_from(reg_idx - 4).unwrap(), offset, data);
        } else if reg_idx == 2 {
            let mut register = self.read_config_register(2).to_le_bytes();
            let start = usize::from(offset);
            if let Some(target) = register.get_mut(start..start + data.len()) {
                target.copy_from_slice(data);
                self.modes = (register[1] & 5) | 0x0a;
            }
        } else {
            self.config.write_config_register(reg_idx, offset, data);
        }
        if self.config.read_reg(1) & 1 != 0 {
            self.relocate();
        }
        self.sync_interrupt();
        None
    }

    fn read_config_register(&mut self, reg_idx: u16) -> u32 {
        if (4..10).contains(&reg_idx) {
            let mut bytes = [0; 4];
            self.bars
                .read(u8::try_from(reg_idx - 4).unwrap(), 0, &mut bytes);
            u32::from_le_bytes(bytes)
        } else if reg_idx == 2 {
            self.config.read_reg(2) | (u32::from(self.modes) << 8)
        } else {
            let mut value = self.config.read_reg(reg_idx);
            if reg_idx == 1 && self.native_pending() {
                value |= 1 << 19;
            }
            value
        }
    }
}

#[cfg(feature = "windows")]
impl BusDevice for OpticalDevice {
    fn read(&mut self, base: u64, offset: u64, data: &mut [u8]) {
        data.fill(0xff);
        if let Some((channel, control, register)) = self.decode(base, offset) {
            if let Some(core) = &mut self.cores[channel] {
                if !control {
                    core.read(register, data);
                } else if data.len() == 1 {
                    data[0] = core.read_control();
                }
            } else {
                data.fill(0);
            }
        }
        self.sync_interrupt();
    }
    fn write(&mut self, base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        if let Some((channel, control, register)) = self.decode(base, offset)
            && let Some(core) = &mut self.cores[channel]
        {
            if !control {
                core.write(register, data);
            } else if data.len() == 1 {
                core.write_control(data[0]);
            }
        }
        self.sync_interrupt();
        None
    }
}

#[cfg(test)]
#[cfg(feature = "windows")]
mod tests {
    use std::io::{Seek, SeekFrom, Write};

    use vmm_sys_util::tempfile::TempFile;

    use super::*;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    fn iso() -> TempFile {
        let file = TempFile::new().unwrap();
        file.as_file().write_all(&[0; 2048]).unwrap();
        file
    }

    fn platform() -> (Arc<KvmVm>, PciDevices) {
        let mut vm = setup_vm_with_memory(0x1000);
        vm.setup_irqchip(true).unwrap();
        let vm = Arc::new(vm);
        let pci = PciDevices::new(&vm).unwrap();
        (vm, pci)
    }

    #[test]
    fn empty_primary_drive_identifies_and_restores_without_an_iso() {
        use crate::snapshot::Persist;
        use crate::vstate::resources::ResourceAllocator;
        let (source, mut pci) = platform();
        let device = OpticalDevice::attach(&source, &mut pci, &[None], None).unwrap();
        let base = device.lock().unwrap().mapped[0];
        source.pio_bus.write(base + 7, &[0xa1]).unwrap();
        let mut status = [0];
        source.pio_bus.read(base + 7, &mut status).unwrap();
        assert_eq!(status, [0x48]);
        let state = device.lock().unwrap().save();
        let allocation = source.resource_allocator().save();
        let mut target = setup_vm_with_memory(0x1000);
        target.setup_irqchip(true).unwrap();
        let target = Arc::new(target);
        *target.resource_allocator() = ResourceAllocator::restore((), &allocation).unwrap();
        let mut pci = PciDevices::new(&target).unwrap();
        let restored = OpticalDevice::attach(&target, &mut pci, &[None], Some(&state)).unwrap();
        let mut first_word = [0; 2];
        target.pio_bus.read(base, &mut first_word).unwrap();
        assert_eq!(u16::from_le_bytes(first_word), 0x8580);
        drop(restored);
        let file = iso();
        let (wrong, mut pci) = platform();
        *wrong.resource_allocator() = source.resource_allocator().clone();
        OpticalDevice::attach(&wrong, &mut pci, &[Some(file.as_path())], Some(&state)).unwrap_err();
    }

    #[test]
    fn channels_keep_independent_transfer_and_interrupt_state() {
        let (vm, mut pci) = platform();
        let file = iso();
        let device =
            OpticalDevice::attach(&vm, &mut pci, &[Some(file.as_path()), None], None).unwrap();
        vm.set_gsi_routes().unwrap();
        let mapped = device.lock().unwrap().mapped;
        for base in [mapped[0], mapped[2]] {
            vm.pio_bus.write(base + 7, &[0xa1]).unwrap();
        }
        assert_ne!(
            device.lock().unwrap().read_config_register(1) & (1 << 19),
            0
        );
        let mut status = [0];
        vm.pio_bus.read(mapped[0] + 7, &mut status).unwrap();
        assert_ne!(
            device.lock().unwrap().read_config_register(1) & (1 << 19),
            0
        );
        {
            let device = device.lock().unwrap();
            assert!(!device.cores[0].as_ref().unwrap().interrupt_pending());
            assert!(device.cores[1].as_ref().unwrap().interrupt_pending());
        }
        let mut primary = [0; 2];
        let mut secondary = [0; 2];
        vm.pio_bus.read(mapped[0], &mut primary).unwrap();
        vm.pio_bus.read(mapped[2], &mut secondary).unwrap();
        assert_eq!(primary, secondary);
        assert_eq!(u16::from_le_bytes(primary), 0x8580);
        let state = device.lock().unwrap().save();
        assert!(state.matches_drive_count(2));
        assert!(!state.matches_drive_count(1));
        vm.pio_bus.read(mapped[2] + 7, &mut status).unwrap();
        assert!(
            !device.lock().unwrap().cores[1]
                .as_ref()
                .unwrap()
                .interrupt_pending()
        );
        assert_eq!(
            device.lock().unwrap().read_config_register(1) & (1 << 19),
            0
        );
        device.lock().unwrap().write_config_register(2, 1, &[0x0a]);
        for (channel, base) in [COMMAND_PORT, 0x170].into_iter().enumerate() {
            vm.pio_bus.write(base + 7, &[0xa1]).unwrap();
            let mut pic = kvm_bindings::kvm_irqchip {
                chip_id: kvm_bindings::KVM_IRQCHIP_PIC_SLAVE,
                ..Default::default()
            };
            let mut delivered = false;
            for _ in 0..100 {
                vm.fd().get_irqchip(&mut pic).unwrap();
                // SAFETY: KVM returned the slave PIC selected above.
                if unsafe { pic.chip.pic.irr } & (1 << (6 + channel)) != 0 {
                    delivered = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert!(
                delivered,
                "optical channel {channel} did not signal its legacy IRQ"
            );
            vm.pio_bus.read(base + 7, &mut status).unwrap();
            assert_eq!(status, [0x48]);
        }
    }

    #[test]
    fn restore_checks_channel_topology_and_each_medium() {
        use crate::snapshot::Persist;
        use crate::vstate::resources::ResourceAllocator;
        let (vm, mut pci) = platform();
        let file = iso();
        let device =
            OpticalDevice::attach(&vm, &mut pci, &[None, Some(file.as_path())], None).unwrap();
        let state: OpticalState =
            bitcode::deserialize(&bitcode::serialize(&device.lock().unwrap().save()).unwrap())
                .unwrap();
        let allocation = vm.resource_allocator().save();
        let mut target = setup_vm_with_memory(0x1000);
        target.setup_irqchip(true).unwrap();
        let target = Arc::new(target);
        *target.resource_allocator() = ResourceAllocator::restore((), &allocation).unwrap();
        let mut pci = PciDevices::new(&target).unwrap();
        OpticalDevice::attach(&target, &mut pci, &[None], Some(&state)).unwrap_err();
        OpticalDevice::attach(
            &target,
            &mut pci,
            &[Some(file.as_path()), None],
            Some(&state),
        )
        .unwrap_err();
        let restored = OpticalDevice::attach(
            &target,
            &mut pci,
            &[None, Some(file.as_path())],
            Some(&state),
        )
        .unwrap();
        assert!(restored.lock().unwrap().save().matches_drive_count(2));
    }

    #[test]
    fn identity_distinguishes_equal_size_media() {
        let file = iso();
        let mut handle = file.as_file().try_clone().unwrap();
        handle.seek(SeekFrom::Start(0)).unwrap();
        let first = media_identity(&mut handle).unwrap();
        handle.seek(SeekFrom::Start(0)).unwrap();
        handle.write_all(&[1; 2048]).unwrap();
        handle.seek(SeekFrom::Start(0)).unwrap();
        assert_ne!(first, media_identity(&mut handle).unwrap());
    }

    #[test]
    fn native_mapping_switches_to_compatibility_and_obeys_io_enable() {
        let (vm, mut pci) = platform();
        let file = iso();
        let device = OpticalDevice::attach(&vm, &mut pci, &[Some(file.as_path())], None).unwrap();
        let mapped = device.lock().unwrap().mapped;
        assert_eq!(device.lock().unwrap().read_config_register(2), 0x0101_0f00);
        vm.pio_bus.write(mapped[0] + 7, &[0xa1]).unwrap();
        assert!(
            device.lock().unwrap().cores[0]
                .as_ref()
                .unwrap()
                .interrupt_pending()
        );
        let mut status = [0];
        vm.pio_bus.read(mapped[1] + 2, &mut status).unwrap();
        assert_eq!(status, [0x48]);
        assert!(
            device.lock().unwrap().cores[0]
                .as_ref()
                .unwrap()
                .interrupt_pending()
        );
        vm.pio_bus.read(mapped[0] + 7, &mut status).unwrap();
        assert!(
            !device.lock().unwrap().cores[0]
                .as_ref()
                .unwrap()
                .interrupt_pending()
        );
        vm.pio_bus.read(COMMAND_PORT + 7, &mut status).unwrap();
        assert_eq!(status, [0xff]);
        device.lock().unwrap().write_config_register(2, 1, &[0x0a]);
        vm.pio_bus.read(COMMAND_PORT + 7, &mut status).unwrap();
        assert_eq!(status, [0x48]);
        vm.pio_bus.read(mapped[0] + 7, &mut status).unwrap();
        assert_eq!(status, [0xff]);
        device
            .lock()
            .unwrap()
            .write_config_register(1, 0, &0x14u16.to_le_bytes());
        vm.pio_bus.read(COMMAND_PORT + 7, &mut status).unwrap();
        assert_eq!(status, [0xff]);
    }

    #[test]
    fn bar_probe_relocation_and_collision_preserve_live_ports() {
        let (vm, mut pci) = platform();
        let file = iso();
        let device = OpticalDevice::attach(&vm, &mut pci, &[Some(file.as_path())], None).unwrap();
        let old = device.lock().unwrap().mapped[0];
        device
            .lock()
            .unwrap()
            .write_config_register(4, 0, &u32::MAX.to_le_bytes());
        assert_eq!(device.lock().unwrap().read_config_register(4), 0xffff_fff9);
        let mut status = [0];
        vm.pio_bus.read(old + 7, &mut status).unwrap();
        assert_eq!(status, [0x40]);
        device
            .lock()
            .unwrap()
            .write_config_register(4, 0, &0x3001u32.to_le_bytes());
        vm.pio_bus.read(old + 7, &mut status).unwrap_err();
        vm.pio_bus.read(0x3007, &mut status).unwrap();
        assert_eq!(status, [0x40]);
        let collision = device.lock().unwrap().mapped[2];
        device.lock().unwrap().write_config_register(
            4,
            0,
            &u32::try_from(collision | 1).unwrap().to_le_bytes(),
        );
        assert_eq!(device.lock().unwrap().read_config_register(4), 0x3001);
        vm.pio_bus.read(0x3007, &mut status).unwrap();
    }

    #[test]
    fn failed_attachment_preserves_existing_ports_and_allocations() {
        #[derive(Debug)]
        struct Occupied;
        impl BusDevice for Occupied {}
        for (port, size) in LEGACY_PORTS {
            let (vm, mut pci) = platform();
            let occupied = Arc::new(Mutex::new(Occupied));
            vm.pio_bus.insert(occupied.clone(), port, size).unwrap();
            let before = vm.resource_allocator().pci_io.clone();
            let file = iso();
            OpticalDevice::attach(&vm, &mut pci, &[Some(file.as_path())], None).unwrap_err();
            assert_eq!(vm.resource_allocator().pci_io, before);
            assert!(
                pci.pci_segment
                    .pci_bus
                    .lock()
                    .unwrap()
                    .get_device(1)
                    .is_none()
            );
            vm.pio_bus.read(port, &mut [0]).unwrap();
        }
    }

    #[test]
    fn snapshots_reject_forged_config_bar_shape_and_mapping() {
        let (vm, mut pci) = platform();
        let file = iso();
        let device = OpticalDevice::attach(&vm, &mut pci, &[Some(file.as_path())], None).unwrap();
        let state = device.lock().unwrap().save();
        validate_mappings(&vm.resource_allocator().pci_io, &state).unwrap();
        let mut bad = state.clone();
        bad.mapped[0] += 8;
        validate_mappings(&vm.resource_allocator().pci_io, &bad).unwrap_err();
        let mut bad = state.clone();
        bad.bars.bars[0].encoded_size = 0;
        make_bars(bad.mapped, Some(&bad.bars)).unwrap_err();
        let mut encoded = serde_json::to_value(&state.config).unwrap();
        encoded["writable_bits"][0] = u32::MAX.into();
        let config = serde_json::from_value(encoded.clone()).unwrap();
        let mut restored = restore_configuration(&config).unwrap();
        restored.write_reg(0, 0);
        assert_eq!(restored.read_reg(0), 0xfc01_1d0f);
        encoded["registers"][11] = 0u32.into();
        restore_configuration(&serde_json::from_value(encoded).unwrap()).unwrap_err();
    }

    #[test]
    fn restores_native_mapping_and_pending_transfer_on_fresh_vm() {
        use crate::snapshot::Persist;
        use crate::vstate::resources::ResourceAllocator;

        let (vm, mut pci) = platform();
        let file = iso();
        let device = OpticalDevice::attach(&vm, &mut pci, &[Some(file.as_path())], None).unwrap();
        let mapped = device.lock().unwrap().mapped;
        vm.pio_bus.write(mapped[0] + 7, &[0xa1]).unwrap();
        let mut first_word = [0; 2];
        vm.pio_bus.read(mapped[0], &mut first_word).unwrap();
        let state = device.lock().unwrap().save();
        let resources = vm.resource_allocator().save();
        let mut expected = [0; 16];
        vm.pio_bus.read(mapped[0], &mut expected).unwrap();

        let mut restored_vm = setup_vm_with_memory(0x1000);
        restored_vm.setup_irqchip(true).unwrap();
        let restored_vm = Arc::new(restored_vm);
        *restored_vm.resource_allocator() = ResourceAllocator::restore((), &resources).unwrap();
        let mut restored_pci = PciDevices::new(&restored_vm).unwrap();
        let restored = OpticalDevice::attach(
            &restored_vm,
            &mut restored_pci,
            &[Some(file.as_path())],
            Some(&state),
        )
        .unwrap();
        assert_eq!(restored.lock().unwrap().mapped, mapped);
        assert!(
            restored.lock().unwrap().cores[0]
                .as_ref()
                .unwrap()
                .interrupt_pending()
        );
        let mut actual = [0; 16];
        restored_vm.pio_bus.read(mapped[0], &mut actual).unwrap();
        assert_eq!(actual, expected);
        restored_vm.pio_bus.read(mapped[0] + 7, &mut [0]).unwrap();
        assert!(
            !restored.lock().unwrap().cores[0]
                .as_ref()
                .unwrap()
                .interrupt_pending()
        );
        assert_eq!(
            restored_pci.pci_segment.pci_irq_slots[usize::from(state.sbdf.device())],
            16
        );
    }

    #[test]
    fn optical_device_does_not_keep_vm_alive() {
        let (vm, mut pci) = platform();
        let weak = Arc::downgrade(&vm);
        let file = iso();
        let device = OpticalDevice::attach(&vm, &mut pci, &[Some(file.as_path())], None).unwrap();
        drop(vm);
        assert!(weak.upgrade().is_none());
        assert_eq!(device.lock().unwrap().read_config_register(0), 0xfc01_1d0f);
    }
}
