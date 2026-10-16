// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

use super::{ENABLED, FRAMEBUFFER_SIZE, MAX_DIMENSION};

/// Display checkpoint bytes and registers without a host mapping.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VgaState {
    pub(super) memory: Vec<u8>,
    pub(super) sequencer: [u8; 8],
    pub(super) graphics: [u8; 16],
    pub(super) attribute: [u8; 32],
    pub(super) crtc: [u8; 32],
    pub(super) palette: Vec<[u8; 3]>,
    pub(super) seq_index: u8,
    pub(super) gfx_index: u8,
    pub(super) attr_index: u8,
    pub(super) crtc_index: u8,
    pub(super) attr_data: bool,
    pub(super) misc: u8,
    pub(super) feature: u8,
    pub(super) dac_mask: u8,
    pub(super) dac_read: u8,
    pub(super) dac_write: u8,
    pub(super) dac_component: u8,
    pub(super) dac_state: u8,
    pub(super) latch: [u8; 4],
    pub(super) vbe_index: u16,
    pub(super) vbe: [u16; 11],
}

impl VgaState {
    /// Rejects malformed restored state before it can reach device accesses.
    pub fn validate(&self) -> bool {
        self.memory.len() == FRAMEBUFFER_SIZE
            && self.palette.len() == 256
            && self.dac_component < 3
            && self.seq_index < 8
            && self.gfx_index < 16
            && self.crtc_index < 32
            && self.attr_index < 64
            && usize::from(self.vbe[5]) < FRAMEBUFFER_SIZE / 65536
            && (self.vbe[4] & ENABLED == 0 || vbe_layout(self.vbe, self.memory.len()).is_some())
    }
}

pub(super) fn vbe_layout(registers: [u16; 11], limit: usize) -> Option<(usize, usize, usize)> {
    let [
        _,
        width,
        height,
        bpp,
        _,
        _,
        virtual_width,
        virtual_height,
        x,
        y,
        _,
    ] = registers;
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || !matches!(bpp, 8 | 15 | 16 | 24 | 32)
        || virtual_width < width
        || u32::from(x) + u32::from(width) > u32::from(virtual_width)
        || u32::from(y) + u32::from(height) > u32::from(virtual_height)
    {
        return None;
    }
    let pixel_bytes = usize::from(bpp.div_ceil(8));
    let stride = usize::from(virtual_width) * pixel_bytes;
    let start = usize::from(y) * stride + usize::from(x) * pixel_bytes;
    let end = start
        .checked_add((usize::from(height) - 1) * stride)?
        .checked_add(usize::from(width) * pixel_bytes)?;
    (end <= limit).then_some((start, stride, pixel_bytes))
}
