// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Optional host console and installation hardware.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// AMD64 boot and native display profile.
#[cfg(target_arch = "x86_64")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DisplayModel {
    /// Legacy VGA for guests without a virtio display driver.
    #[default]
    Vga,
    /// VGA boot display with a modern virtio 2D display driver.
    VirtioGpu,
}

/// One AMD64 optical drive; list order selects primary then secondary master.
#[cfg(target_arch = "x86_64")]
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpticalDriveConfig {
    /// Stable host control-plane identifier.
    pub drive_id: String,
    /// Optional read-only ISO; omission leaves the drive empty.
    pub path_on_host: Option<PathBuf>,
}

/// Preferred modern display dimensions requested by the host.
#[cfg(target_arch = "x86_64")]
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayUpdateConfig {
    /// Preferred width in pixels.
    pub width: u32,
    /// Preferred height in pixels.
    pub height: u32,
}

/// Local console endpoint and architecture-specific installation hardware.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleConfig {
    /// Private Unix socket consumed by the host console service.
    pub socket_path: PathBuf,
    /// Legacy VGA by default; virtio GPU requires explicit CloudHv firmware.
    #[cfg(target_arch = "x86_64")]
    #[serde(default)]
    pub display: DisplayModel,
    /// Optional with explicit CloudHv firmware; required for SeaBIOS and legacy profiles.
    #[cfg(target_arch = "x86_64")]
    pub vga_rom_path: Option<PathBuf>,
    /// At most two optical masters, in primary/secondary channel order.
    #[cfg(target_arch = "x86_64")]
    #[serde(default)]
    pub optical_drives: Vec<OpticalDriveConfig>,
}

impl ConsoleConfig {
    /// Validate host paths before constructing devices.
    pub fn validate(&self) -> Result<(), std::io::Error> {
        super::require_windows_support()?;
        #[cfg(target_arch = "x86_64")]
        if self.display == DisplayModel::VirtioGpu && self.vga_rom_path.is_some() {
            return Err(std::io::Error::other(
                "virtio GPU does not use a VGA option ROM",
            ));
        }
        #[cfg(target_arch = "x86_64")]
        if self.optical_drives.len() > 2
            || self.optical_drives.iter().any(|drive| {
                drive.drive_id.is_empty()
                    || !drive
                        .drive_id
                        .chars()
                        .all(|c| c == '_' || c.is_alphanumeric())
            })
            || (self.optical_drives.len() == 2
                && self.optical_drives[0].drive_id == self.optical_drives[1].drive_id)
        {
            return Err(std::io::Error::other(
                "optical drives require at most two unique alphanumeric or underscore IDs",
            ));
        }
        utils::unix_socket::validate(&self.socket_path)
    }

    #[cfg(all(feature = "windows", target_arch = "x86_64"))]
    pub(crate) fn optical_irqs(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.optical_drives.len())
            .map(|channel| crate::devices::legacy::atapi::IRQ + u32::try_from(channel).unwrap())
    }

    /// Validate the console against the explicitly selected firmware ABI.
    pub fn validate_for_firmware(
        &self,
        firmware: Option<&crate::vmm_config::boot_source::FirmwareConfig>,
    ) -> Result<(), std::io::Error> {
        self.validate()?;
        #[cfg(target_arch = "x86_64")]
        if self.display == DisplayModel::VirtioGpu
            && !matches!(
                firmware,
                Some(crate::vmm_config::boot_source::FirmwareConfig::CloudHv { .. })
            )
        {
            return Err(std::io::Error::other(
                "virtio GPU requires explicit CloudHv firmware",
            ));
        }
        #[cfg(target_arch = "x86_64")]
        if self.vga_rom_path.is_none()
            && !matches!(
                firmware,
                Some(crate::vmm_config::boot_source::FirmwareConfig::CloudHv { .. })
            )
        {
            return Err(std::io::Error::other(
                "VGA option ROM is required unless CloudHv firmware is explicitly selected",
            ));
        }
        #[cfg(target_arch = "aarch64")]
        let _ = firmware;
        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "windows")]
#[cfg(target_arch = "x86_64")]
mod tests {
    use super::*;

    #[test]
    fn omitted_rom_requires_explicit_cloud_hv_firmware() {
        use crate::vmm_config::boot_source::FirmwareConfig;
        let config: ConsoleConfig =
            serde_json::from_str(r#"{"socket_path":"console.sock"}"#).unwrap();
        config.validate_for_firmware(None).unwrap_err();
        config
            .validate_for_firmware(Some(&FirmwareConfig::SeaBios))
            .unwrap_err();
        let cloud_hv = FirmwareConfig::CloudHv {
            system_uuid: "11223344-5566-7788-99aa-bbccddeeff01"
                .to_string()
                .try_into()
                .unwrap(),
        };
        config.validate_for_firmware(Some(&cloud_hv)).unwrap();
        let present = ConsoleConfig {
            vga_rom_path: Some("vgabios.bin".into()),
            ..config
        };
        for firmware in [None, Some(&FirmwareConfig::SeaBios), Some(&cloud_hv)] {
            present.validate_for_firmware(firmware).unwrap();
        }
        let encoded = bitcode::serialize(&present).unwrap();
        let restored: ConsoleConfig = bitcode::deserialize(&encoded).unwrap();
        assert_eq!(restored, present);
    }

    #[test]
    fn virtio_gpu_requires_cloud_hv_and_no_option_rom() {
        use crate::vmm_config::boot_source::FirmwareConfig;
        let mut config: ConsoleConfig =
            serde_json::from_str(r#"{"socket_path":"console.sock","display":"virtio-gpu"}"#)
                .unwrap();
        assert_eq!(config.display, DisplayModel::VirtioGpu);
        config.validate_for_firmware(None).unwrap_err();
        config
            .validate_for_firmware(Some(&FirmwareConfig::SeaBios))
            .unwrap_err();
        let firmware = FirmwareConfig::CloudHv {
            system_uuid: "11223344-5566-7788-99aa-bbccddeeff01"
                .to_owned()
                .try_into()
                .unwrap(),
        };
        config.validate_for_firmware(Some(&firmware)).unwrap();
        config.vga_rom_path = Some("vgabios.bin".into());
        config.validate().unwrap_err();
        serde_json::from_str::<ConsoleConfig>(
            r#"{"socket_path":"console.sock","display":"unknown"}"#,
        )
        .unwrap_err();
    }

    #[test]
    fn optical_drives_are_bounded_and_have_unique_ids() {
        let mut config: ConsoleConfig = serde_json::from_str(r#"{"socket_path":"console.sock","optical_drives":[{"drive_id":"payload","path_on_host":"payload.iso"},{"drive_id":"exec"}]}"#).unwrap();
        config.validate().unwrap();
        assert_eq!(config.optical_irqs().collect::<Vec<_>>(), [14, 15]);
        assert!(config.optical_drives[1].path_on_host.is_none());
        config.optical_drives[1].drive_id = "payload".into();
        config.validate().unwrap_err();
        config.optical_drives[1].drive_id = "".into();
        config.validate().unwrap_err();
        config.optical_drives[1].drive_id = "exec".into();
        config.optical_drives.push(config.optical_drives[1].clone());
        config.validate().unwrap_err();
        serde_json::from_str::<ConsoleConfig>(
            r#"{"socket_path":"console.sock","optical_image_path":"old.iso"}"#,
        )
        .unwrap_err();
    }

    #[test]
    fn console_configuration_rejects_ambiguous_or_invalid_paths() {
        for path in [String::new(), "x".repeat(108), "bad\0path".into()] {
            let config = ConsoleConfig {
                socket_path: path.into(),
                #[cfg(target_arch = "x86_64")]
                display: Default::default(),
                vga_rom_path: Some("vgabios.bin".into()),
                optical_drives: Vec::new(),
            };
            config.validate().unwrap_err();
        }
        let config: ConsoleConfig = serde_json::from_str(
            r#"{"socket_path":"console.sock","vga_rom_path":"vgabios.bin","optical_drives":[{"drive_id":"setup","path_on_host":"setup.iso"}]}"#,
        )
        .unwrap();
        config.validate().unwrap();
        serde_json::from_str::<ConsoleConfig>(r#"{"socket_path":"console.sock","unknown":true}"#)
            .unwrap_err();
    }
}

#[cfg(test)]
#[cfg(feature = "windows")]
#[cfg(target_arch = "x86_64")]
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

#[cfg(test)]
#[cfg(feature = "windows")]
#[cfg(target_arch = "aarch64")]
mod arm_tests {
    use super::*;

    #[test]
    fn arm_console_accepts_only_the_private_socket() {
        let config: ConsoleConfig =
            serde_json::from_str(r#"{"socket_path":"console.sock"}"#).unwrap();
        config.validate().unwrap();
        for property in ["vga_rom_path", "optical_drives"] {
            let value = serde_json::json!({"socket_path":"console.sock", (property):"unsupported"});
            serde_json::from_value::<ConsoleConfig>(value).unwrap_err();
        }
    }
}
