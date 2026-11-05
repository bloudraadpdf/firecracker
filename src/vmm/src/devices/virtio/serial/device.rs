// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io::{self, Write};

use super::{SerialState, valid_name};
use crate::devices::virtio::device::VirtioDeviceType;
use crate::devices::virtio::queued::Protocol;

#[derive(Debug)]
pub struct Serial(SerialState);

impl Serial {
    pub fn name(&self) -> &str {
        &self.0.name
    }
    pub fn new(name: String) -> io::Result<Self> {
        if !valid_name(&name) {
            return Err(io::Error::other("invalid serial port name"));
        }
        Ok(Self(SerialState {
            name,
            ready: false,
            open: false,
            pending: Default::default(),
        }))
    }

    fn control(event: u16, value: u16, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 4];
        bytes.extend_from_slice(&event.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }
}

impl Protocol for Serial {
    type State = SerialState;
    const TYPE: VirtioDeviceType = VirtioDeviceType::Console;
    const ID: &'static str = "serial";
    const FEATURES: u64 = (1 << 32) | (1 << 1);
    const QUEUES: usize = 4;
    const REQUIRED: &'static [usize] = &[0, 1];
    const REQUEST_LIMIT: usize = 65536;
    const RESPONSE_LIMIT: usize = 65536;
    fn config(&self) -> &[u8] {
        &[0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]
    }
    fn write_config(&mut self, _offset: u64, _bytes: &[u8]) {}
    fn reset(&mut self) {
        self.0.ready = false;
        self.0.open = false;
        self.0.pending.clear();
    }
    fn process(
        &mut self,
        queue: usize,
        request: &[u8],
        capacity: usize,
    ) -> io::Result<Option<Vec<u8>>> {
        match queue {
            0 | 2 if !request.is_empty() || capacity == 0 => {
                Err(io::Error::other("invalid serial receive buffer"))
            }
            0 => Ok(None),
            1 if capacity == 0 => {
                io::stdout().lock().write_all(request)?;
                Ok(Some(Vec::new()))
            }
            2 => {
                let Some(message) = self.0.pending.front() else {
                    return Ok(None);
                };
                if message.len() > capacity {
                    return Err(io::Error::other("short serial control buffer"));
                }
                Ok(self.0.pending.pop_front())
            }
            3 if request.len() == 8 && capacity == 0 => {
                let id = u32::from_le_bytes(request[..4].try_into().unwrap());
                let event = u16::from_le_bytes(request[4..6].try_into().unwrap());
                let value = u16::from_le_bytes(request[6..8].try_into().unwrap());
                match (event, value) {
                    (0, 0) => self.reset(),
                    (0, 1) if !self.0.ready => {
                        self.0.ready = true;
                        self.0.pending.push_back(Self::control(1, 1, &[]));
                    }
                    (3, 1) if id == 0 && self.0.ready && !self.0.open => {
                        self.0.open = true;
                        self.0
                            .pending
                            .push_back(Self::control(7, 1, self.0.name.as_bytes()));
                        self.0.pending.push_back(Self::control(6, 1, &[]));
                    }
                    _ => {}
                }
                Ok(Some(Vec::new()))
            }
            _ => Err(io::Error::other("invalid serial queue buffer")),
        }
    }
    fn save(&self) -> Self::State {
        self.0.clone()
    }
    fn restore(state: &Self::State) -> io::Result<Self> {
        Self::new(state.name.clone())?;
        let valid = [
            Self::control(1, 1, &[]),
            Self::control(7, 1, state.name.as_bytes()),
            Self::control(6, 1, &[]),
        ];
        let valid = if state.open {
            &valid[..]
        } else if state.ready {
            &valid[..1]
        } else {
            &valid[..0]
        };
        if (state.open && !state.ready)
            || state.pending.len() > valid.len()
            || !state
                .pending
                .iter()
                .eq(valid[valid.len().saturating_sub(state.pending.len())..].iter())
        {
            return Err(io::Error::other("invalid serial handshake state"));
        }
        Ok(Self(state.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_port_handshake_survives_restore() {
        let mut port = Serial::new("com.toets.0".into()).unwrap();
        port.process(3, &Serial::control(0, 1, &[]), 0).unwrap();
        assert_eq!(
            port.process(2, &[], 8).unwrap(),
            Some(Serial::control(1, 1, &[]))
        );
        port.process(3, &Serial::control(3, 1, &[]), 0).unwrap();
        let mut invalid = port.save();
        invalid
            .pending
            .push_back(Serial::control(7, 1, b"com.toets.0"));
        Serial::restore(&invalid).unwrap_err();
        let mut restored = Serial::restore(&port.save()).unwrap();
        restored.process(2, &[], 8).unwrap_err();
        assert_eq!(
            restored.process(2, &[], 128).unwrap(),
            Some(Serial::control(7, 1, b"com.toets.0"))
        );
        assert_eq!(
            restored.process(2, &[], 8).unwrap(),
            Some(Serial::control(6, 1, &[]))
        );
        assert_eq!(restored.process(2, &[], 8).unwrap(), None);
        for _ in 0..100 {
            restored.process(3, &Serial::control(3, 1, &[]), 0).unwrap();
        }
        assert_eq!(restored.process(2, &[], 8).unwrap(), None);
        restored.process(1, b"bad direction", 1).unwrap_err();
        restored.reset();
        assert_eq!(
            restored.save(),
            Serial::new("com.toets.0".into()).unwrap().save()
        );
    }
}
