// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::sync::{Arc, Barrier, Mutex};

use vm_memory::{Bytes, GuestAddress, GuestMemoryBackend, GuestMemoryRegion};

use super::{MMIO_BASE, MMIO_SIZE, RamfbState};
use crate::devices::display::Frame;
use crate::vstate::bus::BusDevice;
use crate::vstate::memory::GuestMemoryMmap;
use crate::vstate::vm::KvmVm;

const RAMFB: u16 = 0x20;
const MAX_TRANSFER: usize = 1024 * 1024;
const DIRECTORY: [u8; 68] = {
    let mut bytes = [0; 68];
    bytes[3] = 1;
    bytes[7] = 28;
    bytes[8] = RAMFB.to_be_bytes()[0];
    bytes[9] = RAMFB.to_be_bytes()[1];
    let name = b"etc/ramfb";
    let mut index = 0;
    while index < name.len() {
        bytes[12 + index] = name[index];
        index += 1;
    }
    bytes
};

#[derive(Debug)]
pub struct Ramfb {
    memory: GuestMemoryMmap,
    state: RamfbState,
}

impl Ramfb {
    pub(crate) fn new(memory: GuestMemoryMmap, state: RamfbState) -> Self {
        Self { memory, state }
    }

    pub(crate) fn attach(
        vm: &Arc<KvmVm>,
        state: Option<&RamfbState>,
    ) -> io::Result<Arc<Mutex<Self>>> {
        for region in vm.guest_memory().iter() {
            Self::validate_mapping(region.start_addr().0, region.len())?;
        }
        let device = Arc::new(Mutex::new(Self::new(
            vm.guest_memory().clone(),
            state.cloned().unwrap_or_default(),
        )));
        vm.common
            .mmio_bus
            .insert(device.clone(), MMIO_BASE, MMIO_SIZE)
            .map_err(io::Error::other)?;
        Ok(device)
    }

    pub(crate) fn save(&self) -> RamfbState {
        self.state.clone()
    }

    pub(crate) fn validate_mapping(base: u64, length: u64) -> io::Result<()> {
        let end = base
            .checked_add(length)
            .filter(|_| length != 0)
            .ok_or_else(|| io::Error::other("invalid RAM framebuffer mapping range"))?;
        if base < MMIO_BASE + MMIO_SIZE && end > MMIO_BASE {
            return Err(io::Error::other(
                "mapping overlaps RAM framebuffer configuration",
            ));
        }
        Ok(())
    }

    fn data(&self) -> &[u8] {
        match self.state.selector & !0x4000 {
            0 => b"QEMU",
            1 => &[3, 0, 0, 0],
            0x19 => &DIRECTORY,
            RAMFB => &self.state.config,
            _ => &[],
        }
    }

    fn select(&mut self, selector: u16) {
        self.state.selector = selector;
        self.state.offset = 0;
    }

    fn read_data(&mut self, bytes: &mut [u8]) {
        let data = self.data();
        let offset = usize::try_from(self.state.offset).unwrap();
        bytes.fill(0);
        if let Some(data) = data.get(offset..) {
            let length = data.len().min(bytes.len());
            bytes[..length].copy_from_slice(&data[..length]);
        }
        self.state.offset = self
            .state
            .offset
            .saturating_add(u32::try_from(bytes.len()).unwrap());
    }

    fn transfer(&mut self, control: u32, length: usize, address: GuestAddress) -> io::Result<()> {
        if control & 8 != 0 {
            self.select((control >> 16) as u16);
        }
        if control & 0xffff & !0x1e != 0 {
            return Err(io::Error::other("invalid fw_cfg DMA control"));
        }
        if control & (2 | 16) != 0 {
            if length > MAX_TRANSFER || !self.memory.check_range(address, length) {
                return Err(io::Error::other("invalid fw_cfg DMA range"));
            }
            if control & 2 != 0 {
                let mut bytes = vec![0; length];
                self.read_data(&mut bytes);
                self.memory
                    .write_slice(&bytes, address)
                    .map_err(io::Error::other)?;
            } else {
                let offset = usize::try_from(self.state.offset).unwrap();
                let end = offset
                    .checked_add(length)
                    .ok_or_else(|| io::Error::other("fw_cfg overflow"))?;
                if self.state.selector & !0x4000 != RAMFB || end > self.state.config.len() {
                    return Err(io::Error::other("invalid fw_cfg write"));
                }
                let mut config = self.state.config;
                self.memory
                    .read_slice(&mut config[offset..end], address)
                    .map_err(io::Error::other)?;
                self.state.config = config;
                self.state.offset = u32::try_from(end).unwrap();
            }
        } else if control & 4 != 0 {
            self.state.offset = self
                .state
                .offset
                .saturating_add(u32::try_from(length).unwrap());
        }
        Ok(())
    }

    fn dma(&mut self, address: u64) {
        let address = GuestAddress(address);
        let mut descriptor = [0; 16];
        if self.memory.read_slice(&mut descriptor, address).is_err() {
            return;
        }
        let control = u32::from_be_bytes(descriptor[..4].try_into().unwrap());
        let length = u32::from_be_bytes(descriptor[4..8].try_into().unwrap()) as usize;
        let buffer = GuestAddress(u64::from_be_bytes(descriptor[8..].try_into().unwrap()));
        let status = u32::from(self.transfer(control, length, buffer).is_err());
        let _ = self.memory.write_slice(&status.to_be_bytes(), address);
    }

    fn layout(&self) -> Option<(GuestAddress, u16, u16, usize)> {
        let config = &self.state.config;
        let address = GuestAddress(u64::from_be_bytes(config[..8].try_into().unwrap()));
        let word = |offset| u32::from_be_bytes(config[offset..offset + 4].try_into().unwrap());
        let (width, height, stride) = (word(16), word(20), word(24));
        let stride = if stride == 0 {
            width.checked_mul(4)?
        } else {
            stride
        };
        if word(8) != 0x3432_5258
            || word(12) != 0
            || !(16..=2048).contains(&width)
            || !(16..=2048).contains(&height)
            || stride < width * 4
        {
            return None;
        }
        let length = stride.checked_mul(height - 1)?.checked_add(width * 4)? as usize;
        if length > crate::devices::display::FRAMEBUFFER_SIZE
            || !self.memory.check_range(address, length)
        {
            return None;
        }
        Some((
            address,
            u16::try_from(width).unwrap(),
            u16::try_from(height).unwrap(),
            stride as usize,
        ))
    }

    pub(crate) fn render(&self) -> Frame {
        let Some((address, width, height, stride)) = self.layout() else {
            // Direct Linux boot has no GOP; RFB still requires a nonempty display.
            return Frame {
                width: 800,
                height: 600,
                pixels: vec![0; 800 * 600],
            };
        };
        let mut frame = Frame {
            width,
            height,
            pixels: Vec::with_capacity(usize::from(width) * usize::from(height)),
        };
        let mut row = vec![0; usize::from(width) * 4];
        for y in 0..usize::from(height) {
            self.memory
                .read_slice(&mut row, GuestAddress(address.0 + (y * stride) as u64))
                .unwrap();
            frame.pixels.extend(
                row.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|pixel| u32::from_le_bytes(*pixel) & 0x00ff_ffff),
            );
        }
        frame
    }
}

impl BusDevice for Ramfb {
    fn read(&mut self, _base: u64, offset: u64, bytes: &mut [u8]) {
        bytes.fill(0);
        match (offset, bytes.len()) {
            (0, 1 | 2 | 4 | 8) => self.read_data(bytes),
            (16, 4 | 8) => bytes.copy_from_slice(&b"QEMU CFG"[..bytes.len()]),
            (20, 4) => bytes.copy_from_slice(&b"QEMU CFG"[4..]),
            _ => {}
        }
    }

    fn write(&mut self, _base: u64, offset: u64, bytes: &[u8]) -> Option<Arc<Barrier>> {
        let address = match (offset, bytes.len()) {
            (8, 2) => {
                self.select(u16::from_be_bytes(bytes.try_into().unwrap()));
                return None;
            }
            (16, 4) => {
                self.state.dma_high = u32::from_be_bytes(bytes.try_into().unwrap());
                return None;
            }
            (16, 8) => u64::from_be_bytes(bytes.try_into().unwrap()),
            (20, 4) => {
                u64::from(self.state.dma_high) << 32
                    | u64::from(u32::from_be_bytes(bytes.try_into().unwrap()))
            }
            _ => return None,
        };
        self.state.dma_high = 0;
        self.dma(address);
        None
    }
}

#[cfg(test)]
mod tests;
