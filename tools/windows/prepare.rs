// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Create a private, sparse disk for installation directly in Firecracker.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

fn prepare(output: &Path, index: u32) -> std::io::Result<()> {
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
        let answer = include_str!("Autounattend.xml.in")
            .replace("@ARCH@", "amd64")
            .replace("@DRIVER_OS@", "2k25")
            .replace("@EMS_SETTINGS@", "emsport:1 emsbaudrate:115200")
            .replace("@LOCAL_ACCOUNTS@", "")
            .replace("@IMAGE_INDEX@", &index.to_string())
            .replace("@ADMIN_PASSWORD@", &password);
        for (name, bytes) in [
            ("Autounattend.xml", answer.as_bytes()),
            ("administrator-password", password.as_bytes()),
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
    let [output, index] = args.as_slice() else {
        return Err("usage: prepare OUTPUT_DIRECTORY SERVER_CORE_IMAGE_INDEX".into());
    };
    let index: u32 = index.to_str().ok_or("invalid image index")?.parse()?;
    if index == 0 {
        return Err("image index must be positive".into());
    }
    prepare(Path::new(output), index)?;
    println!(
        "Created {}/windows.raw (64 GiB sparse disk).",
        Path::new(output).display()
    );
    println!("Install directly in Firecracker using docs/windows-console.md.");
    Ok(())
}
