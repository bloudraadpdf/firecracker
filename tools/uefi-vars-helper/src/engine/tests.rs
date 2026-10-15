// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0
use super::*;

fn frame(guid: Guid, payload: &[u8]) -> Vec<u8> {
    let mut buffer = guid.to_vec();
    buffer.extend(u64::try_from(payload.len()).unwrap().to_le_bytes());
    buffer.extend(payload);
    buffer
}
fn variable_frame(function: u64, payload: &[u8]) -> Vec<u8> {
    let mut body = function.to_le_bytes().to_vec();
    body.extend(0u64.to_le_bytes());
    body.extend(payload);
    frame(VARIABLE_PROTOCOL, &body)
}
fn policy(lock: u8, text: &str) -> Vec<u8> {
    let mut bytes = vec![0; 44];
    bytes[..4].copy_from_slice(&0x10000u32.to_le_bytes());
    bytes[6..8].copy_from_slice(&44u16.to_le_bytes());
    bytes[8..24].copy_from_slice(&[0x31; 16]);
    bytes[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
    bytes[40] = lock;
    if !text.is_empty() {
        bytes.extend(name(text));
    }
    let length = u16::try_from(bytes.len()).unwrap();
    bytes[4..6].copy_from_slice(&length.to_le_bytes());
    bytes
}
fn restored_locked(engine: &Engine, text: &str) -> Engine {
    let mut restored = Engine::restore(&engine.save()).unwrap();
    assert_eq!(
        restored.set_variable(text, [0x31; 16], 7, b"changed"),
        Ok(PROTECTED)
    );
    restored
}
#[test]
fn every_truncated_protocol_request_returns_an_error_without_panicking() {
    for guid in [VARIABLE_PROTOCOL, POLICY_PROTOCOL] {
        for command in 0u64..=15 {
            for size in 0..160 {
                let mut payload = vec![0xff; size];
                if guid == VARIABLE_PROTOCOL && size >= 8 {
                    put64(&mut payload, 0, command);
                }
                if guid == POLICY_PROTOCOL && size >= 12 {
                    payload[..4].copy_from_slice(b"VCPC");
                    put32(&mut payload, 4, 1);
                    put32(&mut payload, 8, usize::try_from(command).unwrap());
                }
                let mut engine = Engine::new();
                engine.transact(&mut frame(guid, &payload)).unwrap();
            }
        }
    }
}
#[test]
fn get_variable_reports_required_size_and_runtime_visibility() {
    let mut engine = Engine::new();
    engine.cold_boot().unwrap();
    assert_eq!(
        engine.set_variable("sample", [0x31; 16], NV | BS, b"payload"),
        Ok(0)
    );
    let mut request = set_message("sample", [0x31; 16], 0, &[]);
    put64(&mut request, 24, 1);
    assert_eq!(engine.transact(&mut request), Ok(0));
    assert_eq!(u64_at(&request, 32), Ok(TOO_SMALL));
    assert_eq!(size_at(&request, 56), Ok(7));
    engine.transact(&mut frame(EXIT, &[])).unwrap();
    put64(&mut request, 56, 0);
    engine.transact(&mut request).unwrap();
    assert_eq!(u64_at(&request, 32), Ok(NOT_FOUND));
    assert_eq!(
        engine.set_variable("sample", [0x31; 16], NV | BS, b"changed"),
        Ok(INVALID)
    );
}

#[test]
fn appended_large_variable_remains_readable_after_runtime_restore() {
    let mut engine = Engine::new();
    for (attributes, data) in [(7, vec![0x31; 40000]), (7 | APPEND, vec![0xff; 40000])] {
        let mut request = set_message("large", [0x31; 16], attributes, &data);
        assert_eq!(engine.transact(&mut request), Ok(0));
        assert_eq!(u64_at(&request, 32), Ok(0));
    }
    engine.transact(&mut frame(EXIT, &[])).unwrap();
    let mut restored = Engine::restore(&engine.save()).unwrap();
    let mut request = set_message("large", [0x31; 16], 0, &vec![0; 80000]);
    put64(&mut request, 24, 1);
    assert_eq!(restored.transact(&mut request), Ok(0));
    assert_eq!(u64_at(&request, 32), Ok(0));
    let data = &request[76 + name("large").len()..];
    assert_eq!(&data[..40000], vec![0x31; 40000]);
    assert_eq!(&data[40000..], vec![0xff; 40000]);
    let mut info = variable_frame(4, &[0; 28]);
    for (attributes, digest) in [(7, 0), (7 | AUTH, 32), (7 | AUTH | APPEND, 32)] {
        info[64..68].copy_from_slice(&attributes.to_le_bytes());
        restored.transact(&mut info).unwrap();
        assert_eq!(u64_at(&info, 32), Ok(0));
        assert_eq!(size_at(&info, 56), Ok(STORAGE - VARIABLE_OVERHEAD - digest));
    }
}
#[test]
fn lock_on_create_and_specific_names_override_namespace_policy() {
    let mut engine = Engine::new();
    engine.register_policy(&policy(1, "")).unwrap();
    engine.register_policy(&policy(2, "sample")).unwrap();
    assert_eq!(
        engine.set_variable("other", [0x31; 16], 7, b"value"),
        Ok(PROTECTED)
    );
    assert_eq!(
        engine.set_variable("sample", [0x31; 16], 7, b"value"),
        Ok(0)
    );
    let mut restored = restored_locked(&engine, "sample");
    restored.cold_boot().unwrap();
    assert_eq!(
        restored.set_variable("sample", [0x31; 16], 7, b"changed"),
        Ok(0)
    );
}
#[test]
fn policy_state_lock_and_hex_wildcard_are_enforced() {
    let mut engine = Engine::new();
    engine.register_policy(&policy(1, "Boot####")).unwrap();
    assert_eq!(
        engine.set_variable("Boot00Af", [0x31; 16], 7, b"value"),
        Ok(PROTECTED)
    );
    assert_eq!(
        engine.set_variable("Boot00AZ", [0x31; 16], 7, b"value"),
        Ok(0)
    );
    let mut bytes = policy(3, "sample");
    let variable_name = bytes.split_off(44);
    bytes.extend([0x31; 16]);
    bytes.extend([1, 0]);
    bytes.extend(name("gate"));
    let offset = u16::try_from(bytes.len()).unwrap();
    bytes[6..8].copy_from_slice(&offset.to_le_bytes());
    bytes.extend(variable_name);
    let length = u16::try_from(bytes.len()).unwrap();
    bytes[4..6].copy_from_slice(&length.to_le_bytes());
    engine.register_policy(&bytes).unwrap();
    assert_eq!(
        engine.set_variable("sample", [0x31; 16], 7, b"value"),
        Ok(0)
    );
    assert_eq!(engine.set_variable("gate", [0x31; 16], 7, &[1]), Ok(0));
    restored_locked(&engine, "sample");
}
#[test]
fn lifecycle_events_are_monotonic_and_malformed_events_do_not_mutate_state() {
    let mut engine = Engine::new();
    for guid in [END_DXE, READY, EXIT] {
        assert_eq!(engine.transact(&mut frame(guid, &[])), Ok(0));
    }
    let before = engine.save();
    assert_eq!(before.flags, 7);
    assert_eq!(engine.transact(&mut frame(EXIT, &[0])), Ok(0x12));
    assert_eq!(engine.save(), before);
    let mut restored = Engine::restore(&before).unwrap();
    assert_eq!(
        restored.set_variable("runtime", [0x31; 16], NV | BS, b"data"),
        Ok(INVALID)
    );
    restored.cold_boot().unwrap();
    assert_eq!(restored.save().flags, 0);
}
#[test]
fn enumeration_rejects_overflow_and_never_exposes_nonruntime_variables() {
    let mut engine = Engine::new();
    engine.cold_boot().unwrap();
    let mut payload = [0u8; 24];
    payload[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
    let mut request = variable_frame(2, &payload);
    engine.transact(&mut request).unwrap();
    assert_eq!(u64_at(&request, 32), Ok(BAD_SIZE));
    let state = engine.save();
    let mut duplicate = state.clone();
    duplicate.variables.push(duplicate.variables[0].clone());
    assert!(Engine::restore(&duplicate).is_err());
    let mut invalid = state;
    invalid.variables[0].name.push(0);
    assert!(Engine::restore(&invalid).is_err());
}

#[test]
fn authenticated_signature_binds_every_update_parameter_and_requires_enrolled_key() {
    use der::{Decode, Encode, asn1::BitString};
    let esl = include_bytes!("../../tests/fixtures/test.esl");
    let seed = |database: &[u8]| State {
        flags: 0,
        variables: [name("PK"), name("KEK")]
            .into_iter()
            .map(|name| Variable {
                guid: GLOBAL,
                name,
                data: database.to_vec(),
                digest: Vec::new(),
                attributes: SIG_DB_ATTRIBUTES,
                time: [0; 16],
            })
            .collect(),
        policies: Vec::new(),
    };
    let mut engine = Engine::restore(&seed(esl)).unwrap();
    engine.cold_boot().unwrap();
    let auth = include_bytes!("../../tests/fixtures/db.auth");
    let before = engine.save();
    for (name, guid, attributes) in [
        ("dbx", IMAGE, 0x27),
        ("db", GLOBAL, 0x27),
        ("db", IMAGE, 0x67),
    ] {
        assert_eq!(
            engine.set_variable(name, guid, attributes, auth),
            Ok(SECURITY)
        );
        assert_eq!(engine.save(), before);
    }
    let mut wrong =
        x509_cert::Certificate::from_der(include_bytes!("../../tests/fixtures/test.der")).unwrap();
    let mut key = wrong
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .as_bytes()
        .unwrap()
        .to_vec();
    key[12] ^= 0x10;
    wrong
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key = BitString::from_bytes(&key).unwrap();
    let encoded = wrong.to_der().unwrap();
    let mut database = esl[..44].to_vec();
    database.extend(encoded);
    let length = database.len();
    put32(&mut database, 16, length);
    put32(&mut database, 24, length - 28);
    let mut engine = Engine::restore(&seed(&database)).unwrap();
    engine.cold_boot().unwrap();
    let before = engine.save();
    assert_eq!(engine.set_variable("db", IMAGE, 0x27, auth), Ok(SECURITY));
    assert_eq!(engine.save(), before);
}

#[test]
fn malformed_signature_lists_are_rejected_and_append_ignores_duplicate_owners() {
    let original = include_bytes!("../../tests/fixtures/test.esl");
    let mut duplicate = original.to_vec();
    duplicate[28..44].fill(0x42);
    assert_eq!(
        authentication::append_database(original, &duplicate).unwrap(),
        original
    );
    assert_eq!(
        authentication::append_database(&[], &[original.as_slice(), &duplicate].concat()).unwrap(),
        original
    );
    for offset in [16, 20, 24] {
        let mut invalid = original.to_vec();
        invalid[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(authentication::validate_database(&invalid, false).is_err());
    }
}

#[test]
fn append_cannot_exceed_policy_size_and_failure_is_atomic() {
    let mut engine = Engine::new();
    let mut limit = policy(0, "sample");
    limit[28..32].copy_from_slice(&4u32.to_le_bytes());
    engine.register_policy(&limit).unwrap();
    assert_eq!(engine.set_variable("sample", [0x31; 16], 7, b"123"), Ok(0));
    let before = engine.save();
    assert_eq!(
        engine.set_variable("sample", [0x31; 16], 7 | APPEND, b"45"),
        Ok(INVALID)
    );
    assert_eq!(engine.save(), before);
}

#[test]
fn edk2_policy_padding_is_not_part_of_validation_or_lock_semantics() {
    let mut engine = Engine::new();
    let mut bytes = policy(1, "sample");
    bytes[41..44].fill(0xaf);
    engine.register_policy(&bytes).unwrap();
    let mut restored = Engine::restore(&engine.save()).unwrap();
    assert_eq!(
        restored.set_variable("sample", [0x31; 16], 7, b"data"),
        Ok(PROTECTED)
    );
}

#[test]
fn runtime_delete_accepts_zero_attributes_and_empty_append_preserves_attributes() {
    let mut engine = Engine::new();
    assert_eq!(
        engine.set_variable("sample", [0x31; 16], 7, b"value"),
        Ok(0)
    );
    engine.transact(&mut frame(EXIT, &[])).unwrap();
    let before = engine.save();
    assert_eq!(
        engine.set_variable("sample", [0x31; 16], APPEND, &[]),
        Ok(INVALID)
    );
    assert_eq!(engine.save(), before);
    assert_eq!(engine.set_variable("sample", [0x31; 16], 0, &[]), Ok(0));
    assert!(
        !engine
            .save()
            .variables
            .iter()
            .any(|v| v.name == name("sample"))
    );
    assert_eq!(
        engine.set_variable("sample", [0x31; 16], 7, b"value"),
        Ok(0)
    );
    assert_eq!(
        engine.set_variable("sample", [0x31; 16], 0, b"ignored"),
        Ok(0)
    );
    assert!(
        !engine
            .save()
            .variables
            .iter()
            .any(|v| v.name == name("sample"))
    );
}

#[test]
fn equal_specificity_keeps_first_registered_policy_after_restore() {
    let mut engine = Engine::new();
    engine.register_policy(&policy(1, "Boot0###")).unwrap();
    engine.register_policy(&policy(0, "Boot#0##")).unwrap();
    for mut engine in [engine.clone(), Engine::restore(&engine.save()).unwrap()] {
        assert_eq!(
            engine.set_variable("Boot00AB", [0x31; 16], 7, b"value"),
            Ok(PROTECTED)
        );
    }
}

#[test]
fn image_namespace_and_recovery_names_never_fall_back_to_ordinary_variables() {
    let mut engine = Engine::new();
    for (text, guid) in [
        ("future-db", IMAGE),
        ("OsRecoveryOrder", GLOBAL),
        ("OsRecovery00Af", [0x31; 16]),
    ] {
        assert_ne!(engine.set_variable(text, guid, 7, b"value"), Ok(0));
        assert!(engine.save().variables.is_empty());
    }
}

#[test]
fn vendor_keys_nv_uses_its_edk2_namespace_and_cannot_be_written_by_guest() {
    let mut engine = Engine::new();
    engine.cold_boot().unwrap();
    assert!(engine.find(GLOBAL, &name("VendorKeysNv")).is_none());
    assert_eq!(
        engine
            .find(VENDOR_KEYS, &name("VendorKeysNv"))
            .unwrap()
            .data,
        [0]
    );
    let before = engine.save();
    assert_eq!(
        engine.set_variable("VendorKeysNv", VENDOR_KEYS, 7, &[1]),
        Ok(PROTECTED)
    );
    assert_eq!(engine.save(), before);
}

#[test]
fn invalid_or_multiple_platform_keys_cannot_be_loaded_as_enrolled_state() {
    let mut state = State {
        flags: 0,
        policies: Vec::new(),
        variables: vec![Variable {
            guid: GLOBAL,
            name: name("PK"),
            data: include_bytes!("../../tests/fixtures/test.esl").to_vec(),
            digest: Vec::new(),
            attributes: SIG_DB_ATTRIBUTES,
            time: [0; 16],
        }],
    };
    assert!(Engine::restore(&state).is_ok());
    state.variables[0]
        .data
        .extend(include_bytes!("../../tests/fixtures/test.esl"));
    assert!(Engine::restore(&state).is_err());
    state.variables[0].data = b"x".to_vec();
    assert!(Engine::restore(&state).is_err());
}

#[test]
fn platform_default_keys_are_read_only_and_unsupported_modes_cannot_be_restored() {
    let mut engine = Engine::new();
    for text in [
        "PKDefault",
        "KEKDefault",
        "dbDefault",
        "dbxDefault",
        "dbtDefault",
        "dbrDefault",
    ] {
        assert_eq!(
            engine.set_variable(text, GLOBAL, BS | RT, b"value"),
            Ok(PROTECTED)
        );
    }
    for text in ["AuditMode", "DeployedMode"] {
        assert_eq!(
            engine.set_variable(text, GLOBAL, BS | RT, &[1]),
            Ok(UNSUPPORTED)
        );
        let state = State {
            flags: 0,
            policies: Vec::new(),
            variables: vec![Variable {
                guid: GLOBAL,
                name: name(text),
                data: vec![1],
                digest: Vec::new(),
                attributes: BS | RT,
                time: [0; 16],
            }],
        };
        assert!(Engine::restore(&state).is_err());
    }
    assert!(engine.save().variables.is_empty());
}
