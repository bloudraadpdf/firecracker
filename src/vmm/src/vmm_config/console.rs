// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Optional host console and installation hardware.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Local endpoints and read-only installation media for an AMD64 console.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleConfig {
    /// Private Unix socket consumed by the host console service.
    pub socket_path: PathBuf,
    /// SeaVGABIOS expansion ROM for the emulated VGA device.
    pub vga_rom_path: PathBuf,
    /// Optional ISO attached to the primary IDE channel as its master.
    pub optical_image_path: Option<PathBuf>,
}

impl ConsoleConfig {
    /// Validate host paths before constructing devices.
    pub fn validate(&self) -> Result<(), std::io::Error> {
        utils::unix_socket::validate(&self.socket_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_configuration_rejects_ambiguous_or_invalid_paths() {
        for path in [String::new(), "x".repeat(108), "bad\0path".into()] {
            let config = ConsoleConfig {
                socket_path: path.into(),
                vga_rom_path: "vgabios.bin".into(),
                optical_image_path: None,
            };
            config.validate().unwrap_err();
        }
        let config: ConsoleConfig = serde_json::from_str(
            r#"{"socket_path":"console.sock","vga_rom_path":"vgabios.bin","optical_image_path":"setup.iso"}"#,
        )
        .unwrap();
        config.validate().unwrap();
        serde_json::from_str::<ConsoleConfig>(r#"{"socket_path":"console.sock","unknown":true}"#)
            .unwrap_err();
    }
}

#[cfg(test)]
mod memory_tests {
    use crate::arch::x86_64::{arch_memory_regions, reserve_vga_memory};
    use vm_memory::GuestAddress;

    #[test]
    fn vga_aperture_excludes_ram_without_moving_other_addresses() {
        let size = 16 * 1024 * 1024;
        let regions = reserve_vga_memory(arch_memory_regions(size));
        assert_eq!(
            regions,
            [
                (GuestAddress(0), 0xa0000),
                (GuestAddress(0xc0000), size - 0xc0000)
            ]
        );
        assert_eq!(
            regions.iter().map(|(_, size)| size).sum::<usize>(),
            size - 0x20000
        );
        assert_eq!(reserve_vga_memory(regions.clone()), regions);
    }
}
