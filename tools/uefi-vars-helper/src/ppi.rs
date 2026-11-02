// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0
use crate::engine::{Engine, State};

pub(crate) const GUID: [u8; 16] = [
    0xc1, 0xc5, 0xb9, 0xae, 0xf1, 0x94, 0x02, 0x4d, 0xbf, 0xd9, 0x46, 0x02, 0xdb, 0x2d, 0x3c, 0x54,
];
pub(crate) const REQUEST: &str = "Tcg2PhysicalPresence";
pub(crate) const FLAGS: &str = "Tcg2PhysicalPresenceFlags";

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidRequest,
    InvalidState,
}

fn variable<const N: usize>(state: &State, name: &str) -> Option<[u8; N]> {
    let name: Vec<_> = name
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    state
        .variables
        .iter()
        .find(|v| v.guid == GUID && v.name == name && v.attributes == 7)
        .and_then(|v| v.data.as_slice().try_into().ok())
}

pub fn transact(engine: &mut Engine, payload: &[u8]) -> Result<[u32; 3], Error> {
    let bytes: &[u8; 16] = payload.try_into().map_err(|_| Error::InvalidRequest)?;
    let [function, revision, operation, argument] =
        std::array::from_fn(|i| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap()));
    let valid_revision = match function {
        3 | 7 => matches!(revision, 1 | 2),
        2 | 5 | 8 => revision == 1,
        _ => false,
    };
    if !valid_revision
        || (matches!(function, 3 | 5) && operation != 0)
        || (argument != 0 && !(function == 7 && revision == 2 && operation == 23))
    {
        return Err(Error::InvalidRequest);
    }
    let supported = matches!(operation, 0 | 5 | 14 | 21 | 22);
    if matches!(function, 2 | 7 | 8) && !supported {
        return Ok([u32::from(function != 8), 0, 0]);
    }
    let state = engine.save();
    let record = variable::<16>(&state, REQUEST);
    match function {
        3 => Ok(record.map_or([1, 0, 0], |data| {
            [
                0,
                u32::from(data[0]),
                if revision == 2 {
                    u32::from_le_bytes(data[4..8].try_into().unwrap())
                } else {
                    0
                },
            ]
        })),
        5 => Ok(record.map_or([1, 0, 0], |data| {
            [
                0,
                u32::from(data[8]),
                u32::from_le_bytes(data[12..16].try_into().unwrap()),
            ]
        })),
        2 | 7 => {
            let Some(mut data) = record.filter(|_| variable::<4>(&state, FLAGS).is_some()) else {
                return Ok([2, 0, 0]);
            };
            data[0] = u8::try_from(operation).unwrap();
            data[4..8].copy_from_slice(&argument.to_le_bytes());
            let code = if engine.set_variable(REQUEST, GUID, 7, &data).is_ok() {
                0
            } else {
                2
            };
            Ok([code, 0, 0])
        }
        8 => {
            let flags = u32::from_le_bytes(variable(&state, FLAGS).ok_or(Error::InvalidState)?);
            record.ok_or(Error::InvalidState)?;
            let required = match operation {
                0 => false,
                5 => flags & 2 != 0,
                _ => flags & (2 | 16) != 0,
            };
            Ok([if required { 3 } else { 4 }, 0, 0])
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(function: u32, revision: u32, operation: u32, argument: u32) -> Vec<u8> {
        [function, revision, operation, argument]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect()
    }
    fn call(engine: &mut Engine, function: u32, revision: u32, operation: u32) -> [u32; 3] {
        transact(engine, &payload(function, revision, operation, 0)).unwrap()
    }
    fn firmware() -> Engine {
        let mut engine = Engine::new();
        engine.cold_boot().unwrap();
        assert_eq!(engine.set_variable(REQUEST, GUID, 7, &[0; 16]), Ok(()));
        assert_eq!(
            engine.set_variable(FLAGS, GUID, 7, &0x700e2u32.to_le_bytes()),
            Ok(())
        );
        engine
    }
    fn lock(engine: &mut Engine, name: &str) {
        let name: Vec<_> = name
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut state = engine.save();
        let mut policy = vec![0; 44];
        policy[..4].copy_from_slice(&0x10000u32.to_le_bytes());
        policy[4..6].copy_from_slice(&u16::try_from(44 + name.len()).unwrap().to_le_bytes());
        policy[6..8].copy_from_slice(&44u16.to_le_bytes());
        policy[8..24].copy_from_slice(&GUID);
        policy[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
        policy[40] = 1;
        policy.extend(name);
        state.policies.push(policy);
        state.flags = 1;
        *engine = Engine::restore(&state).unwrap();
    }
    #[test]
    fn mandatory_requests_persist_without_changing_previous_result() {
        let mut engine = firmware();
        let mut record = [0; 16];
        record[8] = 5;
        record[12..].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        assert_eq!(engine.set_variable(REQUEST, GUID, 7, &record), Ok(()));
        for operation in [5, 14, 21, 22, 0] {
            assert_eq!(call(&mut engine, 7, 2, operation), [0, 0, 0]);
            engine = Engine::restore(&engine.save()).unwrap();
            engine.cold_boot().unwrap();
            assert_eq!(call(&mut engine, 3, 1, 0), [0, operation, 0]);
            assert_eq!(call(&mut engine, 3, 2, 0), [0, operation, 0]);
            assert_eq!(call(&mut engine, 5, 1, 0), [0, 5, 0xffff_fff0]);
        }
    }
    #[test]
    fn pending_revision_one_omits_parameter_and_submit_replaces_it() {
        let mut engine = firmware();
        let mut record = [0; 16];
        record[0] = 23;
        record[4..8].copy_from_slice(&0x12u32.to_le_bytes());
        assert_eq!(engine.set_variable(REQUEST, GUID, 7, &record), Ok(()));
        assert_eq!(call(&mut engine, 3, 1, 0), [0, 23, 0]);
        assert_eq!(call(&mut engine, 3, 2, 0), [0, 23, 0x12]);
        assert_eq!(call(&mut engine, 2, 1, 5), [0, 0, 0]);
        assert_eq!(call(&mut engine, 3, 2, 0), [0, 5, 0]);
        let before = engine.save();
        assert_eq!(
            transact(&mut engine, &payload(7, 2, 23, 0x12)),
            Ok([1, 0, 0])
        );
        assert_eq!(engine.save(), before);
    }
    #[test]
    fn firmware_records_require_nonvolatile_runtime_attributes() {
        let mut engine = firmware();
        let original = engine.save();
        for name in [REQUEST, FLAGS] {
            let mut state = original.clone();
            let encoded: Vec<_> = name
                .encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect();
            state
                .variables
                .iter_mut()
                .find(|v| v.guid == GUID && v.name == encoded)
                .unwrap()
                .attributes = 6;
            engine = Engine::restore(&state).unwrap();
            assert_eq!(call(&mut engine, 7, 2, 5), [2, 0, 0]);
            assert_eq!(
                transact(&mut engine, &payload(8, 1, 5, 0)),
                Err(Error::InvalidState)
            );
        }
    }
    #[test]
    fn unsupported_operations_never_modify_flags_or_requests() {
        let mut engine = firmware();
        let before = engine.save();
        for operation in [1, 2, 17, 18, 23, 24, 128, 256, u32::MAX] {
            assert_eq!(call(&mut engine, 2, 1, operation), [1, 0, 0]);
            assert_eq!(call(&mut engine, 7, 1, operation), [1, 0, 0]);
            assert_eq!(call(&mut engine, 8, 1, operation), [0, 0, 0]);
        }
        assert_eq!(engine.save(), before);
    }
    #[test]
    fn confirmation_uses_clear_and_turn_on_flags() {
        let mut engine = firmware();
        for flags in [0u32, 2, 16, 18] {
            assert_eq!(
                engine.set_variable(FLAGS, GUID, 7, &flags.to_le_bytes()),
                Ok(())
            );
            assert_eq!(call(&mut engine, 8, 1, 0), [4, 0, 0]);
            assert_eq!(
                call(&mut engine, 8, 1, 5)[0],
                if flags & 2 != 0 { 3 } else { 4 }
            );
            for operation in [14, 21, 22] {
                assert_eq!(
                    call(&mut engine, 8, 1, operation)[0],
                    if flags & 18 != 0 { 3 } else { 4 }
                );
            }
        }
    }
    #[test]
    fn requests_cannot_bypass_variable_locks() {
        let mut engine = firmware();
        lock(&mut engine, FLAGS);
        assert_eq!(
            engine.set_variable(FLAGS, GUID, 7, &[0; 4]),
            Err(0x8000_0000_0000_0008)
        );
        assert_eq!(call(&mut engine, 7, 2, 5), [0, 0, 0]);
        assert_eq!(call(&mut engine, 8, 1, 5), [3, 0, 0]);
        lock(&mut engine, REQUEST);
        let before = engine.save();
        assert_eq!(call(&mut engine, 7, 2, 22), [2, 0, 0]);
        assert_eq!(engine.save(), before);
    }
    #[test]
    fn malformed_frames_revisions_and_parameters_are_rejected() {
        let mut engine = firmware();
        let before = engine.save();
        for message in [
            vec![],
            vec![0; 15],
            vec![0; 17],
            payload(9, 1, 0, 0),
            payload(2, 2, 5, 0),
            payload(3, 3, 0, 0),
            payload(5, 0, 0, 0),
            payload(7, 3, 5, 0),
            payload(8, 2, 5, 0),
            payload(3, 1, 5, 0),
            payload(5, 1, 0, 1),
            payload(2, 1, 5, 1),
            payload(7, 2, 5, 1),
        ] {
            assert_eq!(transact(&mut engine, &message), Err(Error::InvalidRequest));
        }
        assert_eq!(engine.save(), before);
    }
    #[test]
    fn missing_or_corrupt_firmware_state_is_not_reported_as_success() {
        let mut engine = Engine::new();
        engine.cold_boot().unwrap();
        assert_eq!(call(&mut engine, 3, 1, 0), [1, 0, 0]);
        assert_eq!(call(&mut engine, 5, 1, 0), [1, 0, 0]);
        assert_eq!(call(&mut engine, 7, 2, 5), [2, 0, 0]);
        assert_eq!(
            transact(&mut engine, &payload(8, 1, 5, 0)),
            Err(Error::InvalidState)
        );
        assert_eq!(engine.set_variable(REQUEST, GUID, 7, &[0; 15]), Ok(()));
        assert_eq!(call(&mut engine, 3, 2, 0), [1, 0, 0]);
        assert_eq!(engine.set_variable(REQUEST, GUID, 7, &[0; 16]), Ok(()));
        assert_eq!(engine.set_variable(FLAGS, GUID, 7, &[0; 3]), Ok(()));
        assert_eq!(call(&mut engine, 7, 2, 5), [2, 0, 0]);
    }
}
