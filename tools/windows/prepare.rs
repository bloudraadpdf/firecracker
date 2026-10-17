// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Create a private, sparse disk for installation directly in Firecracker.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

enum Profile {
    Server,
    Client,
    Arm64Client,
}

impl Profile {
    fn answer(&self, index: u32, password: &str) -> String {
        let arm64 = matches!(self, Self::Arm64Client);
        include_str!("Autounattend.xml.in")
            .replace("@ARCH@", if arm64 { "arm64" } else { "amd64" })
            .replace("@EMS_PORT_COMMAND@", if arm64 { "" } else {
                "        <RunSynchronousCommand xmlns:wcm=\"http://schemas.microsoft.com/WMIConfig/2002/State\" wcm:action=\"add\"><Order>2</Order><Path>cmd /c bcdedit /emssettings emsport:1 emsbaudrate:115200</Path></RunSynchronousCommand>\n"
            })
            .replace("@ACCOUNT_SETTINGS@", self.account_settings())
            .replace("@IMAGE_INDEX@", &index.to_string())
            .replace("@PASSWORD@", password)
    }

    fn account_settings(&self) -> &'static str {
        match self {
            Self::Server => {
                "<AdministratorPassword><Value>@PASSWORD@</Value><PlainText>true</PlainText></\
                 AdministratorPassword>"
            }
            Self::Client | Self::Arm64Client => {
                r#"<LocalAccounts><LocalAccount xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State" wcm:action="add"><Password><Value>@PASSWORD@</Value><PlainText>true</PlainText></Password><Name>Firecracker</Name><Group>Administrators</Group></LocalAccount></LocalAccounts>"#
            }
        }
    }
}

fn prepare(output: &Path, index: u32, profile: &Profile) -> std::io::Result<()> {
    DirBuilder::new().mode(0o700).create(output)?;
    let result = (|| {
        let disk = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(output.join("windows.raw"))?;
        disk.set_len(64 * 1024 * 1024 * 1024)?;
        disk.sync_all()?;
        let mut random = [0u8; 24];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let password = format!(
            "Fc1!{}",
            random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let answer = profile.answer(index, &password);
        for (name, bytes) in [
            ("Autounattend.xml", answer.as_bytes()),
            ("account-password", password.as_bytes()),
        ] {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(output.join(name))?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        fs::remove_dir_all(output)?;
    }
    result
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [profile, output, index] = args.as_slice() else {
        return Err(
            "usage: prepare server|client|client-arm64 OUTPUT_DIRECTORY IMAGE_INDEX".into(),
        );
    };
    let profile = match profile.to_str() {
        Some("server") => Profile::Server,
        Some("client") => Profile::Client,
        Some("client-arm64") => Profile::Arm64Client,
        _ => return Err("profile must be server, client or client-arm64".into()),
    };
    let index: u32 = index.to_str().ok_or("invalid image index")?.parse()?;
    if index == 0 {
        return Err("image index must be positive".into());
    }
    prepare(Path::new(output), index, &profile)?;
    println!(
        "Created {}/windows.raw (64 GiB sparse disk).",
        Path::new(output).display()
    );
    if matches!(profile, Profile::Arm64Client) {
        println!(
            "ARM64 media servicing and installer assembly remain required; see \
             docs/windows-arm64.md."
        );
    } else {
        println!("Install directly in Firecracker using docs/windows-console.md.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm64_answer_uses_firmware_serial_settings_and_native_components() {
        let answer = Profile::Arm64Client.answer(2, "Fc1!fixture");
        assert_eq!(answer.matches("processorArchitecture=\"arm64\"").count(), 6);
        assert!(!answer.contains("amd64") && !answer.contains("/emssettings"));
        assert!(answer.contains("bcdedit /ems {default} on"));
        assert!(answer.contains("bcdedit /bootems {bootmgr} on"));
        assert!(answer.contains("<Name>Firecracker</Name>"));
        assert!(answer.contains("<Value>2</Value>"));
        assert!(answer.contains("<Value>Fc1!fixture</Value>"));
        assert!(!answer.contains('@'));
    }

    #[test]
    fn amd64_answers_keep_explicit_com1_and_matching_account_types() {
        for profile in [Profile::Server, Profile::Client] {
            let answer = profile.answer(1, "Fc1!fixture");
            assert_eq!(answer.matches("processorArchitecture=\"amd64\"").count(), 6);
            assert!(answer.contains("bcdedit /emssettings emsport:1 emsbaudrate:115200"));
            assert_eq!(
                answer.contains("AdministratorPassword"),
                matches!(profile, Profile::Server)
            );
            assert_eq!(
                answer.contains("<Name>Firecracker</Name>"),
                matches!(profile, Profile::Client)
            );
            assert!(!answer.contains('@'));
        }
    }
}
