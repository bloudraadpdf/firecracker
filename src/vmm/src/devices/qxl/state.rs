// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Host-owned QXL checkpoint data, separate from validated runtime objects.

use std::collections::VecDeque;
use std::ops::Range;

use crate::devices::display::VgaState;
use crate::pci::configuration::{Bars, PciConfigurationState};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct MemoryState {
    pub ram: Vec<u8>,
    pub vram: Vec<u8>,
    pub rom: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) enum SlotRegion {
    Ram,
    Vram,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct SlotState {
    pub region: SlotRegion,
    pub range: Range<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct AddressSpaceState {
    pub slots: [Option<SlotState>; 2],
    pub generations: [u8; 2],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ReleaseState {
    pub address: u64,
    pub id: [u8; 8],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct QueueState {
    pub consumers: [u32; 2],
    pub release_producer: u32,
    pub release_consumer: u32,
    pub releases: Vec<ReleaseState>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) enum CursorPixels {
    Alpha(Vec<u32>),
    Masked(Vec<[u32; 2]>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ShapeState {
    pub width: u16,
    pub height: u16,
    pub hotspot: [u16; 2],
    pub pixels: CursorPixels,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct PointerState {
    pub shape: Option<ShapeState>,
    pub position: [i16; 2],
    pub visible: bool,
    pub length: usize,
    pub frequency: u64,
    pub sampled: u64,
    pub history: VecDeque<[i16; 2]>,
}

/// QXL control state and private banks; the boot framebuffer is saved by VGA once.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControllerState {
    pub(super) memory: MemoryState,
    pub(super) addresses: AddressSpaceState,
    pub(super) queues: QueueState,
    pub(super) primary: Option<Vec<u8>>,
    pub(super) surfaces: Vec<(u32, Vec<u8>)>,
    pub(super) attached: bool,
    pub(super) pointer: PointerState,
}

/// Complete adapter checkpoint, including PCI registers and the shared boot VGA.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PciState {
    pub(super) config: PciConfigurationState,
    pub(super) bars: Bars,
    pub(super) mapped: [u64; 3],
    pub(super) io_base: u64,
    pub(super) vga: VgaState,
    pub(super) controller: ControllerState,
}
