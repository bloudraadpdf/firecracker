// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0
use super::{Engine, Variable, format::*};
use aws_lc_rs::{
    encoding::{AsDer, PublicKeyX509Der},
    rsa::PublicKey as RsaPublicKey,
    signature::{self, RsaParameters, RsaPublicKeyComponents, UnparsedPublicKey},
};
use cms::{
    cert::CertificateChoices,
    content_info::ContentInfo,
    signed_data::{SignedData, SignerIdentifier},
};
use der::{
    Decode, Encode,
    asn1::{Any, ObjectIdentifier, OctetString},
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use x509_cert::{
    Certificate,
    ext::pkix::{BasicConstraints, KeyUsage},
};

pub(super) fn recovery_variable(guid: Guid, value: &[u8]) -> bool {
    (guid == GLOBAL && value == name("OsRecoveryOrder"))
        || (value.len() == 30
            && value.starts_with(&name("OsRecovery")[..20])
            && value[20..28]
                .as_chunks::<2>()
                .0
                .iter()
                .all(|u| matches!(u16::from_le_bytes(*u), 48..=57 | 65..=70 | 97..=102)))
}
pub(super) fn secure_variable(guid: Guid, value: &[u8]) -> bool {
    guid == IMAGE
        || recovery_variable(guid, value)
        || (guid == GLOBAL && ["PK", "KEK"].iter().any(|s| value == name(s)))
}
pub(super) fn signature_database(guid: Guid, value: &[u8]) -> bool {
    secure_variable(guid, value) && !recovery_variable(guid, value)
}
pub(super) fn supported_variable(guid: Guid, value: &[u8]) -> Status {
    if (guid == IMAGE && !["db", "dbx", "dbt", "dbr"].iter().any(|s| value == name(s)))
        || (guid == GLOBAL
            && ["AuditMode", "DeployedMode"]
                .iter()
                .any(|s| value == name(s)))
    {
        return Err(UNSUPPORTED);
    }
    Ok(())
}
pub(super) fn timestamp(time: &[u8; 16]) -> [u16; 6] {
    [
        u16::from_le_bytes([time[0], time[1]]),
        u16::from(time[2]),
        u16::from(time[3]),
        u16::from(time[4]),
        u16::from(time[5]),
        u16::from(time[6]),
    ]
}
fn valid_time(time: &[u8; 16], append: bool) -> bool {
    if time[7..].iter().any(|b| *b != 0) {
        return false;
    }
    if append && *time == [0; 16] {
        return true;
    }
    let [year, month, day, hour, minute, second] = timestamp(time);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return false,
    };
    (1900..=9999).contains(&year)
        && (1..=days).contains(&day)
        && hour < 24
        && minute < 60
        && second < 60
}
struct SignatureList<'a> {
    kind: Guid,
    header: &'a [u8],
    entries: Vec<&'a [u8]>,
}
fn lists(mut data: &[u8]) -> Result<Vec<SignatureList<'_>>, u64> {
    let mut result = Vec::new();
    while !data.is_empty() {
        let kind = number(data, 0).map_err(|_| INVALID)?;
        let length = usize::try_from(u32_at(data, 16).map_err(|_| INVALID)?).unwrap();
        let header = usize::try_from(u32_at(data, 20).map_err(|_| INVALID)?).unwrap();
        let size = usize::try_from(u32_at(data, 24).map_err(|_| INVALID)?).unwrap();
        let start = 28usize.checked_add(header).ok_or(INVALID)?;
        let list = data.get(..length).ok_or(INVALID)?;
        let payload = list.get(start..).ok_or(INVALID)?;
        if length == 0 || size <= 16 || payload.is_empty() || !payload.len().is_multiple_of(size) {
            return Err(INVALID);
        }
        result.push(SignatureList {
            kind,
            header: &list[28..start],
            entries: payload.chunks_exact(size).collect(),
        });
        data = &data[length..];
    }
    Ok(result)
}
pub(super) fn validate_database(data: &[u8], platform_key: bool) -> Status {
    let lists = lists(data)?;
    if lists.is_empty() || (platform_key && (lists.len() != 1 || lists[0].entries.len() != 1)) {
        return Err(INVALID);
    }
    for list in lists {
        if !list.header.is_empty() {
            return Err(INVALID);
        }
        for entry in list.entries {
            let size = entry.len() - 16;
            if list.kind == X509 {
                Certificate::from_der(&entry[16..]).map_err(|_| INVALID)?;
            } else {
                let expected = match list.kind {
                    HASH256 => 32,
                    HASH384 => 48,
                    HASH512 => 64,
                    RSA2048 => 256,
                    k if k == guid(0x3bd2a492_96c0_4079_b420_fcf98ef103ed) => 48,
                    k if k == guid(0x7076876e_80c2_4ee6_aad2_28b349a6865b) => 64,
                    k if k == guid(0x446dbf63_2502_4cda_bcfa_2465d2b0fe9d) => 80,
                    _ => return Err(UNSUPPORTED),
                };
                if size != expected || (platform_key && list.kind != RSA2048) {
                    return Err(INVALID);
                }
            }
        }
    }
    Ok(())
}
pub(super) fn append_database(old: &[u8], new: &[u8]) -> Result<Vec<u8>, u64> {
    let mut seen: BTreeSet<_> = lists(old)?
        .into_iter()
        .flat_map(|list| {
            list.entries
                .into_iter()
                .map(move |entry| (list.kind, list.header, &entry[16..]))
        })
        .collect();
    let mut result = old.to_vec();
    for list in lists(new)? {
        let mut entries: Vec<&[u8]> = Vec::new();
        for entry in list.entries {
            if !seen.insert((list.kind, list.header, &entry[16..])) {
                continue;
            }
            entries.push(entry);
        }
        if entries.is_empty() {
            continue;
        }
        let length = 28 + list.header.len() + entries.iter().map(|e| e.len()).sum::<usize>();
        result.extend(list.kind);
        result.extend(u32::try_from(length).map_err(|_| NO_SPACE)?.to_le_bytes());
        result.extend(u32::try_from(list.header.len()).unwrap().to_le_bytes());
        result.extend(u32::try_from(entries[0].len()).unwrap().to_le_bytes());
        result.extend(list.header);
        for entry in entries {
            result.extend(entry);
        }
    }
    Ok(result)
}
pub(super) fn validate_keys(data: &[u8]) -> Status {
    for list in lists(data)? {
        for entry in list.entries {
            if list.kind == X509 {
                public_key(&Certificate::from_der(&entry[16..]).map_err(|_| INVALID)?)?;
            } else if list.kind == RSA2048 {
                let encoded: PublicKeyX509Der<'static> = RsaPublicKeyComponents {
                    n: &entry[16..],
                    e: &[1, 0, 1],
                }
                .as_der()
                .map_err(|_| INVALID)?;
                rsa_key(encoded.as_ref()).map_err(|_| INVALID)?;
            } else {
                return Err(UNSUPPORTED);
            }
        }
    }
    Ok(())
}
fn public_key(cert: &Certificate) -> Result<RsaPublicKey, u64> {
    let spki = &cert.tbs_certificate.subject_public_key_info;
    if spki.algorithm.oid != RSA_OID {
        return Err(UNSUPPORTED);
    }
    if spki.algorithm.parameters != Some(Any::null()) {
        return Err(SECURITY);
    }
    rsa_key(&spki.to_der().map_err(|_| SECURITY)?)
}
fn rsa_key(data: &[u8]) -> Result<RsaPublicKey, u64> {
    let key = RsaPublicKey::from_der(data).map_err(|_| SECURITY)?;
    let bits = RsaParameters::public_modulus_len(key.as_ref()).map_err(|_| SECURITY)?;
    if !(2048..=8192).contains(&bits) {
        return Err(SECURITY);
    }
    Ok(key)
}
fn verify(
    cert: &Certificate,
    algorithm: ObjectIdentifier,
    data: &[u8],
    signature: &[u8],
) -> Status {
    let key = public_key(cert)?;
    let parameters = match algorithm {
        RSA_SHA256 => &signature::RSA_PKCS1_2048_8192_SHA256,
        a if a == ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.12") => {
            &signature::RSA_PKCS1_2048_8192_SHA384
        }
        a if a == ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.13") => {
            &signature::RSA_PKCS1_2048_8192_SHA512
        }
        _ => return Err(UNSUPPORTED),
    };
    UnparsedPublicKey::new(parameters, key.as_ref())
        .verify(data, signature)
        .map_err(|_| SECURITY)
}
fn certificate_signature(cert: &Certificate, issuer: &Certificate) -> Status {
    if cert.signature_algorithm != cert.tbs_certificate.signature {
        return Err(SECURITY);
    }
    verify(
        issuer,
        cert.signature_algorithm.oid,
        &cert.tbs_certificate.to_der().map_err(|_| SECURITY)?,
        cert.signature.as_bytes().ok_or(SECURITY)?,
    )
}
fn certificate_constraints(cert: &Certificate) -> Status {
    let extensions = cert.tbs_certificate.extensions.as_deref().unwrap_or(&[]);
    for (index, extension) in extensions.iter().enumerate() {
        if extensions[..index]
            .iter()
            .any(|e| e.extn_id == extension.extn_id)
        {
            return Err(SECURITY);
        }
        if extension.critical
            && !["2.5.29.19", "2.5.29.15"]
                .iter()
                .any(|oid| extension.extn_id.to_string() == *oid)
        {
            return Err(UNSUPPORTED);
        }
    }
    cert.tbs_certificate
        .get::<BasicConstraints>()
        .map_err(|_| SECURITY)?;
    cert.tbs_certificate
        .get::<KeyUsage>()
        .map_err(|_| SECURITY)?;
    Ok(())
}
fn issuer_allowed(cert: &Certificate, depth: usize) -> bool {
    if certificate_constraints(cert).is_err() {
        return false;
    }
    let Ok(Some((_, constraints))) = cert.tbs_certificate.get::<BasicConstraints>() else {
        return false;
    };
    constraints.ca
        && constraints
            .path_len_constraint
            .is_none_or(|limit| depth <= usize::from(limit))
        && cert
            .tbs_certificate
            .get::<KeyUsage>()
            .ok()
            .flatten()
            .is_none_or(|(_, usage)| usage.key_cert_sign())
}
fn path(
    leaf: &Certificate,
    certificates: &[&Certificate],
    roots: &[Certificate],
) -> Result<Certificate, u64> {
    let mut pending = vec![(leaf, 0usize)];
    let mut visited = Vec::new();
    while let Some((index, _)) = pending
        .iter()
        .enumerate()
        .min_by_key(|(_, (_, depth))| *depth)
    {
        let (current, depth) = pending.swap_remove(index);
        if visited.contains(&current) {
            continue;
        }
        visited.push(current);
        if certificate_constraints(current).is_err() {
            continue;
        }
        if let Some(root) = roots.iter().find(|root| *root == current) {
            return Ok(root.clone());
        }
        for root in roots {
            if current.tbs_certificate.issuer == root.tbs_certificate.subject
                && certificate_signature(current, root).is_ok()
            {
                return Ok(root.clone());
            }
        }
        let depth = depth
            + usize::from(
                current != leaf
                    && current.tbs_certificate.subject != current.tbs_certificate.issuer,
            );
        for issuer in certificates {
            if !visited.contains(issuer)
                && current.tbs_certificate.issuer == issuer.tbs_certificate.subject
                && issuer_allowed(issuer, depth)
                && certificate_signature(current, issuer).is_ok()
            {
                pending.push((issuer, depth));
            }
        }
    }
    Err(SECURITY)
}
fn anchors(
    engine: &Engine,
    key: &str,
    certificates: &[&Certificate],
) -> Result<Vec<Certificate>, u64> {
    let mut roots = Vec::new();
    let namespace = if key == "dbr" { IMAGE } else { GLOBAL };
    if let Some(anchor) = engine.find(namespace, &name(key)) {
        for list in lists(&anchor.data)? {
            if list.kind == X509 {
                for entry in list.entries {
                    roots.push(Certificate::from_der(&entry[16..]).map_err(|_| SECURITY)?);
                }
            } else if list.kind == RSA2048 {
                for cert in certificates {
                    let key = public_key(cert)?;
                    if key.exponent().big_endian_without_leading_zero() == [1, 0, 1]
                        && list.entries.iter().any(|entry| {
                            entry[16..] == *key.modulus().big_endian_without_leading_zero()
                        })
                    {
                        roots.push((*cert).clone());
                    }
                }
            }
        }
    }
    Ok(roots)
}
fn decode_signed(data: &[u8]) -> Result<SignedData, u64> {
    if let Ok(content) = ContentInfo::from_der(data) {
        if content.content_type != SIGNED_DATA {
            return Err(SECURITY);
        }
        content.content.decode_as().map_err(|_| SECURITY)
    } else {
        SignedData::from_der(data).map_err(|_| SECURITY)
    }
}
fn signed_message(signed: &SignedData, data: &[u8]) -> Result<Vec<u8>, u64> {
    let signer = signed.signer_infos.0.get(0).ok_or(SECURITY)?;
    if signed.encap_content_info.econtent_type != DATA
        || signed.encap_content_info.econtent.is_some()
        || signed.digest_algorithms.len() != 1
        || signed.digest_algorithms.get(0).ok_or(SECURITY)?.oid != SHA256_OID
        || signer.digest_alg.oid != SHA256_OID
        || !matches!(signer.signature_algorithm.oid, RSA_OID | RSA_SHA256)
    {
        return Err(SECURITY);
    }
    let Some(attributes) = &signer.signed_attrs else {
        return Ok(data.to_vec());
    };
    let content_type = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.3");
    let digest = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");
    let mut content_found = false;
    let mut digest_found = false;
    for attr in attributes.iter() {
        if attr.oid == content_type {
            if content_found
                || attr.values.len() != 1
                || attr
                    .values
                    .get(0)
                    .ok_or(SECURITY)?
                    .decode_as::<ObjectIdentifier>()
                    .map_err(|_| SECURITY)?
                    != DATA
            {
                return Err(SECURITY);
            }
            content_found = true;
        }
        if attr.oid == digest {
            if digest_found
                || attr.values.len() != 1
                || attr
                    .values
                    .get(0)
                    .ok_or(SECURITY)?
                    .decode_as::<OctetString>()
                    .map_err(|_| SECURITY)?
                    .as_bytes()
                    != Sha256::digest(data).as_slice()
            {
                return Err(SECURITY);
            }
            digest_found = true;
        }
    }
    if !content_found || !digest_found {
        return Err(SECURITY);
    }
    attributes.to_der().map_err(|_| SECURITY)
}
pub(super) fn authenticate(
    engine: &Engine,
    value: &mut Variable,
    attributes: u32,
    input: &[u8],
) -> Status {
    let time: [u8; 16] = number(input, 0).map_err(|_| SECURITY)?;
    let append = attributes & APPEND != 0;
    let end = 16usize
        .checked_add(usize::try_from(u32_at(input, 16).map_err(|_| SECURITY)?).unwrap())
        .ok_or(SECURITY)?;
    if end < 40
        || end > input.len()
        || u16_at(input, 20)? != 0x200
        || u16_at(input, 22)? != 0xef1
        || number::<16>(input, 24)? != PKCS7
        || !valid_time(&time, append)
    {
        return Err(SECURITY);
    }
    let old = engine.find(value.guid, &value.name);
    if !append && old.is_some_and(|old| timestamp(&old.time) >= timestamp(&time)) {
        return Err(SECURITY);
    }
    let payload = &input[end..];
    let secure = secure_variable(value.guid, &value.name);
    if secure && engine.find(GLOBAL, &name("PK")).is_none() {
        value.data = payload.to_vec();
        value.time = time;
        return Ok(());
    }
    let signed = decode_signed(&input[40..end])?;
    if signed.signer_infos.0.len() != 1 {
        return Err(SECURITY);
    }
    let signer = signed.signer_infos.0.get(0).ok_or(SECURITY)?;
    let certs = signed.certificates.as_ref().ok_or(SECURITY)?;
    if certs.0.len() > 16 {
        return Err(SECURITY);
    }
    let certificates: Vec<_> = certs
        .0
        .iter()
        .filter_map(|c| match c {
            CertificateChoices::Certificate(c) => Some(c),
            _ => None,
        })
        .collect();
    let matches = |cert: &&Certificate| match &signer.sid {
        SignerIdentifier::IssuerAndSerialNumber(id) => {
            id.issuer == cert.tbs_certificate.issuer
                && id.serial_number == cert.tbs_certificate.serial_number
        }
        SignerIdentifier::SubjectKeyIdentifier(id) => cert
            .tbs_certificate
            .get::<x509_cert::ext::pkix::SubjectKeyIdentifier>()
            .ok()
            .flatten()
            .is_some_and(|(_, key)| key == *id),
    };
    let mut signers = certificates.iter().copied().filter(matches);
    let certificate = signers.next().ok_or(SECURITY)?;
    if signers.next().is_some() {
        return Err(SECURITY);
    }
    let mut data = value.name[..value.name.len() - 2].to_vec();
    data.extend(value.guid);
    data.extend(attributes.to_le_bytes());
    data.extend(time);
    data.extend(payload);
    verify(
        certificate,
        RSA_SHA256,
        &signed_message(&signed, &data)?,
        signer.signature.as_bytes(),
    )?;
    if secure {
        let platform = anchors(engine, "PK", &certificates)?;
        let signing_key = public_key(certificate)?;
        let direct_platform_signature = platform
            .iter()
            .any(|root| public_key(root).is_ok_and(|key| key.as_ref() == signing_key.as_ref()));
        let authorised = if direct_platform_signature {
            true
        } else if value.guid == GLOBAL && ["PK", "KEK"].iter().any(|text| value.name == name(text))
        {
            false
        } else {
            let mut roots = anchors(engine, "KEK", &certificates)?;
            if recovery_variable(value.guid, &value.name) {
                roots.extend(anchors(engine, "dbr", &certificates)?);
            }
            path(certificate, &certificates, &roots).is_ok()
        };
        if !authorised {
            return Err(SECURITY);
        }
    } else {
        let roots: Vec<_> = certificates
            .iter()
            .filter(|c| {
                !certificates.iter().any(|issuer| {
                    *issuer != **c
                        && c.tbs_certificate.issuer == issuer.tbs_certificate.subject
                        && certificate_signature(c, issuer).is_ok()
                })
            })
            .map(|c| (*c).clone())
            .collect();
        let root = if certificates.len() == 1 {
            certificate.clone()
        } else {
            path(certificate, &certificates, &roots)?
        };
        let common_name = ObjectIdentifier::new_unwrap("2.5.4.3");
        let names: Vec<_> = certificate
            .tbs_certificate
            .subject
            .0
            .iter()
            .flat_map(|rdn| rdn.0.iter())
            .filter(|atv| atv.oid == common_name)
            .collect();
        if names.len() != 1 {
            return Err(SECURITY);
        }
        let mut digest = Sha256::new();
        digest.update(names[0].value.value());
        digest.update(root.tbs_certificate.to_der().map_err(|_| SECURITY)?);
        value.digest = digest.finalize().to_vec();
        if old.is_some_and(|old| old.digest != value.digest) {
            return Err(SECURITY);
        }
    }
    value.data = payload.to_vec();
    value.time = time;
    Ok(())
}
