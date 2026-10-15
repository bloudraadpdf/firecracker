// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0
use super::*;
use cms::{
    cert::{CertificateChoices, IssuerAndSerialNumber},
    content_info::ContentInfo,
    signed_data::{CertificateSet, SignedData, SignerIdentifier},
};
use der::{
    Decode, Encode,
    asn1::{Any, BitString, OctetString, SetOfVec},
};
use rsa::{
    RsaPrivateKey,
    pkcs1v15::SigningKey,
    pkcs8::EncodePublicKey,
    rand_core::OsRng,
    signature::{SignatureEncoding, Signer},
};
use sha2::{Digest, Sha256};
use x509_cert::{Certificate, ext::pkix::BasicConstraints};

fn keys() -> &'static [RsaPrivateKey; 3] {
    static KEYS: std::sync::OnceLock<[RsaPrivateKey; 3]> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| std::array::from_fn(|_| RsaPrivateKey::new(&mut OsRng, 2048).unwrap()))
}
fn certificate(
    subject: &str,
    key: usize,
    issuer: &str,
    issuer_key: usize,
    ca: bool,
) -> Certificate {
    let mut cert = Certificate::from_der(include_bytes!("../../tests/fixtures/test.der")).unwrap();
    let tbs = &mut cert.tbs_certificate;
    tbs.subject = subject.parse().unwrap();
    tbs.issuer = issuer.parse().unwrap();
    tbs.serial_number = u64::try_from(key + 1).unwrap().into();
    tbs.subject_public_key_info = Decode::from_der(
        keys()[key]
            .to_public_key()
            .to_public_key_der()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let constraints = BasicConstraints {
        ca,
        path_len_constraint: ca.then_some(0),
    }
    .to_der()
    .unwrap();
    let extension = tbs
        .extensions
        .as_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e.extn_id.to_string() == "2.5.29.19")
        .unwrap();
    extension.extn_value = OctetString::new(constraints).unwrap();
    let signature =
        SigningKey::<Sha256>::new(keys()[issuer_key].clone()).sign(&tbs.to_der().unwrap());
    cert.signature = BitString::from_bytes(&signature.to_vec()).unwrap();
    cert
}
fn database(cert: &Certificate) -> Vec<u8> {
    let cert = cert.to_der().unwrap();
    let length = 44 + cert.len();
    let mut data = X509.to_vec();
    data.extend(u32::try_from(length).unwrap().to_le_bytes());
    data.extend(0u32.to_le_bytes());
    data.extend(u32::try_from(length - 28).unwrap().to_le_bytes());
    data.extend([0; 16]);
    data.extend(cert);
    data
}
fn chain() -> [Certificate; 3] {
    [
        certificate("CN=Root", 0, "CN=Root", 0, true),
        certificate("CN=Issuer", 1, "CN=Root", 0, true),
        certificate("CN=Leaf", 2, "CN=Issuer", 1, false),
    ]
}
fn seeded(root: &Certificate) -> Engine {
    let mut engine = Engine::restore(&State {
        flags: 0,
        policies: Vec::new(),
        variables: ["PK", "KEK"]
            .into_iter()
            .map(|text| Variable {
                guid: GLOBAL,
                name: name(text),
                data: database(root),
                digest: Vec::new(),
                attributes: SIG_DB_ATTRIBUTES,
                time: [0; 16],
            })
            .collect(),
    })
    .unwrap();
    engine.cold_boot().unwrap();
    engine
}
fn update(text: &str, guid: Guid, certs: &[Certificate], key: usize) -> Vec<u8> {
    update_at(text, guid, certs, key, 0)
}
fn update_at(text: &str, guid: Guid, certs: &[Certificate], key: usize, second: u8) -> Vec<u8> {
    signed_update(
        text,
        guid,
        certs,
        key,
        second,
        SIG_DB_ATTRIBUTES,
        include_bytes!("../../tests/fixtures/test.esl"),
    )
}
fn signed_update(
    text: &str,
    guid: Guid,
    certs: &[Certificate],
    key: usize,
    second: u8,
    attributes: u32,
    payload: &[u8],
) -> Vec<u8> {
    let fixture = include_bytes!("../../tests/fixtures/db.auth");
    let length = usize::try_from(u32_at(fixture, 16).unwrap()).unwrap() + 16;
    let info = ContentInfo::from_der(&fixture[40..length]).unwrap();
    let mut signed: SignedData = info.content.decode_as().unwrap();
    let mut signer = signed.signer_infos.0.get(0).unwrap().clone();
    signer.sid = SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
        issuer: certs[0].tbs_certificate.issuer.clone(),
        serial_number: certs[0].tbs_certificate.serial_number.clone(),
    });
    let mut data = name(text);
    data.truncate(data.len() - 2);
    data.extend(guid);
    data.extend(attributes.to_le_bytes());
    let mut time = fixture[..16].to_vec();
    time[6] = second;
    data.extend(&time);
    data.extend(payload);
    let message = if let Some(attributes) = &mut signer.signed_attrs {
        let mut values: Vec<_> = attributes.iter().cloned().collect();
        for attribute in &mut values {
            if attribute.oid.to_string() == "1.2.840.113549.1.9.4" {
                attribute.values = SetOfVec::try_from(vec![
                    Any::encode_from(&OctetString::new(Sha256::digest(&data).to_vec()).unwrap())
                        .unwrap(),
                ])
                .unwrap();
            }
        }
        *attributes = SetOfVec::try_from(values).unwrap();
        attributes.to_der().unwrap()
    } else {
        data
    };
    signer.signature = OctetString::new(
        SigningKey::<Sha256>::new(keys()[key].clone())
            .sign(&message)
            .to_vec(),
    )
    .unwrap();
    signed.signer_infos.0 = SetOfVec::try_from(vec![signer]).unwrap();
    signed.certificates = Some(CertificateSet(
        SetOfVec::try_from(
            certs
                .iter()
                .cloned()
                .map(CertificateChoices::Certificate)
                .collect::<Vec<_>>(),
        )
        .unwrap(),
    ));
    let der = ContentInfo {
        content_type: SIGNED_DATA,
        content: Any::encode_from(&signed).unwrap(),
    }
    .to_der()
    .unwrap();
    let mut auth = fixture[..40].to_vec();
    auth[..16].copy_from_slice(&time);
    put32(&mut auth, 16, 24 + der.len());
    auth.extend(der);
    auth.extend(payload);
    auth
}

#[test]
fn signed_chain_requires_a_valid_intermediate_and_private_identity_survives_restore() {
    let [root, issuer, leaf] = chain();
    let mut engine = seeded(&root);
    let valid = update("db", IMAGE, &[leaf.clone(), issuer.clone()], 2);
    assert_eq!(
        engine.set_variable("db", IMAGE, SIG_DB_ATTRIBUTES, &valid),
        Ok(0)
    );
    let mut bad_issuer = issuer.clone();
    let mut signature = bad_issuer.signature.as_bytes().unwrap().to_vec();
    signature[0] ^= 1;
    bad_issuer.signature = BitString::from_bytes(&signature).unwrap();
    let mut engine = seeded(&root);
    let before = engine.save();
    assert_eq!(
        engine.set_variable(
            "db",
            IMAGE,
            SIG_DB_ATTRIBUTES,
            &update("db", IMAGE, &[leaf.clone(), bad_issuer], 2)
        ),
        Ok(SECURITY)
    );
    assert_eq!(engine.save(), before);
    let mut nonca = issuer.clone();
    nonca
        .tbs_certificate
        .extensions
        .as_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e.extn_id.to_string() == "2.5.29.19")
        .unwrap()
        .extn_value = OctetString::new(
        BasicConstraints {
            ca: false,
            path_len_constraint: None,
        }
        .to_der()
        .unwrap(),
    )
    .unwrap();
    nonca.signature = BitString::from_bytes(
        &SigningKey::<Sha256>::new(keys()[0].clone())
            .sign(&nonca.tbs_certificate.to_der().unwrap())
            .to_vec(),
    )
    .unwrap();
    assert_eq!(
        engine.set_variable(
            "db",
            IMAGE,
            SIG_DB_ATTRIBUTES,
            &update("db", IMAGE, &[leaf.clone(), nonca], 2)
        ),
        Ok(SECURITY)
    );
    let private = update(
        "private",
        [0x31; 16],
        &[leaf.clone(), issuer.clone(), root.clone()],
        2,
    );
    assert_eq!(
        engine.set_variable("private", [0x31; 16], SIG_DB_ATTRIBUTES, &private),
        Ok(0)
    );
    let mut restored = Engine::restore(&engine.save()).unwrap();
    let next = update_at(
        "private",
        [0x31; 16],
        &[leaf.clone(), issuer.clone(), root],
        2,
        1,
    );
    assert_eq!(
        restored.set_variable("private", [0x31; 16], SIG_DB_ATTRIBUTES, &next),
        Ok(0)
    );
    let before = restored.save();
    let renamed = certificate("CN=DifferentLeaf", 2, "CN=Issuer", 1, false);
    let renamed_update = update_at("private", [0x31; 16], &[renamed, issuer], 2, 2);
    assert_eq!(
        restored.set_variable("private", [0x31; 16], SIG_DB_ATTRIBUTES, &renamed_update),
        Ok(SECURITY)
    );
    assert_eq!(restored.save(), before);
    let other_root = certificate("CN=DifferentRoot", 0, "CN=DifferentRoot", 0, true);
    let other_issuer = certificate("CN=Issuer", 1, "CN=DifferentRoot", 0, true);
    let other_chain = update_at(
        "private",
        [0x31; 16],
        &[leaf, other_issuer, other_root],
        2,
        2,
    );
    assert_eq!(
        restored.set_variable("private", [0x31; 16], SIG_DB_ATTRIBUTES, &other_chain),
        Ok(SECURITY)
    );
    assert_eq!(restored.save(), before);
    assert_eq!(
        restored.set_variable("private", [0x31; 16], SIG_DB_ATTRIBUTES, &private),
        Ok(SECURITY)
    );
}

#[test]
fn platform_key_cannot_delegate_pk_or_kek_updates_through_certificates() {
    let [root, issuer, leaf] = chain();
    for text in ["PK", "KEK"] {
        let mut engine = seeded(&root);
        let before = engine.save();
        let auth = update(text, GLOBAL, &[leaf.clone(), issuer.clone()], 2);
        assert_eq!(
            engine.set_variable(text, GLOBAL, SIG_DB_ATTRIBUTES, &auth),
            Ok(SECURITY)
        );
        assert_eq!(engine.save(), before);
        let direct = update(text, GLOBAL, std::slice::from_ref(&root), 0);
        assert_eq!(
            engine.set_variable(text, GLOBAL, SIG_DB_ATTRIBUTES, &direct),
            Ok(0)
        );
        let saved = engine.save();
        assert_eq!(Engine::restore(&saved).unwrap().save(), saved);
    }
}

#[test]
fn authenticated_kek_update_rejects_hash_entries_without_mutating_state() {
    let root = certificate("CN=Root", 0, "CN=Root", 0, true);
    let mut engine = seeded(&root);
    let before = engine.save();
    let mut hash = HASH256.to_vec();
    hash.extend(76u32.to_le_bytes());
    hash.extend(0u32.to_le_bytes());
    hash.extend(48u32.to_le_bytes());
    hash.extend([0; 48]);
    let auth = signed_update("KEK", GLOBAL, &[root], 0, 0, SIG_DB_ATTRIBUTES, &hash);
    assert_ne!(
        engine.set_variable("KEK", GLOBAL, SIG_DB_ATTRIBUTES, &auth),
        Ok(0)
    );
    assert_eq!(engine.save(), before);
}

#[test]
fn image_and_recovery_updates_allow_direct_pk_or_kek_chains_only() {
    let root = certificate("CN=Root", 0, "CN=Root", 0, true);
    let kek = certificate("CN=KEK", 1, "CN=KEK", 1, true);
    let mut state = seeded(&root).save();
    state
        .variables
        .iter_mut()
        .find(|v| v.name == name("KEK"))
        .unwrap()
        .data = database(&kek);
    for (text, guid) in [
        ("db", IMAGE),
        ("OsRecoveryOrder", GLOBAL),
        ("OsRecovery0001", [0x31; 16]),
    ] {
        let mut engine = Engine::restore(&state).unwrap();
        let leaf = certificate("CN=Leaf", 2, "CN=Root", 0, false);
        let rejected = update(text, guid, &[leaf], 2);
        assert_eq!(
            engine.set_variable(text, guid, SIG_DB_ATTRIBUTES, &rejected),
            Ok(SECURITY)
        );
        let leaf = certificate("CN=Leaf", 2, "CN=KEK", 1, false);
        let accepted = update(text, guid, &[leaf], 2);
        assert_eq!(
            engine.set_variable(text, guid, SIG_DB_ATTRIBUTES, &accepted),
            Ok(0)
        );
        let mut engine = Engine::restore(&state).unwrap();
        assert_eq!(
            engine.set_variable(
                text,
                guid,
                SIG_DB_ATTRIBUTES,
                &update(text, guid, std::slice::from_ref(&root), 0)
            ),
            Ok(0)
        );
    }
    let mut engine = Engine::restore(&state).unwrap();
    let before = engine.save();
    let append = signed_update(
        "PK",
        GLOBAL,
        std::slice::from_ref(&root),
        0,
        0,
        SIG_DB_ATTRIBUTES | APPEND,
        include_bytes!("../../tests/fixtures/test.esl"),
    );
    assert_eq!(
        engine.set_variable("PK", GLOBAL, SIG_DB_ATTRIBUTES | APPEND, &append),
        Ok(INVALID)
    );
    assert_eq!(engine.save(), before);
}

#[test]
fn cyclic_same_key_certificate_graph_has_bounded_search() {
    let trusted = certificate("CN=Trusted", 1, "CN=Trusted", 1, true);
    let mut certificates = Vec::new();
    for index in 0u64..16 {
        let mut cert = certificate("CN=Ring", 0, "CN=Ring", 0, true);
        cert.tbs_certificate.serial_number = (index + 100).into();
        cert.tbs_certificate
            .extensions
            .as_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e.extn_id.to_string() == "2.5.29.19")
            .unwrap()
            .extn_value = OctetString::new(
            BasicConstraints {
                ca: true,
                path_len_constraint: None,
            }
            .to_der()
            .unwrap(),
        )
        .unwrap();
        cert.signature = BitString::from_bytes(
            &SigningKey::<Sha256>::new(keys()[0].clone())
                .sign(&cert.tbs_certificate.to_der().unwrap())
                .to_vec(),
        )
        .unwrap();
        certificates.push(cert);
    }
    let mut engine = seeded(&trusted);
    let before = engine.save();
    assert_eq!(
        engine.set_variable(
            "db",
            IMAGE,
            SIG_DB_ATTRIBUTES,
            &update("db", IMAGE, &certificates, 0)
        ),
        Ok(SECURITY)
    );
    assert_eq!(engine.save(), before);
}

#[test]
fn private_variable_accepts_an_included_nonselfsigned_issuer_and_retains_identity() {
    let [root, issuer, leaf] = chain();
    let mut engine = seeded(&root);
    let first = update("private", [0x31; 16], &[leaf.clone(), issuer.clone()], 2);
    assert_eq!(
        engine.set_variable("private", [0x31; 16], SIG_DB_ATTRIBUTES, &first),
        Ok(0)
    );
    let mut restored = Engine::restore(&engine.save()).unwrap();
    let next = update_at("private", [0x31; 16], &[leaf, issuer], 2, 1);
    assert_eq!(
        restored.set_variable("private", [0x31; 16], SIG_DB_ATTRIBUTES, &next),
        Ok(0)
    );
}
