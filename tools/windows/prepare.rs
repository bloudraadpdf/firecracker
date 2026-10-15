// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Prepare an isolated AMD64 Windows Server installation disk and answer ISO.

use std::env;
use std::error::Error;
use std::fs::{self, DirBuilder, File};
use std::io::{Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

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

fn prepare(output: &Path, image_index: u32) -> Result<()> {
    // Refuse existing directories: the generated disk is the installer's wipe target.
    DirBuilder::new().mode(0o700).create(output)?;
    let password = password()?;
    let answer = include_str!("Autounattend.xml.in")
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
    let [output, index, windows_iso, virtio_iso, ovmf_code, ovmf_vars] = args.as_slice() else {
        return Err(
            "usage: prepare OUTPUT_DIR IMAGE_INDEX WINDOWS_ISO VIRTIO_ISO OVMF_CODE OVMF_VARS"
                .into(),
        );
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
    prepare(output, index)?;
    let output = fs::canonicalize(output)?;
    fs::copy(ovmf_vars, output.join("OVMF_VARS.fd"))?;

    // QEMU's -drive option uses commas as separators; double commas escape paths.
    let drive_path = |path: &Path| path.to_string_lossy().replace(',', ",,");
    let qemu_args = vec![
        "-machine".into(),
        "q35,accel=kvm".into(),
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
            "file={},media=cdrom,index=0,readonly=on",
            drive_path(&windows_iso)
        ),
        "-drive".into(),
        format!(
            "file={},media=cdrom,index=1,readonly=on",
            drive_path(&virtio_iso)
        ),
        "-drive".into(),
        format!(
            "file={},media=cdrom,index=2,readonly=on",
            drive_path(&output.join("answer.iso"))
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
        "-boot".into(),
        "once=d".into(),
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
    let mut script = File::create(output.join("install.sh"))?;
    writeln!(script, "#!/bin/sh\nset -eu\nexec qemu-system-x86_64 \\")?;
    for (i, arg) in qemu_args.iter().enumerate() {
        let suffix = if i + 1 == qemu_args.len() { "" } else { " \\" };
        writeln!(script, "  '{}'{}", arg.replace('\'', "'\\''"), suffix)?;
    }
    println!("Prepared {}. No VM was started.", output.display());
    println!("Installation command: sh {}/install.sh", output.display());
    Ok(())
}
