// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

#[cfg(feature = "windows")]
pub(super) const EVENT_QUEUE: usize = 0;
#[cfg(feature = "windows")]
pub(super) const STATUS_QUEUE: usize = 1;
#[cfg(feature = "windows")]
pub(super) const NUM_QUEUES: usize = 2;
#[cfg(feature = "windows")]
pub(super) const CONFIG_SIZE: usize = 136;
#[cfg(feature = "windows")]
pub(super) const EVENT_SIZE: usize = 8;
#[cfg(feature = "windows")]
pub(super) const ABS_MAX: u16 = 32767;
#[cfg(feature = "windows")]
pub(super) const MAX_PENDING_FRAMES: usize = 64;
#[cfg(feature = "windows")]
pub(super) const MAX_FRAME_EVENTS: usize = 145;
#[cfg(feature = "windows")]
pub(super) const EV_SYN: u16 = 0;
#[cfg(feature = "windows")]
pub(super) const EV_KEY: u16 = 1;
#[cfg(feature = "windows")]
pub(super) const EV_REL: u16 = 2;
#[cfg(feature = "windows")]
pub(super) const EV_ABS: u16 = 3;
#[cfg(feature = "windows")]
pub(super) const EV_LED: u16 = 17;
#[cfg(feature = "windows")]
pub(super) const BTN_LEFT: u16 = 272;

#[cfg(feature = "windows")]
pub(super) fn keyboard_key(code: u16) -> bool {
    matches!(code, 1..=127 | 183..=194)
}

#[cfg(feature = "windows")]
pub(super) fn lock_mask(code: u16) -> u8 {
    match code {
        58 => 2,
        69 => 1,
        70 => 4,
        _ => 0,
    }
}

#[cfg(feature = "windows")]
pub(super) fn supported_key(code: u16) -> bool {
    keyboard_key(code) || (BTN_LEFT..BTN_LEFT + 3).contains(&code)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct InputEvent {
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
}

#[cfg(feature = "windows")]
impl InputEvent {
    pub const SYN: Self = Self::new(EV_SYN, 0, 0);

    pub const fn new(event_type: u16, code: u16, value: i32) -> Self {
        Self {
            event_type,
            code,
            value,
        }
    }

    pub fn bytes(self) -> [u8; EVENT_SIZE] {
        let mut bytes = [0; EVENT_SIZE];
        bytes[..2].copy_from_slice(&self.event_type.to_le_bytes());
        bytes[2..4].copy_from_slice(&self.code.to_le_bytes());
        bytes[4..].copy_from_slice(&self.value.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: [u8; EVENT_SIZE]) -> Self {
        Self::new(
            u16::from_le_bytes(bytes[..2].try_into().unwrap()),
            u16::from_le_bytes(bytes[2..4].try_into().unwrap()),
            i32::from_le_bytes(bytes[4..].try_into().unwrap()),
        )
    }

    pub fn is_supported(self) -> bool {
        match self.event_type {
            EV_SYN => self == Self::SYN,
            EV_KEY => {
                supported_key(self.code)
                    && ((0..=1).contains(&self.value)
                        || (keyboard_key(self.code) && self.value == 2))
            }
            EV_ABS => self.code <= 1 && (0..=i32::from(ABS_MAX)).contains(&self.value),
            EV_REL => self.code == 8 && (-127..=127).contains(&self.value),
            _ => false,
        }
    }
}

#[cfg(feature = "windows")]
pub(super) fn config(select: u8, subsel: u8) -> [u8; CONFIG_SIZE] {
    let mut bytes = [0; CONFIG_SIZE];
    bytes[0] = select;
    bytes[1] = subsel;
    let data = &mut bytes[8..];
    let size = match (select, subsel) {
        (1, 0) => {
            let name = b"Firecracker keyboard and pointer";
            data[..name.len()].copy_from_slice(name);
            name.len()
        }
        (3, 0) => {
            for (offset, value) in [(0, 6u16), (2, 0x1af4), (4, 18), (6, 1)] {
                data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
            }
            8
        }
        (0x10, 0) | (0x11, 0) => {
            data[0] = 1;
            1
        }
        (0x11, 1) => {
            for key in (1..=194)
                .filter(|&key| keyboard_key(key))
                .chain(BTN_LEFT..BTN_LEFT + 3)
            {
                data[usize::from(key / 8)] |= 1 << (key % 8);
            }
            usize::from((BTN_LEFT + 2) / 8) + 1
        }
        (0x11, 2) => {
            data[1] = 1;
            2
        }
        (0x11, 3) => {
            data[0] = 3;
            1
        }
        (0x11, 17) => {
            data[0] = 7;
            1
        }
        (0x12, 0..=1) => {
            data[4..8].copy_from_slice(&u32::from(ABS_MAX).to_le_bytes());
            20
        }
        _ => 0,
    };
    bytes[2] = u8::try_from(size).unwrap();
    bytes
}
