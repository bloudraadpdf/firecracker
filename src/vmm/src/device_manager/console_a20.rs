// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! System Control Port A (0x92), with QEMU/KVM-compatible latch semantics.
//!
//! Bit 1 records the requested A20 gate state. KVM does not expose an A20-mask
//! interface: CPU addresses do not wrap when software clears this bit. The
//! i8042 output-port A20 latch has the same limitation. This device does not
//! claim to emulate the CPU address mask.

use std::sync::{Arc, Barrier};

use serde::{Deserialize, Serialize};
use vmm_sys_util::eventfd::EventFd;

use crate::vstate::bus::BusDevice;

pub const SYSTEM_CONTROL_PORT: u16 = 0x92;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
pub struct Port92State {
    output: u8,
}

#[derive(Debug)]
pub struct Port92 {
    state: Port92State,
    reset_evt: EventFd,
}

impl Port92 {
    pub fn new(reset_evt: EventFd, state: Option<&Port92State>) -> Self {
        Self {
            reset_evt,
            state: state.copied().unwrap_or_default(),
        }
    }

    pub fn save(&self) -> Port92State {
        self.state
    }
}

impl BusDevice for Port92 {
    fn read(&mut self, _base: u64, offset: u64, data: &mut [u8]) {
        if offset == 0 && data.len() == 1 {
            data[0] = self.state.output;
        }
    }

    fn write(&mut self, _base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        if offset != 0 || data.len() != 1 {
            return None;
        }
        let rising_reset = self.state.output & 1 == 0 && data[0] & 1 != 0;
        self.state.output = data[0];
        if rising_reset {
            crate::logger::info!("Guest requested system control port reset");
            if let Err(error) = self.reset_evt.write(1) {
                crate::logger::error!("System control reset failed: {error}");
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port() -> Port92 {
        Port92::new(EventFd::new(libc::EFD_NONBLOCK).unwrap(), None)
    }

    #[test]
    fn a20_latch_and_other_bits_read_back_without_reset() {
        let mut port = port();
        for value in [0, 2, 0x82, 0x80, 0] {
            port.write(0, 0, &[value]);
            let mut result = [0xff];
            port.read(0, 0, &mut result);
            assert_eq!(result, [value]);
            port.reset_evt.read().unwrap_err();
        }
    }

    #[test]
    fn reset_is_edge_triggered_and_restore_does_not_retrigger() {
        let mut original = port();
        original.write(0, 0, &[3]);
        assert_eq!(original.reset_evt.read().unwrap(), 1);
        original.write(0, 0, &[3]);
        original.reset_evt.read().unwrap_err();
        let saved = original.save();
        let mut restored = Port92::new(EventFd::new(libc::EFD_NONBLOCK).unwrap(), Some(&saved));
        let mut result = [0];
        restored.read(0, 0, &mut result);
        assert_eq!(result, [3]);
        restored.write(0, 0, &[3]);
        restored.reset_evt.read().unwrap_err();
        restored.write(0, 0, &[2]);
        restored.write(0, 0, &[3]);
        assert_eq!(restored.reset_evt.read().unwrap(), 1);
    }

    #[test]
    fn unsupported_accesses_do_not_change_state_or_reset() {
        let mut port = port();
        port.write(0, 1, &[1]);
        port.write(0, 0, &[1, 2]);
        port.reset_evt.read().unwrap_err();
        assert_eq!(port.save().output, 0);
        let mut result = [5, 6];
        port.read(0, 0, &mut result);
        assert_eq!(result, [5, 6]);
    }
}
