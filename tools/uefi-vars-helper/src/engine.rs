// SPDX-License-Identifier: GPL-2.0-or-later
use serde::{Deserialize, Serialize};
use std::{ffi::c_void, ptr::NonNull};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    pub guid: [u8; 16],
    pub name: Vec<u8>,
    pub data: Vec<u8>,
    pub digest: Vec<u8>,
    pub attributes: u32,
    pub time: [u8; 16],
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub flags: u32,
    pub variables: Vec<Variable>,
    pub policies: Vec<Vec<u8>>,
}
#[repr(C)]
struct RawVariable {
    guid: [u8; 16],
    name: *const u8,
    data: *const u8,
    digest: *const u8,
    name_size: u32,
    data_size: u32,
    digest_size: u32,
    attributes: u32,
    time: [u8; 16],
}
unsafe extern "C" {
    fn fcuv_new() -> *mut c_void;
    fn fcuv_free(engine: *mut c_void);
    fn fcuv_cold_boot(engine: *mut c_void);
    fn fcuv_flags(engine: *mut c_void) -> u32;
    fn fcuv_set_flags(engine: *mut c_void, flags: u32);
    fn fcuv_variable(engine: *mut c_void, index: u32, out: *mut RawVariable) -> bool;
    fn fcuv_put_variable(engine: *mut c_void, variable: *const RawVariable) -> bool;
    fn fcuv_policy(engine: *mut c_void, index: u32, size: *mut u32) -> *const u8;
    fn fcuv_put_policy(engine: *mut c_void, data: *const u8, size: u32) -> bool;
    fn fcuv_transact(engine: *mut c_void, buffer: *mut u8, size: u32) -> u32;
}
pub struct Engine(NonNull<c_void>);
impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            fcuv_free(self.0.as_ptr());
        }
    }
}
impl Engine {
    pub fn new() -> Self {
        Self(NonNull::new(unsafe { fcuv_new() }).expect("GLib aborts on allocation failure"))
    }
    pub fn cold_boot(&mut self) {
        unsafe {
            fcuv_cold_boot(self.0.as_ptr());
        }
    }
    pub fn transact(&mut self, buffer: &mut [u8]) -> Result<u32, &'static str> {
        if !(24..=65536).contains(&buffer.len()) {
            return Err("invalid MM buffer size");
        }
        let status = unsafe {
            fcuv_transact(
                self.0.as_ptr(),
                buffer.as_mut_ptr(),
                u32::try_from(buffer.len()).unwrap(),
            )
        };
        if status == u32::MAX {
            return Err("engine resource limit exceeded");
        }
        Ok(status)
    }
    pub fn save(&self) -> State {
        let mut state = State {
            flags: unsafe { fcuv_flags(self.0.as_ptr()) },
            variables: Vec::new(),
            policies: Vec::new(),
        };
        for index in 0.. {
            let mut raw = std::mem::MaybeUninit::<RawVariable>::uninit();
            if !unsafe { fcuv_variable(self.0.as_ptr(), index, raw.as_mut_ptr()) } {
                break;
            }
            let raw = unsafe { raw.assume_init() };
            // Pointers borrow the engine. No mutation occurs while these slices are copied.
            let copy = |ptr, size| {
                if size == 0 {
                    Vec::new()
                } else {
                    unsafe {
                        std::slice::from_raw_parts(ptr, usize::try_from(size).unwrap()).to_vec()
                    }
                }
            };
            state.variables.push(Variable {
                guid: raw.guid,
                name: copy(raw.name, raw.name_size),
                data: copy(raw.data, raw.data_size),
                digest: copy(raw.digest, raw.digest_size),
                attributes: raw.attributes,
                time: raw.time,
            });
        }
        for index in 0.. {
            let mut size = 0;
            let ptr = unsafe { fcuv_policy(self.0.as_ptr(), index, &mut size) };
            if ptr.is_null() {
                break;
            }
            state.policies.push(unsafe {
                std::slice::from_raw_parts(ptr, usize::try_from(size).unwrap()).to_vec()
            });
        }
        state
    }
    pub fn restore(state: &State) -> Result<Self, &'static str> {
        if state.flags & !15 != 0 || state.variables.len() > 4096 || state.policies.len() > 4096 {
            return Err("invalid state limits");
        }
        let e = Self::new();
        for v in &state.variables {
            if v.name.len() > 65536
                || v.data.len() > 1024 * 1024
                || !matches!(v.digest.len(), 0 | 32)
            {
                return Err("invalid variable size");
            }
            let raw = RawVariable {
                guid: v.guid,
                name: v.name.as_ptr(),
                data: v.data.as_ptr(),
                digest: v.digest.as_ptr(),
                name_size: u32::try_from(v.name.len()).unwrap(),
                data_size: u32::try_from(v.data.len()).unwrap(),
                digest_size: u32::try_from(v.digest.len()).unwrap(),
                attributes: v.attributes,
                time: v.time,
            };
            if !unsafe { fcuv_put_variable(e.0.as_ptr(), &raw) } {
                return Err("invalid variable");
            }
        }
        for p in &state.policies {
            if p.len() > 65536
                || !unsafe {
                    fcuv_put_policy(e.0.as_ptr(), p.as_ptr(), u32::try_from(p.len()).unwrap())
                }
            {
                return Err("invalid variable policy");
            }
        }
        unsafe {
            fcuv_set_flags(e.0.as_ptr(), state.flags);
        }
        Ok(e)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_preserves_variables() {
        let mut e = Engine::new();
        e.cold_boot();
        let s = e.save();
        assert!(!s.variables.is_empty());
        assert_eq!(Engine::restore(&s).unwrap().save(), s);
    }
    #[test]
    fn large_signature_database_roundtrip_and_aggregate_bound() {
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
            name: vec![b'd', 0, b'b', 0, 0, 0],
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
        for index in 1..17u16 {
            let mut variable = state.variables[0].clone();
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
        engine.cold_boot();
        engine
    }
    fn set_message(name: &str, vendor: [u8; 16], attributes: u32, data: &[u8]) -> Vec<u8> {
        let name = utf16(name);
        let mut message = guid("33d532ede69909429cc02d72cdd998a7").to_vec();
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
