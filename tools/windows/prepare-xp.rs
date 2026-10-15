// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Add the pinned XP viostor driver to installation media without altering the source ISO.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

mod media;
use media::{OutputDirectory, Result, boot_catalog, checked, verify};
const ISO_HASH: &str = "fd8c8d42c1581e8767217fe800bfc0d5649c0ad20d754c927d6c763e446d1927";
const DRIVER_HASH: &str = "eed092cb9538ca180ebdbea591ad6a8e5fafe646cf4261ab0f71a62952d136f9";

fn textmode_driver(source: &[u8]) -> Result<Vec<u8>> {
    let mut sections = BTreeMap::from([
        ("[SCSI.Load]", "viostor = viostor.sys,4"),
        (
            "[SourceDisksFiles.x86]",
            "viostor.sys = 1,,,,,,4_,4,1,,,1,4",
        ),
        ("[SCSI]", "viostor = \"Red Hat VirtIO SCSI Disk Device\""),
        (
            "[HardwareIdsDatabase]",
            "PCI\\VEN_1AF4&DEV_1001&SUBSYS_00021AF4 = \"viostor\"",
        ),
    ]);
    let mut output = Vec::with_capacity(source.len() + 256);
    for line in source.split_inclusive(|byte| *byte == b'\n') {
        output.extend_from_slice(line);
        if let Some(entry) = sections.remove(String::from_utf8_lossy(line).trim()) {
            output.extend_from_slice(entry.as_bytes());
            output.extend_from_slice(b"\r\n");
        }
    }
    if !sections.is_empty() {
        return Err("XP text-mode setup sections missing".into());
    }
    Ok(output)
}

fn boot_image(iso: &mut (impl Read + Seek)) -> Result<Vec<u8>> {
    let descriptor = boot_catalog(iso)?;
    if descriptor[1] != 0 || descriptor[32..34] != [0x88, 0] {
        return Err("expected a BIOS, no-emulation El Torito boot entry".into());
    }
    let sectors = u16::from_le_bytes(descriptor[38..40].try_into()?);
    if sectors != 4 {
        return Err("expected the pinned XP 2048-byte boot image".into());
    }
    let image = u32::from_le_bytes(descriptor[40..44].try_into()?);
    iso.seek(SeekFrom::Start(u64::from(image) * 2048))?;
    let mut bytes = vec![0; 2048];
    iso.read_exact(&mut bytes)?;
    Ok(bytes)
}

// The pinned XP boot loader reads I386 at 0x10000; the VGA aperture starts at 0xa0000.
const DIRECTORY_LIMIT: u32 = 0x90000;

struct DirectoryEntry {
    name: Vec<u8>,
    sector: u32,
    size: u32,
    directory: bool,
}

fn read_directory(
    iso: &mut (impl Read + Seek),
    sector: u32,
    size: u32,
) -> Result<Vec<DirectoryEntry>> {
    if size == 0 || size > DIRECTORY_LIMIT {
        return Err("XP ISO directory would overwrite VGA/firmware memory".into());
    }
    let mut directory = vec![0; size as usize];
    iso.seek(SeekFrom::Start(u64::from(sector) * 2048))?;
    iso.read_exact(&mut directory)?;
    let mut entries = Vec::new();
    let mut offset = 0;
    while offset < directory.len() {
        let length = usize::from(directory[offset]);
        if length == 0 {
            offset = (offset / 2048 + 1) * 2048;
            continue;
        }
        let record = directory
            .get(offset..offset + length)
            .ok_or("truncated ISO directory")?;
        if record.len() < 34 {
            return Err("short ISO directory record".into());
        }
        entries.push(DirectoryEntry {
            name: record
                .get(33..33 + usize::from(record[32]))
                .ok_or("truncated ISO filename")?
                .to_vec(),
            sector: u32::from_le_bytes(record[2..6].try_into()?),
            size: u32::from_le_bytes(record[10..14].try_into()?),
            directory: record[25] & 2 != 0,
        });
        offset += length;
    }
    Ok(entries)
}

fn primary_root(iso: &mut (impl Read + Seek)) -> Result<Vec<DirectoryEntry>> {
    let mut descriptor = [0; 2048];
    iso.seek(SeekFrom::Start(16 * 2048))?;
    iso.read_exact(&mut descriptor)?;
    if descriptor[..7] != [1, b'C', b'D', b'0', b'0', b'1', 1] {
        return Err("invalid primary ISO descriptor".into());
    }
    read_directory(
        iso,
        u32::from_le_bytes(descriptor[158..162].try_into()?),
        u32::from_le_bytes(descriptor[166..170].try_into()?),
    )
}

fn setup_directory(root: &[DirectoryEntry]) -> Result<&DirectoryEntry> {
    let entry = root
        .iter()
        .find(|entry| entry.name == b"I386" && entry.directory)
        .ok_or("primary ISO directory lacks I386")?;
    if entry.size == 0 || entry.size > DIRECTORY_LIMIT {
        return Err("XP I386 directory would overwrite VGA/firmware memory".into());
    }
    Ok(entry)
}

fn i386_directory_size(iso: &mut (impl Read + Seek)) -> Result<u32> {
    Ok(setup_directory(&primary_root(iso)?)?.size)
}

fn validate_setup_names(iso: &mut (impl Read + Seek)) -> Result<()> {
    let root = primary_root(iso)?;
    for name in [b"WIN51".as_slice(), b"WIN51IP", b"WIN51IP.SP3"] {
        if !root
            .iter()
            .any(|entry| entry.name == name && !entry.directory)
        {
            return Err("XP primary root filenames changed".into());
        }
    }
    let setup = setup_directory(&root)?;
    let children = read_directory(iso, setup.sector, setup.size)?;
    for name in [b"SETUPLDR.BIN".as_slice(), b"NTDETECT.COM", b"TXTSETUP.SIF"] {
        if !children
            .iter()
            .any(|entry| entry.name == name && !entry.directory)
        {
            return Err("XP boot loader requires exact unversioned primary filenames".into());
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 3 {
        return Err("usage: prepare-xp ORIGINAL_ISO VIRTIO_TAR_XZ NEW_OUTPUT_DIRECTORY".into());
    }
    let iso = fs::canonicalize(&arguments[0])?;
    let archive = fs::canonicalize(&arguments[1])?;
    verify(&iso, ISO_HASH)?;
    verify(&archive, DRIVER_HASH)?;
    let boot = boot_image(&mut File::open(&iso)?)?;
    validate_setup_names(&mut File::open(&iso)?)?;
    let output_guard = OutputDirectory::create(std::path::Path::new(&arguments[2]))?;
    let output = fs::canonicalize(&arguments[2])?;
    let staging = output.join("staging");
    fs::create_dir(&staging)?;
    checked(
        Command::new("tar")
            .arg("-xf")
            .arg(&archive)
            .arg("-C")
            .arg(&staging)
            .arg("./viostor/xp/x86"),
    )?;
    checked(
        Command::new("xorriso")
            .args(["-osirrox", "on", "-indev"])
            .arg(&iso)
            .arg("-extract")
            .arg("/I386/TXTSETUP.SIF")
            .arg(staging.join("TXTSETUP.SIF")),
    )?;
    fs::set_permissions(
        staging.join("TXTSETUP.SIF"),
        fs::Permissions::from_mode(0o600),
    )?;
    fs::write(
        staging.join("TXTSETUP.SIF"),
        textmode_driver(&fs::read(staging.join("TXTSETUP.SIF"))?)?,
    )?;
    fs::write(staging.join("BOOT.IMG"), &boot)?;
    fs::write(staging.join("WINNT.SIF"), b"[Data]\r\nAutoPartition=0\r\nMsDosInitiated=0\r\nUnattendedInstall=Yes\r\n\r\n[Unattended]\r\nUnattendMode=ProvideDefault\r\nOemPreinstall=Yes\r\nOemPnPDriversPath=Drivers\\viostor\r\n")?;
    let driver = staging.join("viostor/xp/x86");
    checked(
        Command::new("xorriso")
            .arg("-indev")
            .arg(&iso)
            .arg("-outdev")
            .arg(output.join("xp-viostor.partial.iso"))
            .args([
                "-boot_image",
                "any",
                "discard",
                "-joliet",
                "on",
                "-rockridge",
                "off",
                "-compliance",
                "iso_9660_level=2:omit_version:no_force_dots:no_j_force_dots",
            ])
            .arg("-map")
            .arg(staging.join("TXTSETUP.SIF"))
            .arg("/I386/TXTSETUP.SIF")
            .arg("-map")
            .arg(staging.join("WINNT.SIF"))
            .arg("/I386/WINNT.SIF")
            .arg("-map")
            .arg(driver.join("viostor.sys"))
            .arg("/I386/VIOSTOR.SYS")
            .arg("-map")
            .arg(&driver)
            .arg("/$OEM$/$1/Drivers/viostor")
            .arg("-map")
            .arg(staging.join("BOOT.IMG"))
            .arg("/BOOT.IMG")
            .args([
                "-boot_image",
                "any",
                "bin_path=/BOOT.IMG",
                "-boot_image",
                "any",
                "cat_path=/BOOT.CAT",
                "-boot_image",
                "any",
                "emul_type=no_emulation",
                "-boot_image",
                "any",
                "load_size=2048",
                "-commit",
            ]),
    )?;
    let provisional = output.join("xp-viostor.partial.iso");
    let validation = (|| -> Result<()> {
        let mut image = File::open(&provisional)?;
        let directory_size = i386_directory_size(&mut image)?;
        validate_setup_names(&mut image)?;
        if boot_image(&mut image)? != boot {
            return Err("output boot image changed".into());
        }
        println!("Validated primary I386 directory: {directory_size} bytes");
        Ok(())
    })();
    if let Err(error) = validation {
        fs::remove_file(provisional)?;
        return Err(error);
    }
    fs::rename(provisional, output.join("xp-viostor.iso"))?;
    fs::remove_dir_all(staging)?;
    println!("{}", output.join("xp-viostor.iso").display());
    output_guard.finish();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textmode_setup_preserves_source_and_dos_eof() {
        let source =
            b"[SCSI.Load]\r\n[SCSI]\r\n[SourceDisksFiles.x86]\r\n[HardwareIdsDatabase]\r\n\x1a";
        let output = textmode_driver(source).unwrap();
        assert_eq!(output.last(), Some(&0x1a));
        assert!(
            output
                .windows(b"SUBSYS_00021AF4 = ".len())
                .any(|part| part == b"SUBSYS_00021AF4 = ")
        );
        assert!(textmode_driver(b"[SCSI.Load]\r\n").is_err());
    }

    #[test]
    fn boot_catalog_requires_no_emulation_and_valid_checksum() {
        let mut bytes = vec![0u8; 20 * 2048];
        let descriptor = &mut bytes[16 * 2048..17 * 2048];
        descriptor[1..6].copy_from_slice(b"CD001");
        descriptor[7..30].copy_from_slice(b"EL TORITO SPECIFICATION");
        descriptor[71..75].copy_from_slice(&18u32.to_le_bytes());
        let catalog = &mut bytes[18 * 2048..19 * 2048];
        catalog[0] = 1;
        catalog[30..34].copy_from_slice(&[0x55, 0xaa, 0x88, 0]);
        catalog[28..30].copy_from_slice(&0x55aau16.to_le_bytes());
        catalog[38..40].copy_from_slice(&4u16.to_le_bytes());
        catalog[40..44].copy_from_slice(&19u32.to_le_bytes());
        bytes[19 * 2048..].fill(42);
        assert_eq!(
            boot_image(&mut std::io::Cursor::new(&bytes)).unwrap(),
            vec![42; 2048]
        );
        bytes[18 * 2048 + 33] = 1;
        assert!(boot_image(&mut std::io::Cursor::new(&bytes)).is_err());
    }

    #[test]
    fn rejects_i386_directory_that_overwrites_the_vga_aperture() {
        let mut bytes = vec![0u8; 19 * 2048];
        let descriptor = &mut bytes[16 * 2048..17 * 2048];
        descriptor[..7].copy_from_slice(&[1, b'C', b'D', b'0', b'0', b'1', 1]);
        descriptor[158..162].copy_from_slice(&18u32.to_le_bytes());
        descriptor[166..170].copy_from_slice(&2048u32.to_le_bytes());
        let record = &mut bytes[18 * 2048..18 * 2048 + 38];
        record[0] = 38;
        record[25] = 2;
        record[32] = 4;
        record[33..37].copy_from_slice(b"I386");
        record[10..14].copy_from_slice(&263642u32.to_le_bytes());
        assert_eq!(
            i386_directory_size(&mut std::io::Cursor::new(&bytes)).unwrap(),
            263642
        );
        bytes[18 * 2048 + 10..18 * 2048 + 14].copy_from_slice(&753664u32.to_le_bytes());
        assert!(i386_directory_size(&mut std::io::Cursor::new(&bytes)).is_err());
    }

    #[test]
    fn loader_names_must_not_gain_versions_or_dots() {
        fn record(name: &[u8], sector: u32, size: u32, directory: bool) -> Vec<u8> {
            let mut record = vec![0; (33 + name.len() + 1) & !1];
            record[0] = record.len() as u8;
            record[2..6].copy_from_slice(&sector.to_le_bytes());
            record[10..14].copy_from_slice(&size.to_le_bytes());
            record[25] = if directory { 2 } else { 0 };
            record[32] = name.len() as u8;
            record[33..33 + name.len()].copy_from_slice(name);
            record
        }
        let mut bytes = vec![0u8; 20 * 2048];
        bytes[16 * 2048..16 * 2048 + 7].copy_from_slice(&[1, b'C', b'D', b'0', b'0', b'1', 1]);
        bytes[16 * 2048 + 158..16 * 2048 + 162].copy_from_slice(&18u32.to_le_bytes());
        bytes[16 * 2048 + 166..16 * 2048 + 170].copy_from_slice(&2048u32.to_le_bytes());
        let mut root = record(b"I386", 19, 2048, true);
        for name in [b"WIN51".as_slice(), b"WIN51IP", b"WIN51IP.SP3"] {
            root.extend(record(name, 0, 1, false));
        }
        bytes[18 * 2048..18 * 2048 + root.len()].copy_from_slice(&root);
        let mut children = Vec::new();
        for name in [b"SETUPLDR.BIN".as_slice(), b"NTDETECT.COM", b"TXTSETUP.SIF"] {
            children.extend(record(name, 0, 1, false));
        }
        bytes[19 * 2048..19 * 2048 + children.len()].copy_from_slice(&children);
        assert!(validate_setup_names(&mut std::io::Cursor::new(&bytes)).is_ok());
        let mut changed = bytes.clone();
        changed[19 * 2048 + 33 + 10] = b';';
        assert!(validate_setup_names(&mut std::io::Cursor::new(changed)).is_err());
        let win51 = 18 * 2048 + usize::from(bytes[18 * 2048]);
        bytes[win51 + 33 + 4] = b'.';
        assert!(validate_setup_names(&mut std::io::Cursor::new(bytes)).is_err());
    }
}
