// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Prepare an isolated Windows installation disk and answer ISO.

use std::env;
use std::error::Error;
use std::fs::{self, DirBuilder, File};
use std::io::{Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy)]
enum Architecture {
    Amd64,
    Arm64,
}

impl Architecture {
    fn name(self) -> &'static str {
        match self {
            Self::Amd64 => "amd64",
            Self::Arm64 => "arm64",
        }
    }

    fn emulator(self) -> &'static str {
        match self {
            Self::Amd64 => "qemu-system-x86_64",
            Self::Arm64 => "qemu-system-aarch64",
        }
    }

    fn machine(self) -> &'static str {
        match self {
            Self::Amd64 => "q35,accel=kvm",
            Self::Arm64 => "virt,accel=kvm,gic-version=3",
        }
    }
}

fn run(command: &mut Command) -> Result<()> {
    let status = command.status()?;
    if !status.success() {
        return Err(format!(
            "{} failed: {status}",
            command.get_program().to_string_lossy()
        )
        .into());
    }
    Ok(())
}

fn input(path: &str) -> Result<PathBuf> {
    let path = fs::canonicalize(path)?;
    if !path.is_file() {
        return Err(format!("{} is not a file", path.display()).into());
    }
    Ok(path)
}

fn password() -> Result<String> {
    let mut bytes = [0u8; 24];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let mut password = String::from("Fc1!");
    for byte in bytes {
        use std::fmt::Write;
        write!(password, "{byte:02x}")?;
    }
    Ok(password)
}

fn prepare(output: &Path, image_index: u32, architecture: Architecture) -> Result<()> {
    // Refuse existing directories: the generated disk is the installer's wipe target.
    DirBuilder::new().mode(0o700).create(output)?;
    let password = password()?;
    let (driver_os, ems_settings, local_accounts) = match architecture {
        Architecture::Amd64 => ("2k25", "emsport:1 emsbaudrate:115200", String::new()),
        Architecture::Arm64 => (
            "w11",
            "bios",
            format!(
                r#"<LocalAccounts><LocalAccount xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State" wcm:action="add"><Name>FcAdmin</Name><Group>Administrators</Group><Password><Value>{password}</Value><PlainText>true</PlainText></Password></LocalAccount></LocalAccounts>"#
            ),
        ),
    };
    let answer = include_str!("Autounattend.xml.in")
        .replace("@ARCH@", architecture.name())
        .replace("@DRIVER_OS@", driver_os)
        .replace("@EMS_SETTINGS@", ems_settings)
        .replace("@LOCAL_ACCOUNTS@", &local_accounts)
        .replace("@IMAGE_INDEX@", &image_index.to_string())
        .replace("@ADMIN_PASSWORD@", &password);
    fs::create_dir(output.join("answer"))?;
    fs::write(output.join("answer/Autounattend.xml"), answer)?;
    fs::write(output.join("administrator-password"), password)?;
    run(Command::new("xorriso")
        .args(["-as", "mkisofs", "-J", "-r", "-V", "FCANSWER", "-o"])
        .arg(output.join("answer.iso"))
        .arg(output.join("answer")))?;
    run(Command::new("qemu-img")
        .args(["create", "-f", "raw"])
        .arg(output.join("windows.raw"))
        .arg("64G"))
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let [
        architecture,
        output,
        index,
        windows_iso,
        virtio_iso,
        ovmf_code,
        ovmf_vars,
    ] = args.as_slice()
    else {
        return Err(
            "usage: prepare ARCH OUTPUT_DIR IMAGE_INDEX WINDOWS_ISO VIRTIO_ISO OVMF_CODE OVMF_VARS"
                .into(),
        );
    };
    let architecture = match architecture.as_str() {
        "amd64" => Architecture::Amd64,
        "arm64" => Architecture::Arm64,
        _ => return Err("ARCH must be amd64 or arm64".into()),
    };
    let index: u32 = index.parse()?;
    if index == 0 {
        return Err("IMAGE_INDEX must be positive".into());
    }
    let windows_iso = input(windows_iso)?;
    let virtio_iso = input(virtio_iso)?;
    let ovmf_code = input(ovmf_code)?;
    let ovmf_vars = input(ovmf_vars)?;
    let output = Path::new(output);
    prepare(output, index, architecture)?;
    let output = fs::canonicalize(output)?;
    fs::copy(ovmf_vars, output.join("OVMF_VARS.fd"))?;

    // QEMU's -drive option uses commas as separators; double commas escape paths.
    let drive_path = |path: &Path| path.to_string_lossy().replace(',', ",,");
    let mut qemu_args = vec![
        "-machine".into(),
        architecture.machine().into(),
        "-cpu".into(),
        "host".into(),
        "-smp".into(),
        "2".into(),
        "-m".into(),
        "4096".into(),
        "-drive".into(),
        format!(
            "if=pflash,format=raw,readonly=on,file={}",
            drive_path(&ovmf_code)
        ),
        "-drive".into(),
        format!(
            "if=pflash,format=raw,file={}",
            drive_path(&output.join("OVMF_VARS.fd"))
        ),
        "-drive".into(),
        format!(
            "if=none,id=os,format=raw,file={}",
            drive_path(&output.join("windows.raw"))
        ),
        "-device".into(),
        "virtio-blk-pci,drive=os,disable-legacy=on,bootindex=1".into(),
        "-netdev".into(),
        "user,id=net0".into(),
        "-device".into(),
        "virtio-net-pci,netdev=net0,disable-legacy=on".into(),
        "-display".into(),
        "none".into(),
        "-serial".into(),
        format!("file:{}", output.join("serial.log").display()),
        "-monitor".into(),
        format!(
            "unix:{},server=on,wait=off",
            output.join("monitor.sock").display()
        ),
    ];
    if matches!(architecture, Architecture::Amd64) {
        qemu_args.extend(["-boot".into(), "once=d".into()]);
    }
    if matches!(architecture, Architecture::Arm64) {
        qemu_args.extend([
            "-device".into(),
            "qemu-xhci,id=usb".into(),
            "-device".into(),
            "usb-kbd,bus=usb.0".into(),
            "-device".into(),
            "ramfb".into(),
        ]);
    }
    for (index, path) in [&windows_iso, &virtio_iso, &output.join("answer.iso")]
        .into_iter()
        .enumerate()
    {
        let cdrom = match architecture {
            Architecture::Amd64 => format!(
                "file={},media=cdrom,index={index},readonly=on",
                drive_path(path)
            ),
            Architecture::Arm64 => format!(
                "if=none,id=cd{index},file={},format=raw,media=cdrom,readonly=on",
                drive_path(path)
            ),
        };
        qemu_args.extend(["-drive".into(), cdrom]);
        if matches!(architecture, Architecture::Arm64) {
            let boot = if index == 0 { ",bootindex=2" } else { "" };
            qemu_args.extend([
                "-device".into(),
                format!("usb-storage,bus=usb.0,drive=cd{index}{boot}"),
            ]);
        }
    }
    let mut script = File::create(output.join("install.sh"))?;
    writeln!(
        script,
        "#!/bin/sh\nset -eu\nexec {} \\",
        architecture.emulator()
    )?;
    for (i, arg) in qemu_args.iter().enumerate() {
        let suffix = if i + 1 == qemu_args.len() { "" } else { " \\" };
        writeln!(script, "  '{}'{}", arg.replace('\'', "'\\''"), suffix)?;
    }
    println!("Prepared {}. No VM was started.", output.display());
    println!("Installation command: sh {}/install.sh", output.display());
    Ok(())
}
