// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! PCI ISA-bridge discovery node for firmware PS/2 drivers.
//!
//! The existing legacy devices implement the I/O ports and interrupts. This
//! node exposes their class-based firmware discovery path, without chipset BARs.

use std::io;
use std::sync::{Arc, Barrier, Mutex};

use serde::{Deserialize, Serialize};

use super::pci_mngr::PciDevices;
use crate::pci::configuration::{PciConfiguration, PciConfigurationState};
use crate::pci::{PciBridgeSubclass, PciClassCode, PciDevice, PciSBDF};

const VENDOR_ID: u16 = 0x1d0f;
const DEVICE_ID: u16 = 0xfc02;
const CONFIG_DWORDS: u16 = 4096 / 4;

/// Configuration and PCI location of the optional ISA discovery node.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeState {
    config: PciConfigurationState,
    sbdf: PciSBDF,
}

/// Configuration-only ISA bridge; PS/2 ports remain on the legacy I/O bus.
#[derive(Debug)]
pub struct ConsoleBridge {
    config: PciConfiguration,
    sbdf: PciSBDF,
}

impl ConsoleBridge {
    fn new(sbdf: PciSBDF) -> Self {
        let mut config = PciConfiguration::new_type0(
            VENDOR_ID,
            DEVICE_ID,
            1,
            PciClassCode::Bridge,
            PciBridgeSubclass::IsaBridge as u8,
            VENDOR_ID,
            DEVICE_ID,
        );
        config.write_reg(1, 3);
        Self { config, sbdf }
    }

    fn restore(state: &BridgeState) -> io::Result<Self> {
        if state.sbdf.segment() != 0
            || state.sbdf.bus() != 0
            || state.sbdf.device() == 0
            || state.sbdf.function() != 0
        {
            return Err(io::Error::other("invalid console ISA bridge slot"));
        }
        let saved = PciConfiguration::type0_from_state(state.config.clone())
            .map_err(|error| io::Error::other(error.to_string()))?;
        // Rebuild writable masks instead of accepting snapshot-supplied masks.
        let mut device = Self::new(state.sbdf);
        for index in [1, 3, 15] {
            device.config.write_reg(index, saved.read_reg(index));
        }
        if (0..CONFIG_DWORDS).any(|index| saved.read_reg(index) != device.config.read_reg(index)) {
            return Err(io::Error::other("invalid console ISA bridge configuration"));
        }
        Ok(device)
    }

    /// Attach a firmware-discoverable ISA bridge, preserving its slot on restore.
    pub fn attach(
        pci: &mut PciDevices,
        state: Option<&BridgeState>,
    ) -> io::Result<Arc<Mutex<Self>>> {
        let device = match state {
            Some(state) => Self::restore(state)?,
            None => Self::new(
                pci.pci_segment
                    .next_device_sbdf()
                    .map_err(|error| io::Error::other(error.to_string()))?,
            ),
        };
        let slot = device.sbdf.device();
        let device = Arc::new(Mutex::new(device));
        pci.pci_segment
            .pci_bus
            .lock()
            .unwrap()
            .add_device(slot, device.clone())
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(device)
    }

    /// Save PCI configuration and location.
    pub fn save(&self) -> BridgeState {
        BridgeState {
            config: self.config.state(),
            sbdf: self.sbdf,
        }
    }
}

impl PciDevice for ConsoleBridge {
    fn read_config_register(&mut self, index: u16) -> u32 {
        self.config.read_reg(index)
    }

    fn write_config_register(
        &mut self,
        index: u16,
        offset: u8,
        bytes: &[u8],
    ) -> Option<Arc<Barrier>> {
        self.config.write_config_register(index, offset, bytes);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_discovers_isa_class_and_enabled_decode() {
        let mut device = ConsoleBridge::new(PciSBDF::new(0, 0, 1, 0));
        assert_eq!(device.read_config_register(0), 0xfc02_1d0f);
        assert_eq!(device.read_config_register(2), 0x0601_0001);
        assert_eq!(device.read_config_register(1) & 3, 3);
        assert_eq!(device.read_config_register(3) >> 16 & 0xff, 0);
        device.write_config_register(0, 0, &[0; 4]);
        device.write_config_register(2, 0, &[0; 4]);
        assert_eq!(device.read_config_register(0), 0xfc02_1d0f);
        assert_eq!(device.read_config_register(2), 0x0601_0001);
        for index in 4..10 {
            device.write_config_register(index, 0, &[0xff; 4]);
            assert_eq!(device.read_config_register(index), 0);
        }
    }

    #[test]
    fn configuration_round_trip_preserves_writable_fields_and_slot() {
        let mut original = ConsoleBridge::new(PciSBDF::new(0, 0, 7, 0));
        original.write_config_register(1, 0, &[0, 0]);
        original.write_config_register(3, 0, &[64]);
        original.write_config_register(15, 0, &[12]);
        let mut restored = ConsoleBridge::restore(&original.save()).unwrap();
        assert_eq!(restored.sbdf, original.sbdf);
        for index in 0..CONFIG_DWORDS {
            assert_eq!(
                restored.read_config_register(index),
                original.read_config_register(index)
            );
        }
        restored.write_config_register(1, 0, &[3]);
        assert_eq!(restored.read_config_register(1), 3);
    }

    #[test]
    fn malformed_snapshot_identity_and_slots_are_rejected() {
        let device = ConsoleBridge::new(PciSBDF::new(0, 0, 1, 0));
        for slot in [
            PciSBDF::new(0, 0, 0, 0),
            PciSBDF::new(1, 0, 1, 0),
            PciSBDF::new(0, 1, 1, 0),
            PciSBDF::new(0, 0, 1, 1),
        ] {
            let mut state = device.save();
            state.sbdf = slot;
            ConsoleBridge::restore(&state).unwrap_err();
        }
        let mut state = device.save();
        state.config = PciConfiguration::new_type0(
            0x1234,
            DEVICE_ID,
            1,
            PciClassCode::Bridge,
            1,
            VENDOR_ID,
            DEVICE_ID,
        )
        .state();
        ConsoleBridge::restore(&state).unwrap_err();
        state.config = PciConfiguration::new_type0(
            VENDOR_ID,
            DEVICE_ID,
            1,
            PciClassCode::DisplayController,
            0,
            VENDOR_ID,
            DEVICE_ID,
        )
        .state();
        ConsoleBridge::restore(&state).unwrap_err();
    }

    #[test]
    fn invalid_config_accesses_are_bounded() {
        let mut device = ConsoleBridge::new(PciSBDF::new(0, 0, 1, 0));
        device.write_config_register(u16::MAX, 0, &[0xff; 4]);
        device.write_config_register(1, 3, &[0xff; 2]);
        assert_eq!(device.read_config_register(u16::MAX), u32::MAX);
        assert_eq!(device.read_config_register(1), 3);
    }
}
