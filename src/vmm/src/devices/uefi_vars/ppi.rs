// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
#[cfg(feature = "windows")]
use std::io;

#[cfg(feature = "windows")]
use super::client::invalid;

#[cfg(feature = "windows")]
pub(super) const BASE: u64 = 0x100;
#[cfg(feature = "windows")]
const BUSY: u32 = 1;
#[cfg(feature = "windows")]
const ERROR: u32 = 2;

/// Fixed-width physical-presence mailbox, checkpointed only while idle.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PpiRegisters {
    status: u32,
    request: [u32; 4],
    response: [u32; 3],
}

#[cfg(feature = "windows")]
impl PpiRegisters {
    pub(super) fn valid(&self) -> bool {
        match self.status {
            0 => self.response[0] <= 4,
            ERROR => self.response == [0; 3],
            _ => false,
        }
    }

    pub(super) fn read(&self, offset: u64, data: &mut [u8]) {
        data.fill(0);
        if data.len() != 4 || !offset.is_multiple_of(4) {
            return;
        }
        let value = match offset - BASE {
            0 => self.status,
            4..=16 => self.request[usize::try_from((offset - BASE) / 4 - 1).unwrap()],
            20..=28 => self.response[usize::try_from((offset - BASE) / 4 - 5).unwrap()],
            _ => return,
        };
        data.copy_from_slice(&value.to_le_bytes());
    }

    pub(super) fn write(&mut self, offset: u64, data: &[u8]) -> Option<Vec<u8>> {
        if self.status == BUSY || data.len() != 4 || !offset.is_multiple_of(4) {
            return None;
        }
        let value = u32::from_le_bytes(data.try_into().unwrap());
        match offset - BASE {
            0 if value == 1 => {
                self.response = [0; 3];
                if !self.request_valid() {
                    self.status = ERROR;
                    return None;
                }
                self.status = BUSY;
                Some(
                    self.request
                        .iter()
                        .flat_map(|value| value.to_le_bytes())
                        .collect(),
                )
            }
            4..=16 => {
                self.request[usize::try_from((offset - BASE) / 4 - 1).unwrap()] = value;
                None
            }
            _ => None,
        }
    }

    fn request_valid(&self) -> bool {
        let [function, revision, operation, argument] = self.request;
        match (function, revision) {
            (2 | 8, 1) => argument == 0,
            (3, 1 | 2) | (5, 1) => operation == 0 && argument == 0,
            (7, 1) => argument == 0,
            (7, 2) => operation == 23 || argument == 0,
            _ => false,
        }
    }

    pub(super) fn fail(&mut self) {
        self.status = ERROR;
        self.response = [0; 3];
    }

    pub(super) fn complete(&mut self, reply: &[u8]) -> io::Result<()> {
        if reply.len() != 12 {
            return Err(invalid("invalid physical-presence reply length"));
        }
        for (value, bytes) in self.response.iter_mut().zip(reply.as_chunks::<4>().0) {
            *value = u32::from_le_bytes(*bytes);
        }
        let [code, first, second] = self.response;
        let valid = match self.request[0] {
            2 => code <= 2 && first == 0 && second == 0,
            7 => code <= 3 && first == 0 && second == 0,
            8 => code <= 4 && first == 0 && second == 0,
            3 => code <= 1 && (self.request[1] == 2 || second == 0),
            5 => code <= 1,
            _ => false,
        };
        if !valid {
            return Err(invalid("invalid physical-presence reply status"));
        }
        self.status = 0;
        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "windows")]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_rejects_impossible_status_and_response() {
        let mut registers = PpiRegisters::default();
        assert!(registers.valid());
        for status in [BUSY, 3, u32::MAX] {
            registers.status = status;
            assert!(!registers.valid());
        }
        registers.status = 0;
        registers.response[0] = 4;
        registers.request[0] = 3;
        assert!(registers.valid());
        registers.response[0] = 5;
        assert!(!registers.valid());
        registers.status = ERROR;
        registers.response = [0, 1, 0];
        assert!(!registers.valid());
        registers.fail();
        assert!(registers.valid());
    }
}
