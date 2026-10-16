// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::error::Error;
use std::fs::{self, DirBuilder};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn checked(command: &mut Command) -> Result<()> {
    if !command.status()?.success() {
        return Err(format!("command failed: {command:?}").into());
    }
    Ok(())
}

pub fn digest(path: &Path) -> Result<String> {
    let output = Command::new("sha256sum").arg(path).output()?;
    if !output.status.success() {
        return Err(format!("cannot hash {}", path.display()).into());
    }
    Ok(String::from_utf8(output.stdout)?
        .get(..64)
        .ok_or("short digest")?
        .to_owned())
}

pub fn verify(path: &Path, expected: &str) -> Result<()> {
    if digest(path)? != expected {
        return Err(format!("SHA256 mismatch: {}", path.display()).into());
    }
    Ok(())
}

pub fn boot_catalog(iso: &mut (impl Read + Seek)) -> Result<[u8; 2048]> {
    let mut descriptor = [0; 2048];
    for block in 16..256 {
        iso.seek(SeekFrom::Start(block * 2048))?;
        iso.read_exact(&mut descriptor)?;
        if descriptor[1..6] != *b"CD001" || descriptor[0] == 255 {
            break;
        }
        if descriptor[0] != 0 || descriptor[7..30] != *b"EL TORITO SPECIFICATION" {
            continue;
        }
        let catalog = u32::from_le_bytes(descriptor[71..75].try_into()?);
        iso.seek(SeekFrom::Start(u64::from(catalog) * 2048))?;
        iso.read_exact(&mut descriptor)?;
        let checksum = descriptor[..32]
            .as_chunks::<2>()
            .0
            .iter()
            .fold(0u16, |sum, pair| {
                sum.wrapping_add(u16::from_le_bytes(*pair))
            });
        if descriptor[0] != 1 || descriptor[30..32] != [0x55, 0xaa] || checksum != 0 {
            return Err("invalid El Torito validation entry".into());
        }
        return Ok(descriptor);
    }
    Err("no El Torito boot record".into())
}

/// Owns only a freshly created output tree; failed builds leave no large staging files.
pub struct OutputDirectory(Option<PathBuf>);

impl OutputDirectory {
    pub fn create(path: &Path) -> Result<Self> {
        DirBuilder::new().mode(0o700).create(path)?;
        Ok(Self(Some(path.to_owned())))
    }

    pub fn finish(mut self) {
        self.0 = None;
    }
}

impl Drop for OutputDirectory {
    fn drop(&mut self) {
        if let Some(path) = &self.0
            && let Err(error) = fs::remove_dir_all(path)
        {
            eprintln!(
                "Cannot clean failed media build {}: {error}",
                path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_output_is_removed_but_existing_and_finished_outputs_survive() {
        let root = std::env::current_dir()
            .unwrap()
            .join("build/windows/console");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(format!("media-cleanup-test-{}", std::process::id()));
        let output = OutputDirectory::create(&path).unwrap();
        fs::write(path.join("partial.iso"), b"partial").unwrap();
        assert!(OutputDirectory::create(&path).is_err());
        assert!(path.join("partial.iso").exists());
        drop(output);
        assert!(!path.exists());

        let output = OutputDirectory::create(&path).unwrap();
        fs::write(path.join("complete.iso"), b"complete").unwrap();
        output.finish();
        assert!(path.join("complete.iso").exists());
        fs::remove_dir_all(path).unwrap();
    }
}
