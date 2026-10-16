// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! VGA planar memory and the PCI Bochs DISPI MMIO display interface.
//!
//! Register interfaces: QEMU `specs/standard-vga.html` and Bochs VGABIOS
//! `vbe_display_api.txt`. Fonts are uploaded by guest firmware into VGA plane 2.

#[cfg(feature = "windows")]
use std::time::Instant;

#[cfg(feature = "windows")]
mod memory;
#[cfg(feature = "windows")]
use memory::VideoMemory;

#[cfg(feature = "windows")]
use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod state;
pub use state::VgaState;

/// Framebuffer BAR size, in bytes.
pub const FRAMEBUFFER_SIZE: usize = 16 * 1024 * 1024;
/// Register BAR size, in bytes.
pub const REGISTER_SIZE: u64 = 4096;
const MAX_DIMENSION: u16 = 2048;
const ENABLED: u16 = 1;
#[cfg(feature = "windows")]
const GET_CAPS: u16 = 2;
#[cfg(feature = "windows")]
const DAC_8BIT: u16 = 0x20;
#[cfg(feature = "windows")]
const NO_CLEAR: u16 = 0x80;
#[cfg(feature = "windows")]
const SEQUENCER_MASKS: [u8; 8] = [0x03, 0x3d, 0x0f, 0x3f, 0x0e, 0, 0, 0];
#[cfg(feature = "windows")]
const GRAPHICS_MASKS: [u8; 16] = [
    0x0f, 0x0f, 0x0f, 0x1f, 0x03, 0x7b, 0x0f, 0x0f, 0xff, 0, 0, 0, 0, 0, 0, 0,
];

/// A bounded display image in row-major `0x00RRGGBB` pixels.
#[derive(Debug, PartialEq, Eq)]
#[cfg(feature = "windows")]
pub struct Frame {
    /// Horizontal pixel count.
    pub width: u16,
    /// Vertical pixel count.
    pub height: u16,
    /// One packed RGB value per pixel.
    pub pixels: Vec<u32>,
}

/// Display registers, latches, palette and video memory.
#[derive(Clone, Debug)]
#[cfg(feature = "windows")]
pub struct Vga {
    memory: VideoMemory,
    sequencer: [u8; 8],
    graphics: [u8; 16],
    attribute: [u8; 32],
    crtc: [u8; 32],
    palette: Vec<[u8; 3]>,
    seq_index: u8,
    gfx_index: u8,
    attr_index: u8,
    crtc_index: u8,
    attr_data: bool,
    misc: u8,
    feature: u8,
    dac_mask: u8,
    dac_read: u8,
    dac_write: u8,
    dac_component: u8,
    dac_state: u8,
    latch: [u8; 4],
    vbe_index: u16,
    vbe: [u16; 11],
    epoch: Option<Instant>,
}

#[cfg(feature = "windows")]
impl Vga {
    /// Copies checkpoint bytes without retaining the live VRAM mapping.
    pub fn save(&self) -> VgaState {
        VgaState {
            memory: self.memory.bytes(),
            sequencer: self.sequencer,
            graphics: self.graphics,
            attribute: self.attribute,
            crtc: self.crtc,
            palette: self.palette.clone(),
            seq_index: self.seq_index,
            gfx_index: self.gfx_index,
            attr_index: self.attr_index,
            crtc_index: self.crtc_index,
            attr_data: self.attr_data,
            misc: self.misc,
            feature: self.feature,
            dac_mask: self.dac_mask,
            dac_read: self.dac_read,
            dac_write: self.dac_write,
            dac_component: self.dac_component,
            dac_state: self.dac_state,
            latch: self.latch,
            vbe_index: self.vbe_index,
            vbe: self.vbe,
        }
    }

    /// Validates checkpoint data before allocating the live display mapping.
    pub fn restore(state: &VgaState) -> std::io::Result<Self> {
        if !state.validate() {
            return Err(std::io::Error::other("invalid display state"));
        }
        Ok(Self {
            memory: VideoMemory::from_bytes(&state.memory).map_err(std::io::Error::other)?,
            sequencer: state.sequencer,
            graphics: state.graphics,
            attribute: state.attribute,
            crtc: state.crtc,
            palette: state.palette.clone(),
            seq_index: state.seq_index,
            gfx_index: state.gfx_index,
            attr_index: state.attr_index,
            crtc_index: state.crtc_index,
            attr_data: state.attr_data,
            misc: state.misc,
            feature: state.feature,
            dac_mask: state.dac_mask,
            dac_read: state.dac_read,
            dac_write: state.dac_write,
            dac_component: state.dac_component,
            dac_state: state.dac_state,
            latch: state.latch,
            vbe_index: state.vbe_index,
            vbe: state.vbe,
            epoch: None,
        })
    }
}

#[cfg(feature = "windows")]
impl Serialize for Vga {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.save().serialize(serializer)
    }
}

#[cfg(feature = "windows")]
impl<'de> Deserialize<'de> for Vga {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let state = VgaState::deserialize(deserializer)?;
        Self::restore(&state).map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "windows")]
impl Default for Vga {
    fn default() -> Self {
        let mut vbe = [0; 11];
        vbe[0] = 0xb0c5;
        vbe[10] = u16::try_from(FRAMEBUFFER_SIZE / 65536).unwrap();
        Self {
            memory: VideoMemory::default(),
            sequencer: [0; 8],
            graphics: [0; 16],
            attribute: [0; 32],
            crtc: [0; 32],
            palette: vec![[0; 3]; 256],
            seq_index: 0,
            gfx_index: 0,
            attr_index: 0,
            crtc_index: 0,
            attr_data: false,
            misc: 1,
            feature: 0,
            dac_mask: 0xff,
            dac_read: 0,
            dac_write: 0,
            dac_component: 0,
            dac_state: 0,
            latch: [0; 4],
            vbe_index: 0,
            vbe,
            epoch: None,
        }
    }
}

#[cfg(feature = "windows")]
impl Vga {
    /// Stable, page-aligned VRAM backing for the linear framebuffer KVM slot.
    pub(crate) fn framebuffer_mapping(&self) -> std::sync::Arc<vm_memory::MmapRegion> {
        self.memory.mapping()
    }

    /// Reads a VGA or DISPI I/O port. Unsupported accesses return all ones.
    pub fn read_port(&mut self, port: u16, data: &mut [u8]) {
        data.fill(0xff);
        if data.len() == 2 && matches!(port, 0x1ce..=0x1d0) {
            let value = if port == 0x1ce {
                self.vbe_index
            } else {
                self.read_vbe(self.vbe_index)
            };
            data.copy_from_slice(&value.to_le_bytes());
        } else if data.len() <= 2 {
            for (index, byte) in data.iter_mut().enumerate() {
                *byte = self.read_vga(port.wrapping_add(u16::try_from(index).unwrap()));
            }
        }
    }

    /// Writes a VGA or DISPI I/O port.
    pub fn write_port(&mut self, port: u16, data: &[u8]) {
        if let [lo, hi] = data
            && matches!(port, 0x1ce..=0x1d0)
        {
            let value = u16::from_le_bytes([*lo, *hi]);
            if port == 0x1ce {
                self.vbe_index = value;
            } else {
                self.write_vbe(self.vbe_index, value);
            }
        } else if data.len() <= 2 {
            for (index, byte) in data.iter().enumerate() {
                self.write_vga(port.wrapping_add(u16::try_from(index).unwrap()), *byte);
            }
        }
    }

    /// Reads the flat VGA/DISPI register BAR.
    pub fn read_registers(&mut self, offset: u64, data: &mut [u8]) {
        data.fill(0xff);
        match (offset, data.len()) {
            (0x400..=0x41f, 1 | 2) if offset + data.len() as u64 <= 0x420 => {
                self.read_port(0x3c0 + u16::try_from(offset - 0x400).unwrap(), data);
            }
            (0x500..=0x514, 2) if offset & 1 == 0 => {
                data.copy_from_slice(
                    &self
                        .read_vbe(u16::try_from((offset - 0x500) / 2).unwrap())
                        .to_le_bytes(),
                );
            }
            // No EDID extension is advertised. Firmware uses its built-in mode list.
            (0..=0x3ff, _) if offset + data.len() as u64 <= 0x400 => data.fill(0),
            _ => {}
        }
    }

    /// Writes the flat VGA/DISPI register BAR.
    pub fn write_registers(&mut self, offset: u64, data: &[u8]) {
        match (offset, data) {
            (0x400..=0x41f, _) if data.len() <= 2 && offset + data.len() as u64 <= 0x420 => {
                self.write_port(0x3c0 + u16::try_from(offset - 0x400).unwrap(), data);
            }
            (0x500..=0x514, [lo, hi]) if offset & 1 == 0 => {
                self.write_vbe(
                    u16::try_from((offset - 0x500) / 2).unwrap(),
                    u16::from_le_bytes([*lo, *hi]),
                );
            }
            _ => {}
        }
    }

    /// Reads linear video memory. Out-of-range accesses return all ones.
    pub fn read_framebuffer(&self, offset: u64, data: &mut [u8]) {
        data.fill(0xff);
        if let Some(source) = bounded_range(offset, data.len(), self.memory.len()) {
            self.memory.read(source.start, data);
        }
    }

    /// Writes linear video memory. Out-of-range accesses are ignored.
    pub fn write_framebuffer(&mut self, offset: u64, data: &[u8]) {
        if let Some(target) = bounded_range(offset, data.len(), self.memory.len()) {
            self.memory.write(target.start, data);
        }
    }

    fn read_vbe(&self, index: u16) -> u16 {
        if self.vbe[4] & GET_CAPS != 0 {
            match index {
                1 | 2 => return MAX_DIMENSION,
                3 => return 32,
                _ => {}
            }
        }
        self.vbe.get(usize::from(index)).copied().unwrap_or(0)
    }

    fn write_vbe(&mut self, index: u16, value: u16) {
        let previous = self.vbe;
        match index {
            0 if (0xb0c0..=0xb0c5).contains(&value) => self.vbe[0] = value,
            1 | 2 if self.vbe[4] & ENABLED == 0 && value <= MAX_DIMENSION => {
                self.vbe[usize::from(index)] = value
            }
            3 if self.vbe[4] & ENABLED == 0 && matches!(value, 0 | 8 | 15 | 16 | 24 | 32) => {
                self.vbe[3] = value.max(8)
            }
            4 => {
                self.vbe[4] = value & 0xe3;
                if value & ENABLED != 0 && previous[4] & ENABLED == 0 {
                    self.vbe[6] = self.vbe[1];
                    self.vbe[8] = 0;
                    self.vbe[9] = 0;
                    self.update_virtual_height();
                    if self.vbe_layout().is_none() {
                        self.vbe = previous;
                        return;
                    }
                    if value & NO_CLEAR == 0 {
                        self.memory.clear();
                    }
                }
            }
            5 if usize::from(value) < FRAMEBUFFER_SIZE / 65536 => self.vbe[5] = value,
            6 if value >= self.vbe[1] && value <= MAX_DIMENSION => {
                self.vbe[6] = value;
                self.update_virtual_height();
            }
            8 | 9 => self.vbe[usize::from(index)] = value,
            _ => {}
        }
        if self.vbe[4] & ENABLED != 0 && self.vbe_layout().is_none() {
            self.vbe = previous;
        }
    }

    fn update_virtual_height(&mut self) {
        let stride = usize::from(self.vbe[6]) * usize::from(self.vbe[3].div_ceil(8));
        self.vbe[7] =
            u16::try_from(FRAMEBUFFER_SIZE.checked_div(stride).unwrap_or(0)).unwrap_or(u16::MAX);
    }

    fn vbe_layout(&self) -> Option<(usize, usize, usize)> {
        state::vbe_layout(self.vbe, self.memory.len())
    }

    fn read_vga(&mut self, port: u16) -> u8 {
        match port {
            0x3c0 => self.attr_index,
            0x3c1 => self.attribute[usize::from(self.attr_index & 31)],
            0x3c2 => 0x10,
            0x3c4 => self.seq_index,
            0x3c5 => {
                let index = usize::from(self.seq_index);
                self.sequencer[index] & SEQUENCER_MASKS[index]
            }
            0x3c6 => self.dac_mask,
            0x3c7 => self.dac_state,
            0x3c8 => self.dac_write,
            0x3c9 => {
                let value =
                    self.palette[usize::from(self.dac_read)][usize::from(self.dac_component)];
                self.dac_component += 1;
                if self.dac_component == 3 {
                    self.dac_component = 0;
                    self.dac_read = self.dac_read.wrapping_add(1);
                }
                value
            }
            0x3ca => self.feature,
            0x3cc => self.misc,
            0x3ce => self.gfx_index,
            0x3cf => {
                let index = usize::from(self.gfx_index);
                self.graphics[index] & GRAPHICS_MASKS[index]
            }
            0x3b4 | 0x3d4 if self.active_crtc(port) => self.crtc_index,
            0x3b5 | 0x3d5 if self.active_crtc(port) => self.crtc[usize::from(self.crtc_index)],
            0x3ba | 0x3da if self.active_crtc(port) => {
                self.attr_data = false;
                let elapsed = self
                    .epoch
                    .get_or_insert_with(Instant::now)
                    .elapsed()
                    .as_micros();
                let vertical_blank = elapsed % 16_667 >= 15_000;
                (u8::from(vertical_blank) * 9) | u8::from(elapsed % 32 >= 25)
            }
            _ => 0xff,
        }
    }

    fn active_crtc(&self, port: u16) -> bool {
        (port & 0x20 == 0) == (self.misc & 1 != 0)
    }

    fn write_vga(&mut self, port: u16, value: u8) {
        match port {
            0x3c0 => {
                if self.attr_data {
                    self.attribute[usize::from(self.attr_index & 31)] = value;
                } else {
                    self.attr_index = value & 63;
                }
                self.attr_data = !self.attr_data;
            }
            0x3c2 => self.misc = value,
            0x3c4 => self.seq_index = value & 7,
            0x3c5 => {
                let index = usize::from(self.seq_index);
                self.sequencer[index] = value & SEQUENCER_MASKS[index];
            }
            0x3c6 => self.dac_mask = value,
            0x3c7 => {
                self.dac_read = value;
                self.dac_component = 0;
                self.dac_state = 3;
            }
            0x3c8 => {
                self.dac_write = value;
                self.dac_component = 0;
                self.dac_state = 0;
            }
            0x3c9 => {
                self.palette[usize::from(self.dac_write)][usize::from(self.dac_component)] = value;
                self.dac_component += 1;
                if self.dac_component == 3 {
                    self.dac_component = 0;
                    self.dac_write = self.dac_write.wrapping_add(1);
                }
            }
            0x3ce => self.gfx_index = value & 15,
            0x3cf => {
                let index = usize::from(self.gfx_index);
                self.graphics[index] = value & GRAPHICS_MASKS[index];
            }
            0x3b4 | 0x3d4 if self.active_crtc(port) => self.crtc_index = value & 31,
            0x3b5 | 0x3d5 if self.active_crtc(port) => {
                let index = usize::from(self.crtc_index);
                if self.crtc[0x11] & 0x80 == 0 || index > 7 {
                    self.crtc[index] = value;
                } else if index == 7 {
                    self.crtc[7] = (self.crtc[7] & !0x10) | (value & 0x10);
                }
            }
            0x3ba | 0x3da if self.active_crtc(port) => self.feature = value,
            _ => {}
        }
    }

    fn aperture_offset(&self, offset: u64) -> Option<usize> {
        let offset = usize::try_from(offset).ok()?;
        let (base, length) = match (self.graphics[6] >> 2) & 3 {
            0 => (0, 0x20000),
            1 => (0, 0x10000),
            2 => (0x10000, 0x8000),
            _ => (0x18000, 0x8000),
        };
        (base..base + length)
            .contains(&offset)
            .then(|| (offset - base) & 0xffff)
    }

    /// Reads the VGA legacy aperture, with offset relative to physical A0000.
    pub fn read_legacy(&mut self, offset: u64, data: &mut [u8]) {
        for (index, byte) in data.iter_mut().enumerate() {
            *byte = offset
                .checked_add(index as u64)
                .map(|address| self.read_legacy_byte(address))
                .unwrap_or(0xff);
        }
    }

    fn read_legacy_byte(&mut self, offset: u64) -> u8 {
        if self.vbe[4] & ENABLED != 0 {
            return if offset < 0x10000 {
                self.memory
                    .byte(usize::from(self.vbe[5]) * 65536 + usize::try_from(offset).unwrap())
            } else {
                0xff
            };
        }
        let Some(mut address) = self.aperture_offset(offset) else {
            return 0xff;
        };
        let mut plane = usize::from(self.graphics[4] & 3);
        if self.sequencer[4] & 8 != 0 {
            plane = address & 3;
            address >>= 2;
        } else if self.graphics[5] & 0x10 != 0 {
            plane = (plane & 2) | (address & 1);
            address >>= 1;
        }
        self.memory.read(address * 4, &mut self.latch);
        if self.graphics[5] & 8 == 0 {
            return self.latch[plane];
        }
        let mut result = 0xff;
        for (plane, value) in self.latch.iter().enumerate() {
            if self.graphics[7] & (1 << plane) != 0 {
                result &= if self.graphics[2] & (1 << plane) != 0 {
                    *value
                } else {
                    !value
                };
            }
        }
        result
    }

    /// Writes the VGA legacy aperture, with offset relative to physical A0000.
    pub fn write_legacy(&mut self, offset: u64, data: &[u8]) {
        for (index, byte) in data.iter().enumerate() {
            if let Some(address) = offset.checked_add(index as u64) {
                self.write_legacy_byte(address, *byte);
            }
        }
    }

    fn write_legacy_byte(&mut self, offset: u64, value: u8) {
        if self.vbe[4] & ENABLED != 0 {
            if offset < 0x10000 {
                self.memory.set_byte(
                    usize::from(self.vbe[5]) * 65536 + usize::try_from(offset).unwrap(),
                    value,
                );
            }
            return;
        }
        let Some(mut address) = self.aperture_offset(offset) else {
            return;
        };
        let mut planes = self.sequencer[2] & 15;
        if self.sequencer[4] & 8 != 0 {
            planes &= 1 << (address & 3);
            address >>= 2;
        } else if self.sequencer[4] & 4 == 0 {
            planes &= 0x5 << (address & 1);
            address >>= 1;
        }
        let mode = self.graphics[5] & 3;
        let rotated = value.rotate_right(u32::from(self.graphics[3] & 7));
        for plane in 0..4 {
            if planes & (1 << plane) == 0 {
                continue;
            }
            let latch = self.latch[plane];
            let set_reset = if self.graphics[0] & (1 << plane) != 0 {
                0xff
            } else {
                0
            };
            let mut mask = self.graphics[8];
            let source = match mode {
                0 if self.graphics[1] & (1 << plane) != 0 => set_reset,
                0 => rotated,
                1 => {
                    self.memory.set_byte(address * 4 + plane, latch);
                    continue;
                }
                2 => {
                    if value & (1 << plane) != 0 {
                        0xff
                    } else {
                        0
                    }
                }
                _ => {
                    mask &= rotated;
                    set_reset
                }
            };
            let source = match (self.graphics[3] >> 3) & 3 {
                1 => source & latch,
                2 => source | latch,
                3 => source ^ latch,
                _ => source,
            };
            self.memory
                .set_byte(address * 4 + plane, (source & mask) | (latch & !mask));
        }
    }

    fn color(&self, index: u8) -> u32 {
        let [r, g, b] = self.palette[usize::from(index & self.dac_mask)];
        let expand = |value: u8| {
            if self.vbe[4] & DAC_8BIT != 0 {
                value
            } else {
                (value & 63) << 2 | (value & 63) >> 4
            }
        };
        u32::from(expand(r)) << 16 | u32::from(expand(g)) << 8 | u32::from(expand(b))
    }

    fn attribute_color(&self, index: u8) -> u32 {
        let mut palette = self.attribute[usize::from(index & 15)] & 63;
        if self.attribute[0x10] & 0x80 != 0 {
            palette = (palette & 15) | ((self.attribute[0x14] & 3) << 4);
        }
        self.color(palette | ((self.attribute[0x14] & 12) << 4))
    }

    /// Renders a bounded frame. No firmware font is supplied by the host.
    pub fn render(&self) -> Frame {
        if self.vbe[4] & ENABLED != 0 {
            return self.render_vbe();
        }
        self.render_vga()
    }

    fn render_vbe(&self) -> Frame {
        let Some((start, stride, pixel_bytes)) = self.vbe_layout() else {
            return Frame {
                width: 0,
                height: 0,
                pixels: vec![],
            };
        };
        let width = self.vbe[1];
        let height = self.vbe[2];
        let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height));
        for y in 0..usize::from(height) {
            for x in 0..usize::from(width) {
                let offset = start + y * stride + x * pixel_bytes;
                let mut p = [0; 4];
                self.memory.read(offset, &mut p[..pixel_bytes]);
                let color = match self.vbe[3] {
                    8 => self.color(p[0]),
                    15 | 16 => {
                        let word = u32::from(u16::from_le_bytes([p[0], p[1]]));
                        let green_bits = if self.vbe[3] == 16 { 6 } else { 5 };
                        let green_mask = (1 << green_bits) - 1;
                        let r = (word >> (green_bits + 5)) & 31;
                        let g = (word >> 5) & green_mask;
                        let b = word & 31;
                        (r * 255 / 31) << 16 | (g * 255 / green_mask) << 8 | (b * 255 / 31)
                    }
                    _ => u32::from(p[2]) << 16 | u32::from(p[1]) << 8 | u32::from(p[0]),
                };
                pixels.push(color);
            }
        }
        Frame {
            width,
            height,
            pixels,
        }
    }

    fn render_vga(&self) -> Frame {
        let char_width = if self.sequencer[1] & 1 == 0 { 9 } else { 8 };
        let columns = u16::from(self.crtc[1]) + 1;
        let width = (columns * char_width).min(MAX_DIMENSION);
        let height = (u16::from(self.crtc[0x12])
            + ((u16::from(self.crtc[7]) & 2) << 7)
            + ((u16::from(self.crtc[7]) & 0x40) << 3)
            + 1)
        .min(MAX_DIMENSION);
        let mut frame = Frame {
            width,
            height,
            pixels: vec![0; usize::from(width) * usize::from(height)],
        };
        if self.sequencer[1] & 0x20 != 0 || self.attr_index & 0x20 == 0 {
            return frame;
        }
        let text = self.graphics[6] & 1 == 0;
        let char_height = usize::from(self.crtc[9] & 31) + 1;
        let double_scan = usize::from(self.crtc[9] >> 7) + 1;
        let start = usize::from(u16::from_be_bytes([self.crtc[0xc], self.crtc[0xd]]));
        let stride = usize::from(self.crtc[0x13]) * 2;
        let cursor = usize::from(u16::from_be_bytes([self.crtc[0xe], self.crtc[0xf]]));
        let line_compare = usize::from(self.crtc[0x18])
            | (usize::from(self.crtc[7] & 0x10) << 4)
            | (usize::from(self.crtc[9] & 0x40) << 3);
        for y in 0..usize::from(height) {
            let scanline = y / double_scan;
            let (origin, scanline) = if scanline > line_compare {
                (0, scanline - line_compare - 1)
            } else {
                (start, scanline + usize::from(self.crtc[8] & 31))
            };
            for x in 0..usize::from(width) {
                let color = if text {
                    let column = x / usize::from(char_width);
                    let cell = (origin + scanline / char_height * stride + column) & 0xffff;
                    let character = self.memory.byte(cell * 4);
                    let attribute = self.memory.byte(cell * 4 + 1);
                    let map = self.sequencer[3];
                    let font = if attribute & 8 != 0 {
                        ((map >> 2) & 3) | ((map >> 3) & 4)
                    } else {
                        (map & 3) | ((map >> 2) & 4)
                    };
                    let font_offset = usize::from((font & 3) * 2 + (font >> 2)) * 8192;
                    let glyph_row = scanline % char_height;
                    let glyph = self.memory.byte(
                        ((font_offset + usize::from(character) * 32 + glyph_row) & 0xffff) * 4 + 2,
                    );
                    let dot = x % usize::from(char_width);
                    let ink = if dot < 8 {
                        glyph & (0x80 >> dot) != 0
                    } else {
                        self.attribute[0x10] & 4 != 0
                            && (0xc0..=0xdf).contains(&character)
                            && glyph & 1 != 0
                    };
                    let cursor_visible = cell == cursor
                        && self.crtc[0xa] & 0x20 == 0
                        && (usize::from(self.crtc[0xa] & 31)..=usize::from(self.crtc[0xb] & 31))
                            .contains(&glyph_row);
                    self.attribute_color(if ink || cursor_visible {
                        attribute & 15
                    } else {
                        (attribute >> 4) & if self.attribute[0x10] & 8 != 0 { 7 } else { 15 }
                    })
                } else if self.graphics[5] & 0x40 != 0 {
                    let address = ((origin + scanline / char_height * stride) * 4 + x / 2)
                        % self.memory.len();
                    self.color(self.memory.byte(address))
                } else {
                    let address = (origin + scanline / char_height * stride + x / 8) & 0xffff;
                    let mut index = 0;
                    for plane in 0..4 {
                        index |=
                            ((self.memory.byte(address * 4 + plane) >> (7 - x % 8)) & 1) << plane;
                    }
                    self.attribute_color(index & self.attribute[0x12])
                };
                frame.pixels[y * usize::from(width) + x] = color;
            }
        }
        frame
    }
}

#[cfg(feature = "windows")]
fn bounded_range(offset: u64, length: usize, limit: usize) -> Option<std::ops::Range<usize>> {
    let start = usize::try_from(offset).ok()?;
    let end = start.checked_add(length)?;
    (end <= limit).then_some(start..end)
}

#[cfg(test)]
#[cfg(feature = "windows")]
mod tests;
