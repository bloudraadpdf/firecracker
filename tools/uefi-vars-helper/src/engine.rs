// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0
mod authentication;
#[cfg(test)]
#[path = "engine/crypto_tests.rs"]
mod crypto_tests;
mod format;
mod protocol;
#[cfg(test)]
#[path = "engine/tests.rs"]
mod protocol_tests;
mod variable_policy;

use format::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    pub guid: Guid,
    pub name: Vec<u8>,
    pub data: Vec<u8>,
    pub digest: Vec<u8>,
    pub attributes: u32,
    pub time: [u8; 16],
}
impl Variable {
    fn matches(&self, guid: Guid, name: &[u8]) -> bool {
        self.guid == guid && self.name == name
    }
    fn visible(&self, runtime: bool) -> bool {
        !runtime || self.attributes & RT != 0
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub flags: u32,
    pub variables: Vec<Variable>,
    pub policies: Vec<Vec<u8>>,
}
#[derive(Clone)]
pub struct Engine {
    state: State,
}
impl Engine {
    #[cfg(test)]
    pub fn new() -> Self {
        Self {
            state: State {
                flags: 0,
                variables: Vec::new(),
                policies: Vec::new(),
            },
        }
    }
    fn find(&self, guid: Guid, name: &[u8]) -> Option<&Variable> {
        self.state.variables.iter().find(|v| v.matches(guid, name))
    }
    fn runtime(&self) -> bool {
        self.state.flags & 4 != 0
    }
    fn used(&self) -> usize {
        self.state
            .variables
            .iter()
            .map(|v| VARIABLE_OVERHEAD + v.name.len() + v.data.len() + v.digest.len())
            .sum::<usize>()
            + self.state.policies.iter().map(Vec::len).sum::<usize>()
    }
    fn bounded(&self) -> bool {
        self.state.variables.len() <= 4096
            && self.state.policies.len() <= 4096
            && self.used() <= STORAGE
    }
    pub fn cold_boot(&mut self) -> Result<(), &'static str> {
        let mut candidate = self.clone();
        candidate.state.flags = 0;
        candidate.state.policies.clear();
        candidate.state.variables.retain(|v| v.attributes & NV != 0);
        candidate.secure_boot_state();
        if !candidate.bounded() {
            return Err("cold boot exceeds storage limit");
        }
        *self = candidate;
        Ok(())
    }
    fn internal(&mut self, text: &str, guid: Guid, attributes: u32, data: Vec<u8>) {
        let name = name(text);
        self.state.variables.retain(|v| !v.matches(guid, &name));
        self.state.variables.push(Variable {
            guid,
            name,
            data,
            digest: Vec::new(),
            attributes,
            time: [0; 16],
        });
    }
    fn secure_boot_state(&mut self) {
        let enrolled = self.find(GLOBAL, &name("PK")).is_some();
        self.internal("SetupMode", GLOBAL, BS | RT, vec![u8::from(!enrolled)]);
        self.internal("SecureBoot", GLOBAL, BS | RT, vec![u8::from(enrolled)]);
        self.internal("SecureBootEnable", ENABLE, NV | BS, vec![1]);
        self.internal("CustomMode", CUSTOM, NV | BS, vec![0]);
        self.internal("VendorKeys", GLOBAL, BS | RT, vec![0]);
        self.internal("VendorKeysNv", VENDOR_KEYS, NV | BS | AUTH, vec![0]);
        self.internal(
            "SignatureSupport",
            GLOBAL,
            BS | RT,
            [HASH256, HASH384, HASH512, RSA2048, X509].concat(),
        );
    }
    pub fn save(&self) -> State {
        self.state.clone()
    }
    pub fn restore(state: &State) -> Result<Self, &'static str> {
        if state.flags & !15 != 0 || state.variables.len() > 4096 || state.policies.len() > 4096 {
            return Err("invalid state limits");
        }
        for (index, v) in state.variables.iter().enumerate() {
            if v.name.len() > MAX_FRAME
                || !valid_name(&v.name, false)
                || v.data.is_empty()
                || v.data.len() > STORAGE
                || !matches!(v.digest.len(), 0 | 32)
                || !valid_attributes(v.attributes)
                || state.variables[..index]
                    .iter()
                    .any(|old| old.matches(v.guid, &v.name))
            {
                return Err("invalid variable");
            }
        }
        for v in &state.variables {
            authentication::supported_variable(v.guid, &v.name)
                .map_err(|_| "unsupported variable")?;
            if authentication::secure_variable(v.guid, &v.name) && v.attributes != SIG_DB_ATTRIBUTES
            {
                return Err("invalid secure variable attributes");
            }
            if authentication::signature_database(v.guid, &v.name) {
                let platform_key = v.guid == GLOBAL && v.name == name("PK");
                authentication::validate_database(&v.data, platform_key)
                    .map_err(|_| "invalid signature database")?;
                if v.guid == GLOBAL {
                    authentication::validate_keys(&v.data)
                        .map_err(|_| "unsupported enrolled key")?;
                }
            }
        }
        for p in &state.policies {
            variable_policy::Policy::parse(p).map_err(|_| "invalid variable policy")?;
        }
        let e = Self {
            state: state.clone(),
        };
        if !e.bounded() {
            return Err("state exceeds storage limit");
        }
        Ok(e)
    }
    pub fn set_variable(
        &mut self,
        text: &str,
        guid: Guid,
        attributes: u32,
        data: &[u8],
    ) -> Result<u64, &'static str> {
        Ok(self
            .update(guid, &name(text), attributes, data)
            .err()
            .unwrap_or(0))
    }
    fn update(&mut self, guid: Guid, name: &[u8], attributes: u32, input: &[u8]) -> Status {
        if name.len() > MAX_FRAME || input.len() > MAX_FRAME {
            return Err(BAD_SIZE);
        }
        if !valid_name(name, false) || attributes & !(NV | BS | RT | AUTH | APPEND) != 0 {
            return Err(INVALID);
        }
        authentication::supported_variable(guid, name)?;
        let old = self.find(guid, name);
        let persistent_attributes = attributes & !APPEND;
        let append = attributes & APPEND != 0;
        let delete_without_access = persistent_attributes & (BS | RT | AUTH) == 0 && !append;
        let writing = (!input.is_empty() || append) && !delete_without_access;
        if (guid == GLOBAL
            && [
                "SetupMode",
                "SecureBoot",
                "SignatureSupport",
                "VendorKeys",
                "PKDefault",
                "KEKDefault",
                "dbDefault",
                "dbxDefault",
                "dbtDefault",
                "dbrDefault",
            ]
            .iter()
            .any(|n| name == format::name(n)))
            || (guid == VENDOR_KEYS && name == format::name("VendorKeysNv"))
            || (guid == CUSTOM && name == format::name("CustomMode"))
            || (guid == ENABLE
                && name == format::name("SecureBootEnable")
                && (input != [1] || attributes != NV | BS))
        {
            return Err(PROTECTED);
        }
        if self.runtime()
            && (old.is_some_and(|v| v.attributes & (NV | RT) != NV | RT)
                || (writing && persistent_attributes & (NV | RT) != NV | RT))
        {
            return Err(INVALID);
        }
        if old.is_some_and(|v| v.attributes & AUTH != 0) && attributes & AUTH == 0 {
            return Err(SECURITY);
        }
        if writing && !valid_attributes(persistent_attributes) {
            return Err(INVALID);
        }
        if old.is_some_and(|v| writing && v.attributes != persistent_attributes) {
            return Err(INVALID);
        }
        let secure = authentication::secure_variable(guid, name);
        if secure && persistent_attributes != SIG_DB_ATTRIBUTES {
            return Err(INVALID);
        }
        let mut value = Variable {
            guid,
            name: name.to_vec(),
            attributes: persistent_attributes,
            data: if delete_without_access {
                Vec::new()
            } else {
                input.to_vec()
            },
            digest: Vec::new(),
            time: [0; 16],
        };
        if attributes & AUTH != 0 {
            authentication::authenticate(self, &mut value, attributes, input)?;
        }
        let signature_database = authentication::signature_database(guid, name);
        let deleting = value.data.is_empty() && !append;
        if append {
            if let Some(old) = old {
                if signature_database {
                    value.data = authentication::append_database(&old.data, &value.data)?;
                } else {
                    let mut merged = old.data.clone();
                    merged.extend(&value.data);
                    value.data = merged;
                }
                if authentication::timestamp(&old.time) > authentication::timestamp(&value.time) {
                    value.time = old.time;
                }
            }
            if value.data.is_empty() {
                return Ok(());
            }
        }
        if signature_database && !value.data.is_empty() {
            authentication::validate_database(
                &value.data,
                guid == GLOBAL && name == format::name("PK"),
            )?;
            if guid == GLOBAL {
                authentication::validate_keys(&value.data)?;
            }
        }
        self.check_policy(
            guid,
            name,
            persistent_attributes,
            value.data.len(),
            deleting,
        )?;
        if deleting && old.is_none() {
            return Err(NOT_FOUND);
        }
        let mut candidate = self.clone();
        candidate.state.variables.retain(|v| !v.matches(guid, name));
        if !deleting {
            candidate.state.variables.push(value);
        }
        if guid == GLOBAL && name == format::name("PK") {
            candidate.secure_boot_state();
        }
        if !candidate.bounded() {
            return Err(NO_SPACE);
        }
        *self = candidate;
        Ok(())
    }
}
fn valid_attributes(attributes: u32) -> bool {
    attributes & !(NV | BS | RT | AUTH) == 0 && attributes & BS != 0
}
#[cfg(test)]
fn set_message(text: &str, vendor: Guid, attributes: u32, data: &[u8]) -> Vec<u8> {
    let name = name(text);
    let mut message = VARIABLE_PROTOCOL.to_vec();
    message.extend(
        u64::try_from(16 + 36 + name.len() + data.len())
            .unwrap()
            .to_le_bytes(),
    );
    message.extend(3u64.to_le_bytes());
    message.extend(0u64.to_le_bytes());
    message.extend(vendor);
    message.extend(u64::try_from(data.len()).unwrap().to_le_bytes());
    message.extend(u64::try_from(name.len()).unwrap().to_le_bytes());
    message.extend(attributes.to_le_bytes());
    message.extend(name);
    message.extend(data);
    message
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_preserves_variables() {
        let mut e = Engine::new();
        e.cold_boot().unwrap();
        let s = e.save();
        assert!(!s.variables.is_empty());
        assert_eq!(Engine::restore(&s).unwrap().save(), s);
    }
    #[test]
    fn large_signature_database_protocol_roundtrip_and_aggregate_bound() {
        let mut data = vec![
            0x26, 0x16, 0xc4, 0xc1, 0x4c, 0x50, 0x92, 0x40, 0xac, 0xa9, 0x41, 0xf9, 0x36, 0x93,
            0x43, 0x28,
        ];
        data.extend(67228u32.to_le_bytes());
        data.extend(0u32.to_le_bytes());
        data.extend(48u32.to_le_bytes());
        data.resize(67228, 0);
        for (index, entry) in data[28..].as_chunks_mut::<48>().0.iter_mut().enumerate() {
            entry[16..20].copy_from_slice(&u32::try_from(index).unwrap().to_le_bytes());
        }
        let variable = Variable {
            guid: [
                0xcb, 0xb2, 0x19, 0xd7, 0x3a, 0x3d, 0x96, 0x45, 0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67,
                0x65, 0x6f,
            ],
            name: name("dbx"),
            data,
            digest: Vec::new(),
            attributes: 0x27,
            time: [0; 16],
        };
        let mut state = State {
            flags: 0,
            variables: vec![variable],
            policies: Vec::new(),
        };
        assert_eq!(Engine::restore(&state).unwrap().save(), state);
        let mut engine = Engine::restore(&state).unwrap();
        let value = &state.variables[0];
        let mut request = set_message("dbx", value.guid, 0, &vec![0; value.data.len()]);
        put64(&mut request, 24, 1);
        assert_eq!(engine.transact(&mut request), Ok(0));
        assert_eq!(u64_at(&request, 32), Ok(0));
        assert_eq!(&request[76 + value.name.len()..], value.data);
        for index in 1..17u16 {
            let mut variable = state.variables[0].clone();
            variable.guid = [0x31; 16];
            variable.name = format!("db{index}\0")
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect();
            state.variables.push(variable);
        }
        assert!(Engine::restore(&state).is_err());
    }
    #[test]
    fn invalid_message_is_bounded() {
        let mut e = Engine::new();
        assert!(e.transact(&mut [0; 23]).is_err());
        let mut message = vec![0; 24];
        message[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(e.transact(&mut message), Ok(0x12));
    }
}

#[cfg(test)]
mod authentication_tests {
    use super::*;
    fn guid(hex: &str) -> [u8; 16] {
        std::array::from_fn(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap())
    }
    fn utf16(name: &str) -> Vec<u8> {
        name.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    }
    fn enrolled() -> Engine {
        let global = guid("61dfe48bca93d211aa0d00e098032b8c");
        let image = guid("cbb219d73a3d9645a3bcdad00e67656f");
        let variables = [("PK", global), ("KEK", global), ("db", image)]
            .into_iter()
            .map(|(name, guid)| Variable {
                guid,
                name: utf16(name),
                data: include_bytes!("../tests/fixtures/test.esl").to_vec(),
                digest: Vec::new(),
                attributes: 0x27,
                time: [0; 16],
            })
            .collect();
        let mut engine = Engine::restore(&State {
            flags: 0,
            variables,
            policies: Vec::new(),
        })
        .unwrap();
        engine.cold_boot().unwrap();
        engine
    }
    fn status(engine: &mut Engine, message: &mut [u8]) -> u64 {
        assert_eq!(engine.transact(message), Ok(0));
        u64::from_le_bytes(message[32..40].try_into().unwrap())
    }
    #[test]
    fn signed_update_passes_and_replay_fails_after_restore() {
        let mut engine = enrolled();
        let image = guid("cbb219d73a3d9645a3bcdad00e67656f");
        let mut message = set_message(
            "db",
            image,
            0x27,
            include_bytes!("../tests/fixtures/db.auth"),
        );
        assert_eq!(status(&mut engine, &mut message), 0);
        let mut restored = Engine::restore(&engine.save()).unwrap();
        assert_eq!(status(&mut restored, &mut message), 0x8000_0000_0000_001a);
    }
    #[test]
    fn invalid_signature_never_changes_store() {
        let mut engine = enrolled();
        let before = engine.save();
        let mut auth = include_bytes!("../tests/fixtures/db.auth").to_vec();
        *auth.last_mut().unwrap() ^= 1;
        let mut message = set_message("db", guid("cbb219d73a3d9645a3bcdad00e67656f"), 0x27, &auth);
        assert_eq!(status(&mut engine, &mut message), 0x8000_0000_0000_001a);
        assert_eq!(engine.save(), before);
    }
    #[test]
    fn cannot_disable_secure_boot_or_enter_custom_mode() {
        let mut engine = enrolled();
        for (name, vendor) in [
            ("SecureBootEnable", guid("c70ba3f008af564599c4001009c93a44")),
            ("CustomMode", guid("0cec76c028709943a07271ee5c448b9f")),
        ] {
            let value = if name == "CustomMode" { 1 } else { 0 };
            let mut message = set_message(name, vendor, 3, &[value]);
            assert_ne!(status(&mut engine, &mut message), 0);
        }
    }
    #[test]
    fn reserved_timestamp_fields_are_rejected() {
        let mut engine = enrolled();
        let mut message = set_message(
            "db",
            guid("cbb219d73a3d9645a3bcdad00e67656f"),
            0x27,
            include_bytes!("../tests/fixtures/db-invalid-time.auth"),
        );
        assert_eq!(status(&mut engine, &mut message), 0x8000_0000_0000_001a);
    }
    #[test]
    fn append_does_not_roll_back_timestamp() {
        let mut engine = enrolled();
        let mut initial = set_message(
            "db",
            guid("cbb219d73a3d9645a3bcdad00e67656f"),
            0x27,
            include_bytes!("../tests/fixtures/db.auth"),
        );
        assert_eq!(status(&mut engine, &mut initial), 0);
        let mut append = set_message(
            "db",
            guid("cbb219d73a3d9645a3bcdad00e67656f"),
            0x67,
            include_bytes!("../tests/fixtures/db-append-old.auth"),
        );
        assert_eq!(status(&mut engine, &mut append), 0);
        assert_eq!(status(&mut engine, &mut initial), 0x8000_0000_0000_001a);
    }
    #[test]
    fn accepts_bare_signed_data() {
        let mut engine = enrolled();
        let mut auth = include_bytes!("../tests/fixtures/db.auth").to_vec();
        let len = u32::from_le_bytes(auth[16..20].try_into().unwrap());
        auth.drain(40..59);
        auth[16..20].copy_from_slice(&(len - 19).to_le_bytes());
        let mut message = set_message("db", guid("cbb219d73a3d9645a3bcdad00e67656f"), 0x27, &auth);
        assert_eq!(status(&mut engine, &mut message), 0);
    }
    #[test]
    fn lock_policy_survives_snapshot() {
        let mut engine = enrolled();
        let vendor = [0x31; 16];
        let mut message = set_message("test", vendor, 7, b"original");
        assert_eq!(status(&mut engine, &mut message), 0);
        let name = utf16("test");
        let mut lock = guid("33d532ede69909429cc02d72cdd998a7").to_vec();
        lock.extend(u64::try_from(16 + 24 + name.len()).unwrap().to_le_bytes());
        lock.extend(8u64.to_le_bytes());
        lock.extend(0u64.to_le_bytes());
        lock.extend(vendor);
        lock.extend(u64::try_from(name.len()).unwrap().to_le_bytes());
        lock.extend(name);
        assert_eq!(status(&mut engine, &mut lock), 0);
        let mut state = engine.save();
        state.flags |= 1;
        let mut restored = Engine::restore(&state).unwrap();
        let mut update = set_message("test", vendor, 7, b"changed");
        assert_eq!(status(&mut restored, &mut update), 0x8000_0000_0000_0008);
    }

    #[test]
    fn signed_platform_key_deletion_updates_mode() {
        let mut engine = enrolled();
        let mut delete = set_message(
            "PK",
            guid("61dfe48bca93d211aa0d00e098032b8c"),
            0x27,
            include_bytes!("../tests/fixtures/pk-delete.auth"),
        );
        assert_eq!(status(&mut engine, &mut delete), 0);
        let state = engine.save();
        assert!(!state.variables.iter().any(|v| v.name == utf16("PK")));
        for (name, value) in [("SetupMode", 1), ("SecureBoot", 0)] {
            assert_eq!(
                state
                    .variables
                    .iter()
                    .find(|v| v.name == utf16(name))
                    .unwrap()
                    .data,
                [value],
                "{name}"
            );
        }
        assert_eq!(Engine::restore(&state).unwrap().save(), state);
    }
    #[test]
    fn unsigned_deletion_cannot_remove_platform_key() {
        let mut engine = enrolled();
        let mut delete = set_message("PK", guid("61dfe48bca93d211aa0d00e098032b8c"), 0x27, &[]);
        assert_eq!(status(&mut engine, &mut delete), 0x8000_0000_0000_001a);
    }
    #[test]
    fn derived_secure_boot_state_is_read_only() {
        let mut engine = enrolled();
        let mut update = set_message(
            "SecureBoot",
            guid("61dfe48bca93d211aa0d00e098032b8c"),
            6,
            &[0],
        );
        assert_eq!(status(&mut engine, &mut update), 0x8000_0000_0000_0008);
    }
    #[test]
    fn namespace_policy_survives_restore_and_locks_all_names() {
        let mut state = enrolled().save();
        let mut policy = vec![0; 44];
        policy[..4].copy_from_slice(&0x10000u32.to_le_bytes());
        policy[4..6].copy_from_slice(&44u16.to_le_bytes());
        policy[6..8].copy_from_slice(&44u16.to_le_bytes());
        policy[8..24].copy_from_slice(&[0x31; 16]);
        policy[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
        policy[40] = 1;
        state.policies.push(policy);
        state.flags = 1;
        let mut restored = Engine::restore(&state).unwrap();
        let mut message = set_message("AnyName", [0x31; 16], 7, b"new");
        assert_eq!(status(&mut restored, &mut message), 0x8000_0000_0000_0008);
    }
}
