// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! VGA BAR mappings shared by the legacy and virtio PCI displays.

pub(crate) mod memory;

use std::io;
use std::sync::{Arc, Mutex, Weak};

use vm_allocator::{AddressAllocator, AllocPolicy, RangeInclusive};

use super::{FRAMEBUFFER_SIZE, REGISTER_SIZE, Vga};
#[cfg(target_arch = "x86_64")]
use crate::pci::configuration::BarPrefetchable;
use crate::pci::configuration::Bars;
use crate::vstate::vm::KvmVm;
use memory::FramebufferMemory;

const MAPPINGS: usize = 2;
const BAR_INDICES: [usize; MAPPINGS] = [0, 2];
pub(crate) const BAR_SIZES: [u64; MAPPINGS] = [FRAMEBUFFER_SIZE as u64, REGISTER_SIZE];

fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

pub(crate) fn allocate_mappings<const N: usize>(
    allocator: &mut AddressAllocator,
    sizes: [u64; N],
) -> io::Result<[u64; N]> {
    let mut next = allocator.clone();
    let mut mapped = [0; N];
    for (base, size) in mapped.iter_mut().zip(sizes) {
        *base = next
            .allocate(size, size, AllocPolicy::FirstMatch)
            .map_err(io_error)?
            .start();
    }
    validate_mappings(&next, mapped.into_iter().zip(sizes))?;
    *allocator = next;
    Ok(mapped)
}

pub(crate) fn validate_mappings(
    allocator: &AddressAllocator,
    regions: impl Iterator<Item = (u64, u64)>,
) -> io::Result<()> {
    let start = crate::arch::MEM_32BIT_DEVICES_START;
    let end = start + crate::arch::MEM_32BIT_DEVICES_SIZE - 1;
    if allocator.base() != start || allocator.end() != end {
        return Err(io_error("invalid display allocator window"));
    }
    let mut reservations = allocator.clone();
    for (base, size) in regions {
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

#[derive(Debug)]
pub(crate) struct VgaPci {
    framebuffer: FramebufferMemory,
    vm: Weak<KvmVm>,
    pub(crate) mapped: [u64; MAPPINGS],
    pub(crate) vga: Arc<Mutex<Vga>>,
}

impl VgaPci {
    pub(crate) fn new(vm: &Arc<KvmVm>, vga: Vga, mapped: [u64; MAPPINGS]) -> io::Result<Self> {
        let framebuffer = FramebufferMemory::new(vm, vga.framebuffer_mapping())?;
        Ok(Self {
            framebuffer,
            vm: Arc::downgrade(vm),
            mapped,
            vga: Arc::new(Mutex::new(vga)),
        })
    }

    #[cfg(target_arch = "x86_64")]
    pub(crate) fn allocate_bars(
        &mut self,
        allocator: &mut AddressAllocator,
        bars: &mut Bars,
    ) -> io::Result<()> {
        self.mapped = allocate_mappings(allocator, BAR_SIZES)?;
        for (slot, (bar, size)) in BAR_INDICES.into_iter().zip(BAR_SIZES).enumerate() {
            bars.set_bar_32(
                u8::try_from(bar).unwrap(),
                u32::try_from(self.mapped[slot]).unwrap(),
                u32::try_from(size).unwrap(),
                if slot == 0 {
                    BarPrefetchable::Yes
                } else {
                    BarPrefetchable::No
                },
            );
        }
        Ok(())
    }

    #[cfg(target_arch = "x86_64")]
    pub(crate) fn save(&self) -> super::VgaPciState {
        super::VgaPciState {
            mapped: self.mapped,
            vga: self.vga.lock().unwrap().save(),
        }
    }

    #[cfg(target_arch = "x86_64")]
    pub(crate) fn restore(
        vm: &Arc<KvmVm>,
        state: &super::VgaPciState,
        bars: &Bars,
        command: u32,
    ) -> io::Result<Self> {
        validate_mappings(
            &vm.resource_allocator().mmio32_memory,
            state.mapped.into_iter().zip(BAR_SIZES),
        )?;
        for (slot, (bar, size)) in BAR_INDICES.into_iter().zip(BAR_SIZES).enumerate() {
            let mask = !(u32::try_from(size).unwrap() - 1);
            let saved = &bars.bars[bar];
            let flags = if slot == 0 { 8 } else { 0 };
            let address = u64::from(saved.encoded_addr & mask);
            if saved.encoded_size != mask
                || saved.encoded_addr & !mask != flags
                || (command & 2 != 0 && address != state.mapped[slot] && address != u64::from(mask))
            {
                return Err(io_error("invalid VGA BAR state"));
            }
        }
        Self::new(vm, Vga::restore(&state.vga)?, state.mapped)
    }

    #[cfg(target_arch = "x86_64")]
    pub(crate) fn mmio_regions(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.mapped
            .into_iter()
            .zip(BAR_SIZES)
            .chain(std::iter::once((0xa0000, 0x20000)))
    }

    pub(crate) fn set_decode(&mut self, enabled: bool) -> io::Result<()> {
        self.framebuffer
            .set_address(enabled.then_some(self.mapped[0]))
    }

    pub(crate) fn relocate(&mut self, bars: &mut Bars, command: u32) {
        if command & 2 == 0 {
            return;
        }
        let Some(vm) = self.vm.upgrade() else {
            return;
        };
        for (slot, (bar, size)) in BAR_INDICES.into_iter().zip(BAR_SIZES).enumerate() {
            let new = bars.get_bar_addr(u8::try_from(bar).unwrap());
            let old = self.mapped[slot];
            if new == old || new == (0xffff_ffff & !(size - 1)) {
                continue;
            }
            let mut allocator = vm.resource_allocator();
            let moved = allocator
                .mmio32_memory
                .allocate(size, size, AllocPolicy::ExactMatch(new))
                .is_ok_and(|range| {
                    if vm.common.mmio_bus.move_range(old, new, size).is_err() {
                        let _ = allocator.mmio32_memory.free(&range);
                        return false;
                    }
                    if slot == 0 && self.framebuffer.set_address(Some(new)).is_err() {
                        vm.common.mmio_bus.move_range(new, old, size).unwrap();
                        let _ = allocator.mmio32_memory.free(&range);
                        return false;
                    }
                    let _ = allocator
                        .mmio32_memory
                        .free(&RangeInclusive::new(old, old + size - 1).unwrap());
                    true
                });
            if moved {
                self.mapped[slot] = new;
            } else {
                let flags = bars.bars[bar].encoded_addr & 0xf;
                bars.bars[bar].encoded_addr = u32::try_from(old).unwrap() | flags;
            }
        }
    }

    pub(crate) fn read(&self, base: u64, offset: u64, command: u32, bytes: &mut [u8]) {
        let mut vga = self.vga.lock().unwrap();
        #[cfg(target_arch = "x86_64")]
        if let Some(port) = pio_port(base, offset, command) {
            vga.read_port(port, bytes);
            return;
        }
        if command & 2 == 0 {
            return;
        }
        match base {
            #[cfg(target_arch = "x86_64")]
            0xa0000 => vga.read_legacy(offset, bytes),
            b if b == self.mapped[0] => vga.read_framebuffer(offset, bytes),
            b if b == self.mapped[1] => vga.read_registers(offset, bytes),
            _ => {}
        }
    }

    pub(crate) fn write(&self, base: u64, offset: u64, command: u32, bytes: &[u8]) {
        let mut vga = self.vga.lock().unwrap();
        #[cfg(target_arch = "x86_64")]
        if let Some(port) = pio_port(base, offset, command) {
            vga.write_port(port, bytes);
            return;
        }
        if command & 2 == 0 {
            return;
        }
        match base {
            #[cfg(target_arch = "x86_64")]
            0xa0000 => vga.write_legacy(offset, bytes),
            b if b == self.mapped[0] => vga.write_framebuffer(offset, bytes),
            b if b == self.mapped[1] => vga.write_registers(offset, bytes),
            _ => {}
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn pio_port(base: u64, offset: u64, command: u32) -> Option<u16> {
    if command & 1 == 0 || !matches!(base, 0x3b0 | 0x1ce) {
        return None;
    }
    u16::try_from(base.checked_add(offset)?).ok()
}
