// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Rebuild pinned Windows UDF installation media with a selectable viostor driver.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

mod media;
use media::{OutputDirectory, Result, boot_catalog, checked as run, digest, verify};

fn manifest(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<PathBuf, (u64, String)>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            manifest(root, &path, files)?;
        } else if metadata.is_file() && !entry.file_type()?.is_symlink() {
            files.insert(
                path.strip_prefix(root)?.to_owned(),
                (metadata.len(), digest(&path)?),
            );
        } else {
            return Err("unexpected non-regular media file".into());
        }
    }
    Ok(())
}

fn extract(source: &Path, destination: &Path, members: &[&str]) -> Result<()> {
    fs::create_dir(destination)?;
    run(Command::new("7z")
        .args(["x", "-y", "-bd"])
        .arg(format!("-o{}", destination.display()))
        .arg(source)
        .args(members))
}

fn verify_boot_images(source: &Path, tree: &Path) -> Result<()> {
    let mut iso = File::open(source)?;
    let descriptor = boot_catalog(&mut iso)?;
    if descriptor[..2] != [1, 0]
        || descriptor[32..34] != [0x88, 0]
        || descriptor[64..68] != [0x91, 0xef, 1, 0]
        || descriptor[96..98] != [0x88, 0]
    {
        return Err("unexpected BIOS/EFI boot catalogue layout".into());
    }
    for (entry, name) in [
        (32, "boot/etfsboot.com"),
        (96, "efi/microsoft/boot/efisys.bin"),
    ] {
        let lba = u64::from(u32::from_le_bytes(
            descriptor[entry + 8..entry + 12].try_into()?,
        ));
        let bytes = fs::read(tree.join(name))?;
        let mut original = vec![0; bytes.len()];
        iso.seek(SeekFrom::Start(lba * 2048))?;
        iso.read_exact(&mut original)?;
        if original != bytes {
            return Err(
                format!("boot image differs from original El Torito extent: {name}").into(),
            );
        }
    }
    Ok(())
}

fn prepare_2023_boot(tree: &Path, output: &Path) -> Result<()> {
    let extracted = output.join("boot-2023");
    extract(
        &tree.join("sources/boot.wim"),
        &extracted,
        &[
            "1/Windows/Boot/EFI_EX/*",
            "1/Windows/Boot/DVD_EX/*",
            "1/Windows/Boot/Fonts_EX/*",
            "1/Windows/Boot/EFI/boot.stl",
        ],
    )?;
    let boot = extracted.join("1/Windows/Boot");
    let mut replacements = vec![
        (
            "EFI_EX/bootmgfw_EX.efi".to_owned(),
            "efi/boot/bootx64.efi".to_owned(),
        ),
        (
            "DVD_EX/EFI/en-US/efisys_EX.bin".to_owned(),
            "efi/microsoft/boot/efisys_ex.bin".to_owned(),
        ),
        (
            "EFI/boot.stl".to_owned(),
            "efi/microsoft/boot/boot.stl".to_owned(),
        ),
    ];
    if boot.join("EFI_EX/bootmgr_EX.efi").exists() {
        replacements.push(("EFI_EX/bootmgr_EX.efi".to_owned(), "bootmgr.efi".to_owned()));
    }
    for entry in fs::read_dir(boot.join("Fonts_EX"))? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "invalid boot font name")?;
        if !entry.file_type()?.is_file() || !name.ends_with("_EX.ttf") {
            return Err("unexpected boot font entry".into());
        }
        replacements.push((
            format!("Fonts_EX/{name}"),
            format!("efi/microsoft/boot/fonts/{}", name.replace("_EX", "")),
        ));
    }
    replacements.sort();
    let mut evidence = String::new();
    for (source, destination) in replacements {
        fs::copy(boot.join(&source), tree.join(&destination))?;
        evidence.push_str(&format!(
            "{} sources/boot.wim:1/Windows/Boot/{source} -> {destination}\n",
            digest(&boot.join(&source))?
        ));
    }
    fs::write(output.join("boot-replacements.sha256"), evidence)?;
    fs::remove_dir_all(extracted)?;
    Ok(())
}

fn profile_config(profile: Option<&str>) -> Result<(&'static str, &'static str, &'static str)> {
    Ok(match profile {
        Some("win7") => (
            "windows7",
            "ee69f3e9b86ff973f632db8e01700c5724ef78420b175d25bae6ead90f6805a7",
            "w7",
        ),
        Some("win10") => (
            "windows10",
            "c90a6df8997bf49e56b9673982f3e80745058723a707aef8f22998ae6479597d",
            "w10",
        ),
        Some("win11") => (
            "windows11",
            "4f59662a96fc1da48c1b415d6c369d08af55ddd64e8f1c84e0166d9e50405d7a",
            "w11",
        ),
        Some("win11-enterprise-26h2") => (
            "windows11-enterprise-26h2",
            "bc3f24086ebadc94489066b5ad78089e2cf5c3491e90e790bb81a2b199c10e38",
            "w11",
        ),
        Some("server2025") => (
            "server2025",
            "7b052573ba7894c9924e3e87ba732ccd354d18cb75a883efa9b900ea125bfd51",
            "2k25",
        ),
        _ => {
            return Err(
                "profile must be win7, win10, win11, win11-enterprise-26h2 or server2025".into(),
            );
        }
    })
}

fn read_answer(path: &Path) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    File::open(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut data)?;
    if data.len() > 1024 * 1024 {
        return Err("answer file exceeds 1 MiB".into());
    }
    Ok(data)
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [profile, source, drivers, output, extra @ ..] = args.as_slice() else {
        return Err(
            "usage: prepare-nt6 win7|win10|win11|win11-enterprise-26h2|server2025 WINDOWS_ISO VIRTIO_DRIVER_ISO NEW_OUTPUT_DIRECTORY [ANSWER_XML]".into(),
        );
    };
    if extra.len() > 1 {
        return Err("at most one answer file is allowed".into());
    }
    let answer = extra.first().map(|path| read_answer(path)).transpose()?;
    let (name, hash, driver) = profile_config(profile.to_str())?;
    for (path, expected) in [
        (source, hash),
        (
            drivers,
            "303f7ae40dad495d6ae474fdc571df58958a4dbc5c37a522d80f9a203867949d",
        ),
    ] {
        verify(path, expected)?;
    }
    let output_guard = OutputDirectory::create(output)?;
    let tree = output.join("tree");
    extract(source, &tree, &[])?;
    verify_boot_images(source, &tree)?;
    let secure_boot = profile == Path::new("win11-enterprise-26h2");
    if secure_boot {
        prepare_2023_boot(&tree, output)?;
    }
    let driver_tree = output.join("driver-tree");
    let driver_path = format!("viostor/{driver}/amd64");
    extract(drivers, &driver_tree, &[&format!("{driver_path}/*")])?;
    let destination = tree.join("Drivers/viostor");
    fs::create_dir_all(&destination)?;
    for name in ["viostor.inf", "viostor.sys", "viostor.cat"] {
        fs::copy(
            driver_tree.join(&driver_path).join(name),
            destination.join(name),
        )?;
    }
    if let Some(answer) = answer {
        if tree.join("Autounattend.xml").exists() {
            return Err("source media already contains Autounattend.xml".into());
        }
        fs::write(tree.join("Autounattend.xml"), answer)?;
        let network = format!("NetKVM/{driver}/amd64");
        let network_tree = output.join("network-driver-tree");
        extract(drivers, &network_tree, &[&format!("{network}/*")])?;
        let destination = tree.join("Drivers/NetKVM");
        fs::create_dir(&destination)?;
        for entry in fs::read_dir(network_tree.join(network))? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                return Err("unexpected network driver entry".into());
            }
            fs::copy(entry.path(), destination.join(entry.file_name()))?;
        }
        fs::remove_dir_all(network_tree)?;
    }
    let mut expected = BTreeMap::new();
    manifest(&tree, &tree, &mut expected)?;
    let partial = output.join(format!("{name}-viostor.partial.iso"));
    run(Command::new("genisoimage")
        .args([
            "-udf",
            "-allow-limited-size",
            "-iso-level",
            "3",
            "-J",
            "-joliet-long",
            "-V",
            "WINDOWS_INSTALL",
            "-b",
            "boot/etfsboot.com",
            "-no-emul-boot",
            "-boot-load-size",
            "8",
            "-eltorito-alt-boot",
            "-e",
            if secure_boot {
                "efi/microsoft/boot/efisys_ex.bin"
            } else {
                "efi/microsoft/boot/efisys.bin"
            },
            "-no-emul-boot",
            "-o",
        ])
        .arg(&partial)
        .arg(&tree))?;
    let verify = output.join("verify");
    extract(&partial, &verify, &[])?;
    let mut actual = BTreeMap::new();
    manifest(&verify, &verify, &mut actual)?;
    for (path, identity) in &expected {
        if actual.get(path) != Some(identity) {
            return Err(format!("rebuilt UDF file differs: {}", path.display()).into());
        }
    }
    let evidence: String = expected
        .iter()
        .map(|(path, (size, hash))| format!("{hash} {size} {}\n", path.display()))
        .collect();
    fs::write(output.join("files.sha256-sizes"), evidence)?;
    let result = output.join(format!("{name}-viostor.iso"));
    fs::rename(partial, &result)?;
    fs::write(
        output.join("image.sha256"),
        format!("{}  {name}-viostor.iso\n", digest(&result)?),
    )?;
    for directory in [tree, driver_tree, verify] {
        fs::remove_dir_all(directory)?;
    }
    println!(
        "Verified {} prepared payload files: {}",
        expected.len(),
        result.display()
    );
    output_guard.finish();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_profiles_select_matching_driver_and_pin_source() {
        for (profile, driver) in [
            ("win7", "w7"),
            ("win10", "w10"),
            ("win11", "w11"),
            ("win11-enterprise-26h2", "w11"),
            ("server2025", "2k25"),
        ] {
            let (_, hash, selected) = profile_config(Some(profile)).unwrap();
            assert_eq!(selected, driver);
            assert_eq!(hash.len(), 64);
            assert!(hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
        assert!(profile_config(Some("windows-server-latest.iso")).is_err());
        assert!(profile_config(None).is_err());
    }
}
