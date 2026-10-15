// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::fs::File;
use std::io;

use linux_loader::cmdline;
use serde::{Deserialize, Serialize};

/// Default guest kernel command line:
/// - `reboot=k` shut down the guest on reboot, instead of well... rebooting;
/// - `panic=1` on panic, reboot after 1 second;
/// - `nomodule` disable loadable kernel module support;
/// - `8250.nr_uarts=0` disable 8250 serial interface;
/// - `i8042.noaux` do not probe the i8042 controller for an attached mouse (save boot time);
/// - `i8042.nomux` do not probe i8042 for a multiplexing controller (save boot time);
/// - `i8042.dumbkbd` do not attempt to control kbd state via the i8042 (save boot time).
/// - `swiotlb=noforce` disable software bounce buffers (SWIOTLB)
pub const DEFAULT_KERNEL_CMDLINE: &str = "reboot=k panic=1 nomodule 8250.nr_uarts=0 i8042.noaux \
                                          i8042.nomux i8042.dumbkbd swiotlb=noforce";

/// Strongly typed data structure used to configure the boot source of the
/// microvm.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BootSourceConfig {
    /// Path of the kernel image.
    pub kernel_image_path: String,
    /// Path of the initrd, if there is one.
    pub initrd_path: Option<String>,
    /// The boot arguments to pass to the kernel. If this field is uninitialized,
    /// DEFAULT_KERNEL_CMDLINE is used.
    pub boot_args: Option<String>,
    /// Optional firmware ABI and stable system identity.
    pub firmware: Option<FirmwareConfig>,
}

/// Supported firmware handoff profiles.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FirmwareType {
    /// AMD64 CloudHv PVH firmware with SMBIOS identity.
    #[serde(rename = "cloud-hv")]
    CloudHv,
}

/// Explicit firmware ABI declaration; never inferred from the image filename.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FirmwareConfig {
    /// Firmware ABI selected by the caller.
    #[serde(rename = "type")]
    pub profile: FirmwareType,
    /// Caller-owned identity, stable across cold boots and snapshot restores.
    pub system_uuid: SystemUuid,
}

/// A system UUID that excludes SMBIOS's absent and unset identity sentinels.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct SystemUuid([u8; 16]);

impl TryFrom<String> for SystemUuid {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let uuid = uuid::Uuid::parse_str(&value).map_err(|error| error.to_string())?;
        let bytes = uuid.into_bytes();
        if bytes == [0; 16] || bytes == [255; 16] {
            return Err("system UUID must not be nil or all ones".into());
        }
        Ok(Self(bytes))
    }
}

impl From<SystemUuid> for String {
    fn from(value: SystemUuid) -> Self {
        uuid::Uuid::from_bytes(value.0).to_string()
    }
}

impl SystemUuid {
    /// SMBIOS 2.6 and later encode the first three UUID fields little-endian.
    pub fn smbios_bytes(self) -> [u8; 16] {
        uuid::Uuid::from_bytes(self.0).to_bytes_le()
    }
}

impl BootSourceConfig {
    /// Check profile constraints without opening boot files during snapshot restore.
    pub fn validate(&self) -> Result<(), BootSourceConfigError> {
        if self.firmware.is_some() {
            if !cfg!(target_arch = "x86_64") {
                return Err(BootSourceConfigError::InvalidFirmware(
                    "CloudHv identity requires AMD64",
                ));
            }
            if self.initrd_path.is_some() {
                return Err(BootSourceConfigError::InvalidFirmware(
                    "CloudHv identity does not accept an initrd",
                ));
            }
        }
        Ok(())
    }
}

/// Errors associated with actions on `BootSourceConfig`.
#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum BootSourceConfigError {
    /// Invalid firmware configuration: {0}
    InvalidFirmware(&'static str),
    /// The kernel file cannot be opened: {0}
    InvalidKernelPath(io::Error),
    /// The initrd file cannot be opened due to invalid path or invalid permissions. {0}
    InvalidInitrdPath(io::Error),
    /// The kernel command line is invalid: {0}
    InvalidKernelCommandLine(String),
}

/// Holds the kernel specification (both configuration as well as runtime details).
#[derive(Debug, Default)]
pub struct BootSource {
    /// The boot source configuration.
    pub config: BootSourceConfig,
    /// The boot source builder (a boot source allocated and validated).
    /// It is an option cause a resumed microVM does not need it.
    pub builder: Option<BootConfig>,
}

/// Holds the kernel builder (created and validates based on BootSourceConfig).
#[derive(Debug)]
pub struct BootConfig {
    /// The commandline, if explicitly provided by the user. `None` means the
    /// consumer should apply its own default (e.g. [`DEFAULT_KERNEL_CMDLINE`]).
    pub cmdline: Option<cmdline::Cmdline>,
    /// The descriptor to the kernel file.
    pub kernel_file: File,
    /// The descriptor to the initrd file, if there is one.
    pub initrd_file: Option<File>,
}

/// Build a [`Cmdline`](cmdline::Cmdline) from a string.
pub fn build_cmdline(s: &str) -> Result<cmdline::Cmdline, cmdline::Error> {
    cmdline::Cmdline::try_from(s, crate::arch::CMDLINE_MAX_SIZE)
}

/// Append the `root=` (and `ro`/`rw`) arguments derived from a root block
/// device to `cmdline`. Uses `root=PARTUUID=<uuid>` when a partuuid is
/// available, otherwise `root=/dev/vda`.
pub fn append_root_device_cmdline(
    cmdline: &mut cmdline::Cmdline,
    partuuid: Option<&str>,
    read_only: bool,
) -> Result<(), cmdline::Error> {
    match partuuid {
        Some(uuid) => cmdline.insert_str(format!("root=PARTUUID={uuid}"))?,
        None => cmdline.insert_str("root=/dev/vda")?,
    }
    cmdline.insert_str(if read_only { "ro" } else { "rw" })?;
    Ok(())
}

impl BootConfig {
    /// Creates the BootConfig based on a given configuration.
    pub fn new(cfg: &BootSourceConfig) -> Result<Self, BootSourceConfigError> {
        use self::BootSourceConfigError::{
            InvalidInitrdPath, InvalidKernelCommandLine, InvalidKernelPath,
        };

        cfg.validate()?;

        // Validate boot source config.
        let kernel_file = File::open(&cfg.kernel_image_path).map_err(InvalidKernelPath)?;
        let initrd_file: Option<File> = match &cfg.initrd_path {
            Some(path) => Some(File::open(path).map_err(InvalidInitrdPath)?),
            None => None,
        };

        let cmdline = match cfg.boot_args.as_ref() {
            None => None,
            Some(s) => {
                Some(build_cmdline(s).map_err(|err| InvalidKernelCommandLine(err.to_string()))?)
            }
        };

        Ok(BootConfig {
            cmdline,
            kernel_file,
            initrd_file,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use vmm_sys_util::tempfile::TempFile;

    use super::*;
    use crate::snapshot::Snapshot;

    #[test]
    fn test_boot_config() {
        let kernel_file = TempFile::new().unwrap();
        let kernel_path = kernel_file.as_path().to_str().unwrap().to_string();

        let boot_src_cfg = BootSourceConfig {
            firmware: None,
            boot_args: None,
            initrd_path: None,
            kernel_image_path: kernel_path,
        };

        let boot_cfg = BootConfig::new(&boot_src_cfg).unwrap();
        assert!(boot_cfg.initrd_file.is_none());
        assert!(boot_cfg.cmdline.is_none());
    }

    #[test]
    fn test_serde() {
        let boot_src_cfg = BootSourceConfig {
            firmware: None,
            boot_args: Some(DEFAULT_KERNEL_CMDLINE.to_string()),
            initrd_path: Some("/tmp/initrd".to_string()),
            kernel_image_path: "./vmlinux.bin".to_string(),
        };

        // Use bitcode serialization directly for the test data
        let serialized_data = bitcode::serialize(&boot_src_cfg).unwrap();
        let restored_boot_cfg: BootSourceConfig = bitcode::deserialize(&serialized_data).unwrap();
        assert_eq!(boot_src_cfg, restored_boot_cfg);

        // Also test with Snapshot wrapper
        let snapshot_data = bitcode::serialize(&Snapshot::new(boot_src_cfg.clone())).unwrap();
        let restored_snapshot = Snapshot::load_without_crc_check(&snapshot_data).unwrap();
        assert_eq!(boot_src_cfg, restored_snapshot.data);
    }
    fn cloudhv_json() -> serde_json::Value {
        serde_json::json!({
            "kernel_image_path": "firmware.fd",
            "firmware": {"type": "cloud-hv", "system_uuid": "11223344-5566-7788-99aa-bbccddeeff01"}
        })
    }

    #[test]
    fn cloudhv_profile_round_trips_json_and_snapshot_identity() {
        let config: BootSourceConfig = serde_json::from_value(cloudhv_json()).unwrap();
        let encoded = bitcode::serialize(&config).unwrap();
        let restored: BootSourceConfig = bitcode::deserialize(&encoded).unwrap();
        assert_eq!(config, restored);
        let value = serde_json::to_value(&restored).unwrap();
        assert_eq!(value["firmware"], cloudhv_json()["firmware"]);
        let legacy: BootSourceConfig =
            serde_json::from_str(r#"{"kernel_image_path":"seabios.elf"}"#).unwrap();
        assert_eq!(legacy.firmware, None);
        legacy.validate().unwrap();
    }

    #[test]
    fn cloudhv_profile_requires_supported_type_and_real_uuid() {
        for value in [
            "",
            "invalid",
            "00000000-0000-0000-0000-000000000000",
            "ffffffff-ffff-ffff-ffff-ffffffffffff",
        ] {
            let mut json = cloudhv_json();
            json["firmware"]["system_uuid"] = value.into();
            serde_json::from_value::<BootSourceConfig>(json).unwrap_err();
        }
        for field in ["type", "system_uuid"] {
            let mut json = cloudhv_json();
            json["firmware"].as_object_mut().unwrap().remove(field);
            serde_json::from_value::<BootSourceConfig>(json).unwrap_err();
        }
        let mut json = cloudhv_json();
        json["firmware"]["type"] = "seabios".into();
        serde_json::from_value::<BootSourceConfig>(json).unwrap_err();
        let mut json = cloudhv_json();
        json["firmware"]["extra"] = true.into();
        serde_json::from_value::<BootSourceConfig>(json).unwrap_err();
    }

    #[test]
    fn cloudhv_profile_rejects_initrd_and_other_architectures_before_open() {
        let mut config: BootSourceConfig = serde_json::from_value(cloudhv_json()).unwrap();
        #[cfg(target_arch = "x86_64")]
        config.validate().unwrap();
        #[cfg(target_arch = "aarch64")]
        config.validate().unwrap_err();
        config.initrd_path = Some("unused".into());
        assert!(matches!(
            BootConfig::new(&config),
            Err(BootSourceConfigError::InvalidFirmware(_))
        ));
    }
}
