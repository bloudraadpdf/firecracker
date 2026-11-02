// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0
use super::*;
use aws_lc_rs::{
    encoding::AsDer,
    rand::SystemRandom,
    rsa::KeySize,
    signature::{
        KeyPair, RSA_PKCS1_SHA256, RSA_PKCS1_SHA384, RSA_PKCS1_SHA512, RsaEncoding, RsaKeyPair,
        RsaPublicKeyComponents,
    },
};
use cms::{
    cert::{CertificateChoices, IssuerAndSerialNumber},
    content_info::ContentInfo,
    signed_data::{CertificateSet, SignedData, SignerIdentifier},
};
use der::{
    Decode, Encode,
    asn1::{Any, BitString, OctetString, SetOfVec},
};
use sha2::{Digest, Sha256};
use x509_cert::{Certificate, ext::pkix::BasicConstraints};

fn keys() -> &'static [RsaKeyPair; 3] {
    static KEYS: std::sync::OnceLock<[RsaKeyPair; 3]> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| std::array::from_fn(|_| RsaKeyPair::generate(KeySize::Rsa2048).unwrap()))
}
fn sign(key: &RsaKeyPair, algorithm: &'static dyn RsaEncoding, data: &[u8]) -> Vec<u8> {
    let mut signature = vec![0; key.public_modulus_len()];
    key.sign(algorithm, &SystemRandom::new(), data, &mut signature)
        .unwrap();
    signature
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
    tbs.subject_public_key_info =
        Decode::from_der(keys()[key].public_key().as_der().unwrap().as_ref()).unwrap();
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
    cert.signature = BitString::from_bytes(&sign(
        &keys()[issuer_key],
        &RSA_PKCS1_SHA256,
        &tbs.to_der().unwrap(),
    ))
    .unwrap();
    cert
}
fn database(cert: &Certificate) -> Vec<u8> {
    signature_database(X509, &cert.to_der().unwrap())
}
fn signature_database(kind: Guid, value: &[u8]) -> Vec<u8> {
    let length = 44 + value.len();
    let mut data = kind.to_vec();
    data.extend(u32::try_from(length).unwrap().to_le_bytes());
    data.extend(0u32.to_le_bytes());
    data.extend(u32::try_from(length - 28).unwrap().to_le_bytes());
    data.extend([0; 16]);
    data.extend(value);
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
    signer.signature = OctetString::new(sign(&keys()[key], &RSA_PKCS1_SHA256, &message)).unwrap();
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
fn rsa_certificate_signatures_accept_supported_sizes_and_digests() {
    let large = RsaKeyPair::generate(KeySize::Rsa8192).unwrap();
    for key in [&keys()[0], &large] {
        let mut root = certificate("CN=Root", 0, "CN=Root", 0, true);
        root.tbs_certificate.subject_public_key_info =
            Decode::from_der(key.public_key().as_der().unwrap().as_ref()).unwrap();
        root.signature = BitString::from_bytes(&sign(
            key,
            &RSA_PKCS1_SHA256,
            &root.tbs_certificate.to_der().unwrap(),
        ))
        .unwrap();
        for (oid, algorithm) in [
            ("1.2.840.113549.1.1.11", &RSA_PKCS1_SHA256),
            ("1.2.840.113549.1.1.12", &RSA_PKCS1_SHA384),
            ("1.2.840.113549.1.1.13", &RSA_PKCS1_SHA512),
        ] {
            let mut leaf = certificate("CN=Leaf", 1, "CN=Root", 0, false);
            leaf.signature_algorithm.oid = oid.parse().unwrap();
            leaf.tbs_certificate.signature = leaf.signature_algorithm.clone();
            let mut signature = sign(key, algorithm, &leaf.tbs_certificate.to_der().unwrap());
            leaf.signature = BitString::from_bytes(&signature).unwrap();
            let mut engine = seeded(&root);
            assert_eq!(
                engine.set_variable(
                    "db",
                    IMAGE,
                    SIG_DB_ATTRIBUTES,
                    &update("db", IMAGE, &[leaf.clone()], 1),
                ),
                Ok(())
            );
            let before = engine.save();
            signature[0] ^= 1;
            leaf.signature = BitString::from_bytes(&signature).unwrap();
            assert_eq!(
                engine.set_variable(
                    "db",
                    IMAGE,
                    SIG_DB_ATTRIBUTES,
                    &update_at("db", IMAGE, &[leaf], 1, 1),
                ),
                Err(SECURITY)
            );
            assert_eq!(engine.save(), before);
        }
    }
}

#[test]
fn raw_rsa_platform_anchor_authorises_the_same_key_in_a_different_certificate() {
    let root = certificate("CN=Root", 0, "CN=Root", 0, true);
    let mut state = seeded(&root).save();
    state
        .variables
        .iter_mut()
        .find(|v| v.name == name("PK"))
        .unwrap()
        .data = signature_database(
        RSA2048,
        keys()[0]
            .public_key()
            .modulus()
            .big_endian_without_leading_zero(),
    );
    let mut engine = Engine::restore(&state).unwrap();
    let alternative = certificate("CN=Alternative", 0, "CN=Alternative", 0, true);
    let value = signed_update(
        "KEK",
        GLOBAL,
        &[alternative],
        0,
        0,
        SIG_DB_ATTRIBUTES,
        &database(&root),
    );
    assert_eq!(
        engine.set_variable("KEK", GLOBAL, SIG_DB_ATTRIBUTES, &value),
        Ok(())
    );
    assert!(Engine::restore(&engine.save()).is_ok());
}

#[test]
fn invalid_rsa_platform_keys_reject_without_mutation() {
    let root = certificate("CN=Root", 0, "CN=Root", 0, true);
    let mut engine = seeded(&root);
    let before = engine.save();
    let mut even = keys()[0]
        .public_key()
        .modulus()
        .big_endian_without_leading_zero()
        .to_vec();
    even[255] &= !1;
    for modulus in [vec![0; 256], vec![1; 256], even] {
        let value = signed_update(
            "PK",
            GLOBAL,
            std::slice::from_ref(&root),
            0,
            0,
            SIG_DB_ATTRIBUTES,
            &signature_database(RSA2048, &modulus),
        );
        assert_eq!(
            engine.set_variable("PK", GLOBAL, SIG_DB_ATTRIBUTES, &value),
            Err(INVALID)
        );
        assert_eq!(engine.save(), before);
    }
    for modulus in [vec![0xff; 128], vec![0xff; 1025]] {
        let encoded = RsaPublicKeyComponents {
            n: modulus.as_slice(),
            e: &[1, 0, 1],
        }
        .as_der()
        .unwrap();
        let mut invalid = root.clone();
        invalid.tbs_certificate.subject_public_key_info =
            Decode::from_der(encoded.as_ref()).unwrap();
        let value = signed_update(
            "PK",
            GLOBAL,
            std::slice::from_ref(&root),
            0,
            0,
            SIG_DB_ATTRIBUTES,
            &database(&invalid),
        );
        assert_eq!(
            engine.set_variable("PK", GLOBAL, SIG_DB_ATTRIBUTES, &value),
            Err(SECURITY)
        );
        assert_eq!(engine.save(), before);
    }
}

#[test]
fn rsa_public_key_parameters_and_exponents_retain_the_supported_contract() {
    let root = certificate("CN=Root", 0, "CN=Root", 0, true);
    let mut candidates = Vec::new();
    for parameters in [None, Some(Any::encode_from(&3u8).unwrap())] {
        let mut cert = root.clone();
        cert.tbs_certificate
            .subject_public_key_info
            .algorithm
            .parameters = parameters;
        candidates.push((cert, Err(SECURITY)));
    }
    let mut wrong_algorithm = root.clone();
    wrong_algorithm
        .tbs_certificate
        .subject_public_key_info
        .algorithm
        .oid = "1.2.840.10045.2.1".parse().unwrap();
    candidates.push((wrong_algorithm, Err(UNSUPPORTED)));
    for (exponent, expected) in [
        (vec![1], Err(SECURITY)),
        (vec![2], Err(SECURITY)),
        (vec![2, 0, 0, 0, 1], Err(SECURITY)),
        (vec![3], Ok(())),
        (vec![1, 0xff, 0xff, 0xff, 0xff], Ok(())),
    ] {
        let encoded = RsaPublicKeyComponents {
            n: keys()[0]
                .public_key()
                .modulus()
                .big_endian_without_leading_zero(),
            e: exponent.as_slice(),
        }
        .as_der()
        .unwrap();
        let mut cert = root.clone();
        cert.tbs_certificate.subject_public_key_info = Decode::from_der(encoded.as_ref()).unwrap();
        candidates.push((cert, expected));
    }
    for (cert, expected) in candidates {
        let mut engine = seeded(&root);
        let before = engine.save();
        let value = signed_update(
            "PK",
            GLOBAL,
            std::slice::from_ref(&root),
            0,
            0,
            SIG_DB_ATTRIBUTES,
            &database(&cert),
        );
        assert_eq!(
            engine.set_variable("PK", GLOBAL, SIG_DB_ATTRIBUTES, &value),
            expected
        );
        if expected.is_err() {
            assert_eq!(engine.save(), before);
        } else {
            assert!(Engine::restore(&engine.save()).is_ok());
        }
    }
}

#[test]
fn rustcrypto_checkpoint_retains_private_identity_and_accepts_the_next_signed_update() {
    let bytes = include_bytes!("../../tests/fixtures/rustcrypto-checkpoint.json");
    let checkpoint: crate::service::Checkpoint = serde_json::from_slice(bytes).unwrap();
    assert_eq!(checkpoint.version, 2);
    assert_eq!(serde_json::to_vec(&checkpoint).unwrap().as_slice(), bytes);
    let mut engine = Engine::restore(&checkpoint.state).unwrap();
    let private = engine.find([0x31; 16], &name("private")).unwrap();
    assert_eq!(private.data, b"before");
    assert_eq!(private.attributes, SIG_DB_ATTRIBUTES);
    let digest = private.digest.clone();
    assert_eq!(digest.len(), 32);
    let before = engine.save();
    let next = include_bytes!("../../tests/fixtures/rustcrypto-private-next.auth");
    let mut tampered = next.to_vec();
    *tampered.last_mut().unwrap() ^= 1;
    assert_eq!(
        engine.set_variable("private", [0x31; 16], SIG_DB_ATTRIBUTES, &tampered),
        Err(SECURITY)
    );
    assert_eq!(engine.save(), before);
    assert_eq!(
        engine.set_variable("private", [0x31; 16], SIG_DB_ATTRIBUTES, next),
        Ok(())
    );
    let private = engine.find([0x31; 16], &name("private")).unwrap();
    assert_eq!(private.data, b"after");
    assert_eq!(private.digest, digest);
    let after = engine.save();
    assert_eq!(
        engine.set_variable("private", [0x31; 16], SIG_DB_ATTRIBUTES, next),
        Err(SECURITY)
    );
    assert_eq!(engine.save(), after);
    assert_eq!(Engine::restore(&after).unwrap().save(), after);
}

#[test]
fn signed_chain_requires_a_valid_intermediate_and_private_identity_survives_restore() {
    let [root, issuer, leaf] = chain();
    let mut engine = seeded(&root);
    let valid = update("db", IMAGE, &[leaf.clone(), issuer.clone()], 2);
    assert_eq!(
        engine.set_variable("db", IMAGE, SIG_DB_ATTRIBUTES, &valid),
        Ok(())
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
        Err(SECURITY)
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
    nonca.signature = BitString::from_bytes(&sign(
        &keys()[0],
        &RSA_PKCS1_SHA256,
        &nonca.tbs_certificate.to_der().unwrap(),
    ))
    .unwrap();
    assert_eq!(
        engine.set_variable(
            "db",
            IMAGE,
            SIG_DB_ATTRIBUTES,
            &update("db", IMAGE, &[leaf.clone(), nonca], 2)
        ),
        Err(SECURITY)
    );
    let private = update(
        "private",
        [0x31; 16],
        &[leaf.clone(), issuer.clone(), root.clone()],
        2,
    );
    assert_eq!(
        engine.set_variable("private", [0x31; 16], SIG_DB_ATTRIBUTES, &private),
        Ok(())
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
        Ok(())
    );
    let before = restored.save();
    let renamed = certificate("CN=DifferentLeaf", 2, "CN=Issuer", 1, false);
    let renamed_update = update_at("private", [0x31; 16], &[renamed, issuer], 2, 2);
    assert_eq!(
        restored.set_variable("private", [0x31; 16], SIG_DB_ATTRIBUTES, &renamed_update),
        Err(SECURITY)
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
        Err(SECURITY)
    );
    assert_eq!(restored.save(), before);
    for replay in [&private, &next] {
        assert_eq!(
            restored.set_variable("private", [0x31; 16], SIG_DB_ATTRIBUTES, replay),
            Err(SECURITY)
        );
        assert_eq!(restored.save(), before);
    }
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
            Err(SECURITY)
        );
        assert_eq!(engine.save(), before);
        let direct = update(text, GLOBAL, std::slice::from_ref(&root), 0);
        assert_eq!(
            engine.set_variable(text, GLOBAL, SIG_DB_ATTRIBUTES, &direct),
            Ok(())
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
        Ok(())
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
            Err(SECURITY)
        );
        let leaf = certificate("CN=Leaf", 2, "CN=KEK", 1, false);
        let accepted = update(text, guid, &[leaf], 2);
        assert_eq!(
            engine.set_variable(text, guid, SIG_DB_ATTRIBUTES, &accepted),
            Ok(())
        );
        let mut engine = Engine::restore(&state).unwrap();
        assert_eq!(
            engine.set_variable(
                text,
                guid,
                SIG_DB_ATTRIBUTES,
                &update(text, guid, std::slice::from_ref(&root), 0)
            ),
            Ok(())
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
        Err(INVALID)
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
        cert.signature = BitString::from_bytes(&sign(
            &keys()[0],
            &RSA_PKCS1_SHA256,
            &cert.tbs_certificate.to_der().unwrap(),
        ))
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
        Err(SECURITY)
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
        Ok(())
    );
    let mut restored = Engine::restore(&engine.save()).unwrap();
    let next = update_at("private", [0x31; 16], &[leaf, issuer], 2, 1);
    assert_eq!(
        restored.set_variable("private", [0x31; 16], SIG_DB_ATTRIBUTES, &next),
        Ok(())
    );
}
