// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Prepare pinned AMD64 installation media or stage ARM64 media for servicing.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

mod media;
use media::{OutputDirectory, Result, boot_catalog, checked as run, digest, verify};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Architecture {
    Amd64,
    Arm64,
}

impl Architecture {
    fn directory(self) -> &'static str {
        match self {
            Self::Amd64 => "amd64",
            Self::Arm64 => "ARM64",
        }
    }
}

fn verify_arm_boot(tree: &Path) -> Result<()> {
    let mut loader = File::open(tree.join("efi/boot/bootaa64.efi"))?;
    let mut header = [0; 64];
    loader.read_exact(&mut header)?;
    if header[..2] != *b"MZ" {
        return Err("ARM64 fallback loader is not a PE image".into());
    }
    loader.seek(SeekFrom::Start(u64::from(u32::from_le_bytes(
        header[60..64].try_into()?,
    ))))?;
    let mut pe = [0; 6];
    loader.read_exact(&mut pe)?;
    if pe[..4] != *b"PE\0\0" || u16::from_le_bytes(pe[4..6].try_into()?) != 0xaa64 {
        return Err("fallback loader must target ARM64".into());
    }
    Ok(())
}

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

fn prepare_2023_boot(tree: &Path, output: &Path, unattended_pro: bool) -> Result<()> {
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
            if unattended_pro {
                "DVD_EX/EFI/en-US/efisys_noprompt_EX.bin"
            } else {
                "DVD_EX/EFI/en-US/efisys_EX.bin"
            }
            .to_owned(),
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

fn profile_config(
    profile: Option<&str>,
) -> Result<(&'static str, &'static str, &'static str, bool, Architecture)> {
    Ok(match profile {
        Some("win7") => (
            "windows7",
            "ee69f3e9b86ff973f632db8e01700c5724ef78420b175d25bae6ead90f6805a7",
            "w7",
            false,
            Architecture::Amd64,
        ),
        Some("win10") => (
            "windows10",
            "c90a6df8997bf49e56b9673982f3e80745058723a707aef8f22998ae6479597d",
            "w10",
            false,
            Architecture::Amd64,
        ),
        Some("win11") => (
            "windows11",
            "4f59662a96fc1da48c1b415d6c369d08af55ddd64e8f1c84e0166d9e50405d7a",
            "w11",
            false,
            Architecture::Amd64,
        ),
        Some("win11-enterprise-26h2") => (
            "windows11-enterprise-26h2",
            "bc3f24086ebadc94489066b5ad78089e2cf5c3491e90e790bb81a2b199c10e38",
            "w11",
            true,
            Architecture::Amd64,
        ),
        Some("win11-pro-26h2") => (
            "windows11-pro-26h2",
            "bd4307df32bc8af33b39ccecb1174aeb345386630f89a2b86c7a4e36b55ea650",
            "w11",
            true,
            Architecture::Amd64,
        ),
        Some("win11-iot-arm64") => (
            "windows11-iot-arm64",
            "f8f068cdc90c894a55d8c8530db7c193234ba57bb11d33b71383839ac41246b4",
            "w11",
            false,
            Architecture::Arm64,
        ),
        Some("server2025") => (
            "server2025",
            "7b052573ba7894c9924e3e87ba732ccd354d18cb75a883efa9b900ea125bfd51",
            "2k25",
            false,
            Architecture::Amd64,
        ),
        _ => {
            return Err(
                "profile must be win7, win10, win11, win11-enterprise-26h2, win11-pro-26h2, \
                 win11-iot-arm64 or server2025"
                    .into(),
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

fn prepare_install_files(
    drivers: &Path,
    driver: &str,
    architecture: Architecture,
    output: &Path,
    tree: &Path,
    answer: Option<Vec<u8>>,
) -> Result<()> {
    let automatic = tree.join("$WinPEDriver$");
    fs::create_dir(&automatic)?;
    let driver_tree = output.join("driver-tree");
    let driver_path = format!("viostor/{driver}/{}", architecture.directory());
    extract(drivers, &driver_tree, &[&format!("{driver_path}/*")])?;
    let destination = automatic.join("viostor");
    fs::create_dir(&destination)?;
    for name in ["viostor.inf", "viostor.sys", "viostor.cat"] {
        fs::copy(
            driver_tree.join(&driver_path).join(name),
            destination.join(name),
        )?;
    }
    fs::remove_dir_all(driver_tree)?;
    let components: &[&str] = match architecture {
        Architecture::Amd64 => &["NetKVM"],
        Architecture::Arm64 => &["NetKVM", "vioinput", "viosock"],
    };
    for component in components {
        let package = format!("{component}/{driver}/{}", architecture.directory());
        let extracted = output.join("component-driver-tree");
        extract(drivers, &extracted, &[&format!("{package}/*")])?;
        let destination = automatic.join(component);
        fs::create_dir(&destination)?;
        for entry in fs::read_dir(extracted.join(package))? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                return Err("unexpected driver package entry".into());
            }
            if entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdb"))
            {
                continue;
            }
            fs::copy(entry.path(), destination.join(entry.file_name()))?;
        }
        fs::remove_dir_all(extracted)?;
    }
    if let Some(answer) = answer {
        if tree.join("Autounattend.xml").exists() {
            return Err("source media already contains Autounattend.xml".into());
        }
        fs::write(tree.join("Autounattend.xml"), answer)?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [profile, source, drivers, output, extra @ ..] = args.as_slice() else {
        return Err("usage: prepare-nt6 \
             win7|win10|win11|win11-enterprise-26h2|win11-pro-26h2|win11-iot-arm64|server2025 \
             WINDOWS_ISO VIRTIO_DRIVER_ISO NEW_OUTPUT_DIRECTORY [ANSWER_XML]"
            .into());
    };
    if extra.len() > 1 {
        return Err("at most one answer file is allowed".into());
    }
    let answer = extra.first().map(|path| read_answer(path)).transpose()?;
    let (name, hash, driver, uses_2023_boot, architecture) = profile_config(profile.to_str())?;
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
    match architecture {
        Architecture::Amd64 => verify_boot_images(source, &tree)?,
        Architecture::Arm64 => verify_arm_boot(&tree)?,
    }
    if uses_2023_boot {
        prepare_2023_boot(
            &tree,
            output,
            name == "windows11-pro-26h2" && answer.is_some(),
        )?;
    }
    prepare_install_files(drivers, driver, architecture, output, &tree, answer)?;
    let mut expected = BTreeMap::new();
    manifest(&tree, &tree, &mut expected)?;
    let evidence: String = expected
        .iter()
        .map(|(path, (size, hash))| format!("{hash} {size} {}\n", path.display()))
        .collect();
    fs::write(output.join("files.sha256-sizes"), evidence)?;
    if architecture == Architecture::Arm64 {
        println!(
            "Staged {} ARM64 payload files: {}. WIM servicing and GPT/FAT installer assembly \
             remain required.",
            expected.len(),
            tree.display()
        );
        output_guard.finish();
        return Ok(());
    }
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
            if uses_2023_boot {
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
    let result = output.join(format!("{name}-viostor.iso"));
    fs::rename(partial, &result)?;
    fs::write(
        output.join("image.sha256"),
        format!("{}  {name}-viostor.iso\n", digest(&result)?),
    )?;
    for directory in [tree, verify] {
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
        for (profile, expected) in [
            (
                "win7",
                (
                    "windows7",
                    "ee69f3e9b86ff973f632db8e01700c5724ef78420b175d25bae6ead90f6805a7",
                    "w7",
                    false,
                    Architecture::Amd64,
                ),
            ),
            (
                "win10",
                (
                    "windows10",
                    "c90a6df8997bf49e56b9673982f3e80745058723a707aef8f22998ae6479597d",
                    "w10",
                    false,
                    Architecture::Amd64,
                ),
            ),
            (
                "win11",
                (
                    "windows11",
                    "4f59662a96fc1da48c1b415d6c369d08af55ddd64e8f1c84e0166d9e50405d7a",
                    "w11",
                    false,
                    Architecture::Amd64,
                ),
            ),
            (
                "win11-enterprise-26h2",
                (
                    "windows11-enterprise-26h2",
                    "bc3f24086ebadc94489066b5ad78089e2cf5c3491e90e790bb81a2b199c10e38",
                    "w11",
                    true,
                    Architecture::Amd64,
                ),
            ),
            (
                "win11-pro-26h2",
                (
                    "windows11-pro-26h2",
                    "bd4307df32bc8af33b39ccecb1174aeb345386630f89a2b86c7a4e36b55ea650",
                    "w11",
                    true,
                    Architecture::Amd64,
                ),
            ),
            (
                "win11-iot-arm64",
                (
                    "windows11-iot-arm64",
                    "f8f068cdc90c894a55d8c8530db7c193234ba57bb11d33b71383839ac41246b4",
                    "w11",
                    false,
                    Architecture::Arm64,
                ),
            ),
            (
                "server2025",
                (
                    "server2025",
                    "7b052573ba7894c9924e3e87ba732ccd354d18cb75a883efa9b900ea125bfd51",
                    "2k25",
                    false,
                    Architecture::Amd64,
                ),
            ),
        ] {
            assert_eq!(profile_config(Some(profile)).unwrap(), expected);
        }
        assert!(profile_config(Some("windows-server-latest.iso")).is_err());
        assert!(profile_config(None).is_err());
    }

    #[test]
    fn arm_fallback_loader_rejects_wrong_machine_and_invalid_headers() {
        let root = std::env::current_dir()
            .unwrap()
            .join("build")
            .join(format!("nt6-arm-loader-fixture-{}", std::process::id()));
        let _owner = OutputDirectory::create(&root).unwrap();
        fs::create_dir_all(root.join("efi/boot")).unwrap();
        let path = root.join("efi/boot/bootaa64.efi");
        let mut valid = [0; 70];
        valid[..2].copy_from_slice(b"MZ");
        valid[60..64].copy_from_slice(&64u32.to_le_bytes());
        valid[64..68].copy_from_slice(b"PE\0\0");
        valid[68..70].copy_from_slice(&0xaa64u16.to_le_bytes());
        fs::write(&path, valid).unwrap();
        verify_arm_boot(&root).unwrap();
        for (offset, bytes) in [(0, b"XX".as_slice()), (64, b"PX"), (68, &[0x64, 0x86])] {
            let mut invalid = valid;
            invalid[offset..offset + bytes.len()].copy_from_slice(bytes);
            fs::write(&path, invalid).unwrap();
            assert!(verify_arm_boot(&root).is_err());
        }
        fs::write(&path, &valid[..69]).unwrap();
        assert!(verify_arm_boot(&root).is_err());
    }

    #[test]
    fn original_2023_cd_image_matches_unattended_pro_selection() {
        let root = std::env::current_dir()
            .unwrap()
            .join("build")
            .join(format!("nt6-boot-fixture-{}", std::process::id()));
        let _owner = OutputDirectory::create(&root).unwrap();
        let source = root.join("source");
        let boot = source.join("1/Windows/Boot");
        for directory in ["EFI_EX", "DVD_EX/EFI/en-US", "Fonts_EX", "EFI"] {
            fs::create_dir_all(boot.join(directory)).unwrap();
        }
        for (name, bytes) in [
            ("EFI_EX/bootmgfw_EX.efi", b"manager".as_slice()),
            ("DVD_EX/EFI/en-US/efisys_EX.bin", b"prompt"),
            ("DVD_EX/EFI/en-US/efisys_noprompt_EX.bin", b"noprompt"),
            ("Fonts_EX/fixture_EX.ttf", b"font"),
            ("EFI/boot.stl", b"list"),
        ] {
            fs::write(boot.join(name), bytes).unwrap();
        }
        let archive = root.join("boot.zip");
        run(Command::new("7z")
            .args(["a", "-tzip", "-bd"])
            .arg(&archive)
            .arg("1")
            .current_dir(&source))
        .unwrap();
        for (unattended_pro, expected, selected) in [
            (false, b"prompt".as_slice(), "efisys_EX.bin"),
            (true, b"noprompt".as_slice(), "efisys_noprompt_EX.bin"),
        ] {
            let output = root.join(selected);
            let tree = output.join("tree");
            fs::create_dir_all(tree.join("sources")).unwrap();
            fs::create_dir_all(tree.join("efi/boot")).unwrap();
            fs::create_dir_all(tree.join("efi/microsoft/boot/fonts")).unwrap();
            fs::copy(&archive, tree.join("sources/boot.wim")).unwrap();
            prepare_2023_boot(&tree, &output, unattended_pro).unwrap();
            assert_eq!(
                fs::read(tree.join("efi/microsoft/boot/efisys_ex.bin")).unwrap(),
                expected
            );
            let evidence = fs::read_to_string(output.join("boot-replacements.sha256")).unwrap();
            assert!(evidence.contains(&format!(
                "DVD_EX/EFI/en-US/{selected} -> efi/microsoft/boot/efisys_ex.bin"
            )));
            assert!(!output.join("boot-2023").exists());
        }
    }

    fn check_automatic_drivers(with_answer: bool, architecture: Architecture) {
        let root = std::env::current_dir().unwrap().join("build").join(format!(
            "nt6-driver-fixture-{}-{with_answer}-{}",
            std::process::id(),
            architecture.directory()
        ));
        let _owner = OutputDirectory::create(&root).unwrap();
        let source = root.join("source");
        let tree = root.join("tree");
        fs::create_dir(&tree).unwrap();
        let mut selected = BTreeMap::new();
        for component in ["viostor", "NetKVM", "vioinput", "viosock"] {
            for (version, arch) in [
                ("w11", "amd64"),
                ("w10", "amd64"),
                ("w11", "x86"),
                ("w11", "ARM64"),
            ] {
                let directory = source.join(component).join(version).join(arch);
                fs::create_dir_all(&directory).unwrap();
                let mut names: Vec<_> = ["inf", "sys", "cat"]
                    .map(|extension| format!("{}.{}", component.to_ascii_lowercase(), extension))
                    .into();
                let dependencies: &[&str] = match component {
                    "NetKVM" => &["netkvmco.exe", "netkvmp.exe", "Readme.md"],
                    "vioinput" => &["viohidkmdf.sys"],
                    "viosock" => &["viosocklib.dll", "viosockwspsvc.exe", "vstbridge.exe"],
                    _ => &[],
                };
                names.extend(dependencies.iter().map(|name| (*name).to_owned()));
                for name in ["lower.pdb", "upper.PDB", "mixed.PdB"] {
                    fs::write(directory.join(name), b"debug symbols").unwrap();
                }
                for name in names {
                    let bytes = format!("{component}/{version}/{arch}/{name}").into_bytes();
                    fs::write(directory.join(&name), &bytes).unwrap();
                    if version == "w11"
                        && arch == architecture.directory()
                        && (matches!(component, "viostor" | "NetKVM")
                            || architecture == Architecture::Arm64)
                    {
                        selected.insert(Path::new(component).join(name), bytes);
                    }
                }
            }
        }
        let archive = root.join("drivers.zip");
        run(Command::new("7z")
            .args(["a", "-tzip", "-bd"])
            .arg(&archive)
            .args(["viostor", "NetKVM", "vioinput", "viosock"])
            .current_dir(&source))
        .unwrap();
        let answer = with_answer.then(|| b"<unattend/>".to_vec());
        prepare_install_files(&archive, "w11", architecture, &root, &tree, answer.clone()).unwrap();
        let automatic = tree.join("$WinPEDriver$");
        assert!(
            automatic.is_dir(),
            "Setup automatic driver directory absent"
        );
        let mut actual = BTreeMap::new();
        manifest(&automatic, &automatic, &mut actual).unwrap();
        assert_eq!(actual.len(), selected.len());
        for (path, bytes) in selected {
            assert_eq!(fs::read(automatic.join(path)).unwrap(), bytes);
        }
        assert!(!tree.join("Drivers").exists());
        assert_eq!(fs::read(tree.join("Autounattend.xml")).ok(), answer);
    }

    #[test]
    fn graphical_media_has_automatic_storage_and_network_drivers() {
        for architecture in [Architecture::Amd64, Architecture::Arm64] {
            check_automatic_drivers(false, architecture);
        }
    }

    #[test]
    fn unattended_media_has_the_same_automatic_driver_layout() {
        for architecture in [Architecture::Amd64, Architecture::Arm64] {
            check_automatic_drivers(true, architecture);
        }
    }
}
