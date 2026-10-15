// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-or-later
//! Pinned Microsoft trust policy; no private keys or guest-facing provisioning.
use crate::engine::{State, Variable};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

const OWNER: [u8; 16] = [
    0xbd, 0x9a, 0xfa, 0x77, 0x59, 0x03, 0x32, 0x4d, 0xbd, 0x60, 0x28, 0xf4, 0xe7, 0x8f, 0x78, 0x4b,
];
const X509: [u8; 16] = [
    0xa1, 0x59, 0xc0, 0xa5, 0xe4, 0x94, 0xa7, 0x4a, 0x87, 0xb5, 0xab, 0x15, 0x5c, 0x2b, 0xf0, 0x72,
];
const SHA256: [u8; 16] = [
    0x26, 0x16, 0xc4, 0xc1, 0x4c, 0x50, 0x92, 0x40, 0xac, 0xa9, 0x41, 0xf9, 0x36, 0x93, 0x43, 0x28,
];
const GLOBAL: [u8; 16] = [
    0x61, 0xdf, 0xe4, 0x8b, 0xca, 0x93, 0xd2, 0x11, 0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c,
];
const IMAGE: [u8; 16] = [
    0xcb, 0xb2, 0x19, 0xd7, 0x3a, 0x3d, 0x96, 0x45, 0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67, 0x65, 0x6f,
];

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn load(root: &Path, name: &str, expected: &str) -> io::Result<Vec<u8>> {
    let path = root.join(name);
    if fs::metadata(&path)?.len() > 1024 * 1024 {
        return Err(invalid("policy input exceeds limit"));
    }
    let data = fs::read(path)?;
    if format!("{:x}", Sha256::digest(&data)) != expected {
        return Err(invalid("policy input digest mismatch"));
    }
    Ok(data)
}
fn signature_list(kind: [u8; 16], entries: &[Vec<u8>]) -> io::Result<Vec<u8>> {
    let first = entries
        .first()
        .ok_or_else(|| invalid("empty signature list"))?;
    if entries.iter().any(|entry| entry.len() != first.len()) {
        return Err(invalid("mixed signature sizes"));
    }
    let size = u32::try_from(first.len() + 16).map_err(|_| invalid("signature too large"))?;
    let length = size
        .checked_mul(u32::try_from(entries.len()).map_err(|_| invalid("too many signatures"))?)
        .and_then(|size| size.checked_add(28))
        .ok_or_else(|| invalid("signature list too large"))?;
    let mut out = Vec::from(kind);
    out.extend(length.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(size.to_le_bytes());
    for entry in entries {
        out.extend(OWNER);
        out.extend(entry);
    }
    Ok(out)
}
fn hash(value: &str) -> io::Result<Vec<u8>> {
    if value.len() != 64 || !value.is_ascii() {
        return Err(invalid("invalid SHA256 encoding"));
    }
    (0..64)
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| invalid("invalid SHA256 encoding"))
        })
        .collect()
}
#[derive(Deserialize)]
struct ImageHash {
    #[serde(rename = "authenticodeHash")]
    hash: String,
    #[serde(rename = "hashType")]
    kind: String,
}
#[derive(Deserialize)]
struct Certificate {
    value: String,
}
#[derive(Deserialize)]
struct Svn {
    value: String,
}
#[derive(Deserialize)]
struct Revocations {
    images: BTreeMap<String, Vec<ImageHash>>,
    certificates: Vec<Certificate>,
    svns: Vec<Svn>,
}

pub fn prepare(root: &Path, output: &Path) -> io::Result<()> {
    let certificate = |name, digest| signature_list(X509, &[load(root, name, digest)?]);
    let pk = certificate(
        "WindowsOEMDevicesPK.der",
        "2f569e8edaf9657dc4951c29598725255c7f821472db71374211fe44d082546f",
    )?;
    let kek = certificate(
        "microsoft corporation kek 2k ca 2023.der",
        "3cd3f0309edae228767a976dd40d9f4affc4fbd5218f2e8cc3c9dd97e8ac6f9d",
    )?;
    let db = certificate(
        "windows uefi ca 2023.der",
        "076f1fea90ac29155ebf77c17682f75f1fdd1be196da302dc8461e350a9ae330",
    )?;
    let revoked = load(
        root,
        "dbx_info_msft_latest.json",
        "7c3bcbb57af5678bf30a1945f38ab3314aaf05e73d33766aaa69e6dcbe9c68a8",
    )?;
    let revoked: Revocations = serde_json::from_slice(&revoked)?;
    let hashes = revoked
        .images
        .get("x64")
        .ok_or_else(|| invalid("missing AMD64 revocations"))?
        .iter()
        .map(|entry| {
            if entry.kind != "SHA256" {
                return Err(invalid("unknown image hash type"));
            }
            hash(&entry.hash)
        })
        .collect::<io::Result<Vec<_>>>()?;
    let mut dbx = signature_list(SHA256, &hashes)?;
    if revoked.certificates.len() != 1
        || revoked.certificates[0].value != "MicWinProPCA2011_2011-10-19.der"
    {
        return Err(invalid("unexpected revoked certificate list"));
    }
    dbx.extend(certificate(
        "MicWinProPCA2011_2011-10-19.der",
        "e8e95f0733a55e8bad7be0a1413ee23c51fcea64b3c8fa6a786935fddcc71961",
    )?);
    dbx.extend(signature_list(
        SHA256,
        &revoked
            .svns
            .iter()
            .map(|svn| hash(&svn.value))
            .collect::<io::Result<Vec<_>>>()?,
    )?);
    let variables = [
        ("PK", GLOBAL, pk),
        ("KEK", GLOBAL, kek),
        ("db", IMAGE, db),
        ("dbx", IMAGE, dbx),
    ]
    .into_iter()
    .map(|(name, guid, data)| Variable {
        guid,
        name: format!("{name}\0")
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect(),
        data,
        digest: Vec::new(),
        attributes: 0x27,
        time: [0; 16],
    })
    .collect();
    let state = State {
        flags: 0,
        variables,
        policies: Vec::new(),
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(output)?;
    file.write_all(&serde_json::to_vec(&state)?)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signature_list_has_exact_efi_lengths() {
        let list = signature_list(SHA256, &[vec![1; 32], vec![2; 32]]).unwrap();
        assert_eq!(list.len(), 124);
        assert_eq!(&list[16..20], &124u32.to_le_bytes());
        assert_eq!(&list[24..28], &48u32.to_le_bytes());
        assert_eq!(&list[28..44], &OWNER);
        assert_eq!(&list[76..92], &OWNER);
        assert!(signature_list(X509, &[vec![1], vec![2, 3]]).is_err());
        assert!(signature_list(SHA256, &[]).is_err());
    }
    #[test]
    fn digest_encoding_is_strict() {
        assert_eq!(hash(&"ab".repeat(32)).unwrap(), vec![0xab; 32]);
        assert!(hash(&"gg".repeat(32)).is_err());
        assert!(hash(&"a".repeat(63)).is_err());
    }
}
