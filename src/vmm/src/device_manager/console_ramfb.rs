// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Firmware-reserved RAM display using the fw_cfg MMIO/DMA interface.
//! https://www.qemu.org/docs/master/specs/fw_cfg.html

use serde::{Deserialize, Serialize};

#[cfg(feature = "windows")]
pub(crate) const MMIO_BASE: u64 = 0x1000_2000;
#[cfg(feature = "windows")]
pub(crate) const MMIO_SIZE: u64 = 0x1000;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RamfbState {
    selector: u16,
    offset: u32,
    dma_high: u32,
    config: [u8; 28],
}

#[cfg(feature = "windows")]
mod device;
#[cfg(feature = "windows")]
pub(super) use device::Ramfb;
