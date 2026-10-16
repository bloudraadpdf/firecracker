// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Architecture-selected input behind the stable console wire protocol.

use std::sync::{Arc, Mutex};

#[cfg(target_arch = "x86_64")]
use super::console_usb::UsbDevice;
#[cfg(target_arch = "x86_64")]
use crate::devices::legacy::I8042Device;
#[cfg(any(target_arch = "aarch64", test))]
use crate::devices::virtio::device::VirtioDevice;
#[cfg(any(target_arch = "aarch64", test))]
use crate::devices::virtio::input::{Input, InputError};

#[derive(Debug)]
pub(super) enum ConsoleInput {
    #[cfg(target_arch = "x86_64")]
    Legacy {
        keyboard: Arc<Mutex<I8042Device>>,
        tablet: Option<Arc<Mutex<UsbDevice>>>,
    },
    #[cfg(any(target_arch = "aarch64", test))]
    Virtio(Arc<Mutex<dyn VirtioDevice>>),
}

#[cfg(any(target_arch = "aarch64", test))]
fn accepted(result: Result<(), InputError>) -> bool {
    match result {
        Ok(()) => true,
        Err(InputError::PendingFull | InputError::InvalidHostEvent) => false,
        Err(error) => panic!("console input failed: {error}"),
    }
}

impl ConsoleInput {
    pub(super) fn key(&self, code: u8, extended: bool, pressed: bool) -> bool {
        match self {
            #[cfg(target_arch = "x86_64")]
            Self::Legacy { keyboard, .. } => {
                let mut input = keyboard.lock().unwrap();
                match code {
                    0xe1 if pressed => input.inject_pause(),
                    0xe1 => return true,
                    0xe0 => input.inject_print_screen(pressed),
                    _ => input.inject_key(code, extended, pressed),
                }
                .is_ok()
            }
            #[cfg(any(target_arch = "aarch64", test))]
            Self::Virtio(input) => {
                let mut device = input.lock().unwrap();
                let input = device.as_mut_any().downcast_mut::<Input>().unwrap();
                let key = crate::devices::keyboard::evdev_key(code, extended).unwrap();
                if code == 0xe1 {
                    !pressed || accepted(input.pulse(key))
                } else {
                    accepted(input.key(key, pressed))
                }
            }
        }
    }

    pub(super) fn pointer(&self, x: u16, y: u16, buttons: u8, wheel: i8) -> bool {
        match self {
            #[cfg(target_arch = "x86_64")]
            Self::Legacy { tablet, .. } => tablet
                .as_ref()
                .is_some_and(|tablet| tablet.lock().unwrap().pointer(x, y, buttons, wheel).is_ok()),
            #[cfg(any(target_arch = "aarch64", test))]
            Self::Virtio(input) => accepted(
                input
                    .lock()
                    .unwrap()
                    .as_mut_any()
                    .downcast_mut::<Input>()
                    .unwrap()
                    .pointer(x, y, buttons, wheel),
            ),
        }
    }

    pub(super) fn feedback(&self) -> [u8; 3] {
        match self {
            #[cfg(target_arch = "x86_64")]
            Self::Legacy { keyboard, tablet } => {
                let ready = tablet
                    .as_ref()
                    .is_some_and(|tablet| tablet.lock().unwrap().tablet_ready());
                let input = keyboard.lock().unwrap();
                [
                    u8::from(ready),
                    input.keyboard_leds(),
                    input.keyboard_pending_leds(),
                ]
            }
            #[cfg(any(target_arch = "aarch64", test))]
            Self::Virtio(input) => {
                let device = input.lock().unwrap();
                let input = device.as_any().downcast_ref::<Input>().unwrap();
                let ps2_order = |leds: u8| (leds & 4) >> 2 | (leds & 1) << 1 | (leds & 2) << 1;
                [
                    u8::from(input.is_activated()),
                    ps2_order(input.led_state()),
                    ps2_order(input.pending_led_state()),
                ]
            }
        }
    }
}
