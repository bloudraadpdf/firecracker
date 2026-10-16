// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Barrier};

use serde::{Deserialize, Serialize};
use vmm_sys_util::eventfd::EventFd;

use crate::logger::{IncMetric, SharedIncMetric, error};
use crate::vstate::bus::BusDevice;

/// Errors thrown by the i8042 device.
#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum I8042Error {
    /// i8042 internal buffer full.
    InternalBufferFull,
    /// Could not trigger input interrupt: {0}.
    Interrupt(io::Error),
    /// Invalid i8042 snapshot state.
    InvalidState,
    /// Unsupported PS/2 key code.
    InvalidInput,
}

/// Metrics specific to the i8042 device.
#[derive(Debug, Serialize)]
pub(super) struct I8042DeviceMetrics {
    error_count: SharedIncMetric,
    missed_read_count: SharedIncMetric,
    missed_write_count: SharedIncMetric,
    read_count: SharedIncMetric,
    reset_count: SharedIncMetric,
    write_count: SharedIncMetric,
}

pub(super) static METRICS: I8042DeviceMetrics = I8042DeviceMetrics {
    error_count: SharedIncMetric::new(),
    missed_read_count: SharedIncMetric::new(),
    missed_write_count: SharedIncMetric::new(),
    read_count: SharedIncMetric::new(),
    reset_count: SharedIncMetric::new(),
    write_count: SharedIncMetric::new(),
};

const QUEUE_CAPACITY: usize = 256;
const KBD_IRQ: u8 = 1;
const AUX_IRQ: u8 = 2;
const KBD_DISABLED: u8 = 0x10;
const AUX_DISABLED: u8 = 0x20;
const TRANSLATE: u8 = 0x40;
const ACK: u8 = 0xfa;
const RESEND: u8 = 0xfe;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum Destination {
    Control,
    Output,
    KeyboardOutput,
    DiscardAuxiliary,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum KeyboardParameter {
    Leds,
    ScanSet,
    Typematic,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Keyboard {
    enabled: bool,
    scan_set: u8,
    leds: u8,
    expected_leds: u8,
    pending_leds: u8,
    locks_pressed: u8,
    typematic: u8,
    parameter: Option<KeyboardParameter>,
    last_byte: u8,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self {
            enabled: true,
            scan_set: 2,
            leds: 0,
            expected_leds: 0,
            pending_leds: 0,
            locks_pressed: 0,
            typematic: 0x2b,
            parameter: None,
            last_byte: ACK,
        }
    }
}

/// Saved controller, keyboard and pending output state.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct I8042State {
    control: u8,
    output: u8,
    command_write: bool,
    destination: Option<Destination>,
    queue: VecDeque<u8>,
    pending_input: VecDeque<u8>,
    keyboard: Keyboard,
}

impl Default for I8042State {
    fn default() -> Self {
        Self {
            control: 0x25,
            output: 3,
            command_write: false,
            destination: None,
            queue: VecDeque::new(),
            pending_input: VecDeque::new(),
            keyboard: Keyboard::default(),
        }
    }
}

/// Keyboard-only PS/2 controller at I/O ports 0x60 and 0x64.
#[derive(Debug)]
pub struct I8042Device {
    reset_evt: EventFd,
    /// Keyboard interrupt (IRQ 1).
    pub kbd_interrupt_evt: EventFd,
    state: I8042State,
}

impl I8042Device {
    /// Construct a controller whose reset requests terminate the VM.
    pub fn new(reset_evt: EventFd) -> io::Result<Self> {
        Ok(Self {
            reset_evt,
            kbd_interrupt_evt: EventFd::new(libc::EFD_NONBLOCK)?,
            state: I8042State::default(),
        })
    }

    /// Save pending commands and output as well as device configuration.
    pub fn save_state(&self) -> I8042State {
        self.state.clone()
    }

    /// Guest-selected PS/2 LEDs: Scroll Lock, Num Lock and Caps Lock in bits 0..2.
    pub fn keyboard_leds(&self) -> u8 {
        self.state.keyboard.leds
    }

    /// Caps/Num Lock transitions awaiting the guest's LED acknowledgement.
    pub fn keyboard_pending_leds(&self) -> u8 {
        self.state.keyboard.pending_leds
    }

    /// Restore validated state. IRQ routes must be registered before this call.
    pub fn restore_state(&mut self, state: &I8042State) -> Result<(), I8042Error> {
        if state.queue.len() > QUEUE_CAPACITY
            || state.pending_input.len() > QUEUE_CAPACITY
            || !(1..=2).contains(&state.keyboard.scan_set)
            || state.keyboard.leds > 7
            || state.keyboard.expected_leds > 7
            || state.keyboard.pending_leds & !6 != 0
            || state.keyboard.locks_pressed & !6 != 0
            || state.keyboard.typematic > 0x7f
            || state.control & (AUX_DISABLED | AUX_IRQ) != AUX_DISABLED
        {
            return Err(I8042Error::InvalidState);
        }
        self.state = state.clone();
        self.flush_input();
        self.signal_front()
    }

    fn signal_front(&self) -> Result<(), I8042Error> {
        if !self.state.queue.is_empty() && self.state.control & KBD_IRQ != 0 {
            self.kbd_interrupt_evt
                .write(1)
                .map_err(I8042Error::Interrupt)?;
        }
        Ok(())
    }

    fn input_capacity(&self) -> usize {
        QUEUE_CAPACITY.saturating_sub(self.state.queue.len() + self.state.pending_input.len())
    }

    fn flush_input(&mut self) {
        if self.state.control & KBD_DISABLED == 0 {
            let count =
                (QUEUE_CAPACITY - self.state.queue.len()).min(self.state.pending_input.len());
            self.state
                .queue
                .extend(self.state.pending_input.drain(..count));
        }
    }

    fn enqueue_input(&mut self, bytes: &[u8]) -> Result<(), I8042Error> {
        if bytes.len() > self.input_capacity() {
            return Err(I8042Error::InternalBufferFull);
        }
        let empty = self.state.queue.is_empty();
        self.state.pending_input.extend(bytes.iter().copied());
        self.flush_input();
        if empty {
            self.signal_front()?;
        }
        Ok(())
    }

    fn enqueue(&mut self, bytes: &[u8]) -> Result<(), I8042Error> {
        if bytes.len() > QUEUE_CAPACITY - self.state.queue.len() {
            return Err(I8042Error::InternalBufferFull);
        }
        let empty = self.state.queue.is_empty();
        self.state.queue.extend(bytes.iter().copied());
        if empty {
            self.signal_front()?;
        }
        Ok(())
    }

    fn reply(&mut self, bytes: &[u8]) -> Result<(), I8042Error> {
        self.enqueue(bytes)?;
        if let Some(&last) = bytes.last() {
            self.state.keyboard.last_byte = last;
        }
        Ok(())
    }

    /// Whether a normal or E0-prefixed set-2 make code names a supported key.
    pub fn valid_key(code: u8, extended: bool) -> bool {
        if extended {
            matches!(
                code,
                0x11 | 0x14
                    | 0x1f
                    | 0x27
                    | 0x2f
                    | 0x4a
                    | 0x5a
                    | 0x69
                    | 0x6b
                    | 0x6c
                    | 0x70
                    | 0x71
                    | 0x72
                    | 0x74
                    | 0x75
                    | 0x7a
                    | 0x7d
            )
        } else {
            set1_code(code).is_some() && !matches!(code, 0x1f | 0x27 | 0x2f)
        }
    }

    /// Inject a physical key using its set-2 make code. Repeated presses are allowed.
    /// Extended keys use the E0 prefix; Pause and Print Screen have dedicated methods.
    pub fn inject_key(
        &mut self,
        code: u8,
        extended: bool,
        pressed: bool,
    ) -> Result<(), I8042Error> {
        if !Self::valid_key(code, extended) {
            return Err(I8042Error::InvalidInput);
        }
        let translated = set1_code(code).ok_or(I8042Error::InvalidInput)?;
        if !self.state.keyboard.enabled {
            return Ok(());
        }
        let set1 = self.state.keyboard.scan_set == 1 || self.state.control & TRANSLATE != 0;
        let mut bytes = Vec::with_capacity(3);
        if extended {
            bytes.push(0xe0);
        }
        if set1 {
            bytes.push(translated | if pressed { 0 } else { 0x80 });
        } else {
            if !pressed {
                bytes.push(0xf0);
            }
            bytes.push(code);
        }
        self.enqueue_input(&bytes)?;
        let lock = match (code, extended) {
            (0x58, false) => 4,
            (0x77, false) => 2,
            _ => 0,
        };
        let keyboard = &mut self.state.keyboard;
        if pressed && lock != 0 {
            if keyboard.locks_pressed & lock == 0 {
                keyboard.expected_leds = (keyboard.expected_leds & keyboard.pending_leds)
                    | (keyboard.leds & !keyboard.pending_leds);
                keyboard.expected_leds ^= lock;
                keyboard.pending_leds |= lock;
                keyboard.locks_pressed |= lock;
            }
        } else {
            keyboard.locks_pressed &= !lock;
        }
        Ok(())
    }

    /// Inject the Pause make sequence (this key has no break sequence).
    pub fn inject_pause(&mut self) -> Result<(), I8042Error> {
        if !self.state.keyboard.enabled {
            return Ok(());
        }
        let bytes: &[u8] =
            if self.state.keyboard.scan_set == 1 || self.state.control & TRANSLATE != 0 {
                &[0xe1, 0x1d, 0x45, 0xe1, 0x9d, 0xc5]
            } else {
                &[0xe1, 0x14, 0x77, 0xe1, 0xf0, 0x14, 0xf0, 0x77]
            };
        self.enqueue_input(bytes)
    }

    /// Inject Print Screen with its synthetic shift bytes.
    pub fn inject_print_screen(&mut self, pressed: bool) -> Result<(), I8042Error> {
        if !self.state.keyboard.enabled {
            return Ok(());
        }
        let set1 = self.state.keyboard.scan_set == 1 || self.state.control & TRANSLATE != 0;
        self.enqueue_input(match (set1, pressed) {
            (true, true) => &[0xe0, 0x2a, 0xe0, 0x37],
            (true, false) => &[0xe0, 0xb7, 0xe0, 0xaa],
            (false, true) => &[0xe0, 0x12, 0xe0, 0x7c],
            (false, false) => &[0xe0, 0xf0, 0x7c, 0xe0, 0xf0, 0x12],
        })
    }

    /// Inject and release Ctrl-Alt-Delete without leaving modifiers pressed.
    pub fn trigger_ctrl_alt_del(&mut self) -> Result<(), I8042Error> {
        if self.input_capacity() < 11 {
            return Err(I8042Error::InternalBufferFull);
        }
        for (code, extended, pressed) in [
            (0x14, false, true),
            (0x11, false, true),
            (0x71, true, true),
            (0x71, true, false),
            (0x11, false, false),
            (0x14, false, false),
        ] {
            self.inject_key(code, extended, pressed)?;
        }
        Ok(())
    }

    fn reset(&self) -> Result<(), I8042Error> {
        crate::logger::info!("Guest requested i8042 reset");
        self.reset_evt.write(1).map_err(I8042Error::Interrupt)?;
        METRICS.reset_count.inc();
        Ok(())
    }

    fn command(&mut self, command: u8) -> Result<(), I8042Error> {
        self.state.destination = None;
        match command {
            0x20 => self.enqueue(&[self.state.control])?,
            0x60 => self.state.destination = Some(Destination::Control),
            0xa7 | 0xa8 => (),
            0xa9 => self.enqueue(&[1])?,
            0xab => self.enqueue(&[0])?,
            0xaa => {
                self.state.control |= 4;
                self.enqueue(&[0x55])?;
            }
            0xad => self.state.control |= KBD_DISABLED,
            0xae => self.state.control &= !KBD_DISABLED,
            0xc0 => self.enqueue(&[0x80])?,
            0xd0 => self.enqueue(&[self.state.output])?,
            0xd1 => self.state.destination = Some(Destination::Output),
            0xd2 => self.state.destination = Some(Destination::KeyboardOutput),
            0xd3 | 0xd4 => self.state.destination = Some(Destination::DiscardAuxiliary),
            0xdd => self.state.output &= !2,
            0xdf => self.state.output |= 2,
            0xf0..=0xff if command & 1 == 0 => self.reset()?,
            _ => (),
        }
        Ok(())
    }

    fn data(&mut self, value: u8) -> Result<(), I8042Error> {
        match self.state.destination.take() {
            Some(Destination::Control) => {
                self.state.control = (value | AUX_DISABLED) & !AUX_IRQ;
                self.signal_front()
            }
            Some(Destination::Output) => {
                self.state.output = value;
                if value & 1 == 0 {
                    self.reset()?;
                }
                Ok(())
            }
            Some(Destination::KeyboardOutput) => self.enqueue(&[value]),
            Some(Destination::DiscardAuxiliary) => Ok(()),
            None => {
                self.state.control &= !KBD_DISABLED;
                self.keyboard_command(value)
            }
        }
    }

    fn keyboard_command(&mut self, value: u8) -> Result<(), I8042Error> {
        if let Some(parameter) = self.state.keyboard.parameter.take() {
            match parameter {
                KeyboardParameter::Leds if value <= 7 => {
                    let keyboard = &mut self.state.keyboard;
                    keyboard.leds = value;
                    keyboard.pending_leds &= value ^ keyboard.expected_leds;
                }
                KeyboardParameter::Typematic if value <= 0x7f => {
                    self.state.keyboard.typematic = value
                }
                KeyboardParameter::ScanSet if value == 0 => {
                    let set = self.state.keyboard.scan_set;
                    let result = if self.state.control & TRANSLATE != 0 {
                        if set == 1 { 0x43 } else { 0x41 }
                    } else {
                        set
                    };
                    return self.reply(&[ACK, result]);
                }
                KeyboardParameter::ScanSet if (1..=2).contains(&value) => {
                    self.state.keyboard.scan_set = value
                }
                _ => {
                    self.state.keyboard.parameter = Some(parameter);
                    return self.reply(&[RESEND]);
                }
            }
            return self.reply(&[ACK]);
        }
        match value {
            0xed => self.state.keyboard.parameter = Some(KeyboardParameter::Leds),
            0xee => return self.reply(&[0xee]),
            0xf0 => self.state.keyboard.parameter = Some(KeyboardParameter::ScanSet),
            0xf2 => {
                return self.reply(&[
                    ACK,
                    0xab,
                    if self.state.control & TRANSLATE != 0 {
                        0x41
                    } else {
                        0x83
                    },
                ]);
            }
            0xf3 => self.state.keyboard.parameter = Some(KeyboardParameter::Typematic),
            0xf4 => self.state.keyboard.enabled = true,
            0xf5 | 0xf6 => {
                self.state.pending_input.clear();
                self.state.keyboard = Keyboard::default();
                self.state.keyboard.enabled = value == 0xf6;
            }
            0xfe => return self.enqueue(&[self.state.keyboard.last_byte]),
            0xff => {
                self.state.pending_input.clear();
                self.state.keyboard = Keyboard::default();
                return self.reply(&[ACK, 0xaa]);
            }
            _ => return self.reply(&[RESEND]),
        }
        self.reply(&[ACK])
    }
}

// IBM enhanced keyboard set-2 make codes paired with their set-1 equivalents.
fn set1_code(code: u8) -> Option<u8> {
    Some(match code {
        0x76 => 0x01,
        0x16 => 0x02,
        0x1e => 0x03,
        0x26 => 0x04,
        0x25 => 0x05,
        0x2e => 0x06,
        0x36 => 0x07,
        0x3d => 0x08,
        0x3e => 0x09,
        0x46 => 0x0a,
        0x45 => 0x0b,
        0x4e => 0x0c,
        0x55 => 0x0d,
        0x66 => 0x0e,
        0x0d => 0x0f,
        0x15 => 0x10,
        0x1d => 0x11,
        0x24 => 0x12,
        0x2d => 0x13,
        0x2c => 0x14,
        0x35 => 0x15,
        0x3c => 0x16,
        0x43 => 0x17,
        0x44 => 0x18,
        0x4d => 0x19,
        0x54 => 0x1a,
        0x5b => 0x1b,
        0x5a => 0x1c,
        0x14 => 0x1d,
        0x1c => 0x1e,
        0x1b => 0x1f,
        0x23 => 0x20,
        0x2b => 0x21,
        0x34 => 0x22,
        0x33 => 0x23,
        0x3b => 0x24,
        0x42 => 0x25,
        0x4b => 0x26,
        0x4c => 0x27,
        0x52 => 0x28,
        0x0e => 0x29,
        0x12 => 0x2a,
        0x5d => 0x2b,
        0x1a => 0x2c,
        0x22 => 0x2d,
        0x21 => 0x2e,
        0x2a => 0x2f,
        0x32 => 0x30,
        0x31 => 0x31,
        0x3a => 0x32,
        0x41 => 0x33,
        0x49 => 0x34,
        0x4a => 0x35,
        0x59 => 0x36,
        0x7c => 0x37,
        0x11 => 0x38,
        0x29 => 0x39,
        0x58 => 0x3a,
        0x05 => 0x3b,
        0x06 => 0x3c,
        0x04 => 0x3d,
        0x0c => 0x3e,
        0x03 => 0x3f,
        0x0b => 0x40,
        0x83 => 0x41,
        0x0a => 0x42,
        0x01 => 0x43,
        0x09 => 0x44,
        0x77 => 0x45,
        0x7e => 0x46,
        0x6c => 0x47,
        0x75 => 0x48,
        0x7d => 0x49,
        0x7b => 0x4a,
        0x6b => 0x4b,
        0x73 => 0x4c,
        0x74 => 0x4d,
        0x79 => 0x4e,
        0x69 => 0x4f,
        0x72 => 0x50,
        0x7a => 0x51,
        0x70 => 0x52,
        0x71 => 0x53,
        0x61 => 0x56,
        0x78 => 0x57,
        0x07 => 0x58,
        0x1f => 0x5b,
        0x27 => 0x5c,
        0x2f => 0x5d,
        _ => return None,
    })
}

impl BusDevice for I8042Device {
    fn read(&mut self, _base: u64, offset: u64, data: &mut [u8]) {
        if data.len() != 1 || ![0, 4].contains(&offset) {
            METRICS.missed_read_count.inc();
            return;
        }
        if offset == 4 {
            data[0] = 0x10 | self.state.control & 4 | if self.state.command_write { 8 } else { 0 };
            if !self.state.queue.is_empty() {
                data[0] |= 1;
            }
        } else {
            data[0] = self.state.queue.pop_front().unwrap_or(0);
            self.flush_input();
            if let Err(err) = self.signal_front() {
                error!("i8042 interrupt: {err}");
                METRICS.error_count.inc();
            }
        }
        METRICS.read_count.inc();
    }

    fn write(&mut self, _base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        if data.len() != 1 || ![0, 4].contains(&offset) {
            METRICS.missed_write_count.inc();
            return None;
        }
        self.state.command_write = offset == 4;
        let result = if offset == 4 {
            self.command(data[0])
        } else {
            self.data(data[0])
        };
        let result = result.and_then(|()| {
            let empty = self.state.queue.is_empty();
            self.flush_input();
            if empty { self.signal_front() } else { Ok(()) }
        });
        if let Err(err) = result {
            error!("i8042 write: {err}");
            METRICS.error_count.inc();
        }
        METRICS.write_count.inc();
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller() -> I8042Device {
        I8042Device::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap()
    }

    fn read(device: &mut I8042Device, offset: u64) -> u8 {
        let mut data = [0];
        device.read(0, offset, &mut data);
        data[0]
    }

    fn command(device: &mut I8042Device, byte: u8) {
        device.write(0, 4, &[byte]);
    }
    fn data(device: &mut I8042Device, byte: u8) {
        device.write(0, 0, &[byte]);
    }
    fn drain(device: &mut I8042Device) -> Vec<u8> {
        let mut bytes = Vec::new();
        while read(device, 4) & 1 != 0 {
            bytes.push(read(device, 0));
        }
        bytes
    }

    #[test]
    fn absent_auxiliary_port_cannot_be_enabled_or_inject_keyboard_replies() {
        let mut device = controller();
        command(&mut device, 0xa8);
        command(&mut device, 0x20);
        assert_eq!(read(&mut device, 0) & 0x22, 0x20);
        command(&mut device, 0x60);
        data(&mut device, 0xff);
        command(&mut device, 0x20);
        assert_eq!(read(&mut device, 0) & 0x22, 0x20);
        command(&mut device, 0xa9);
        assert_eq!(drain(&mut device), [1]);
        command(&mut device, 0xab);
        assert_eq!(drain(&mut device), [0]);
        for prefix in [0xd3, 0xd4] {
            command(&mut device, prefix);
            data(&mut device, 0xff);
            assert_eq!(read(&mut device, 4) & 0x21, 0);
        }
        data(&mut device, 0xf2);
        assert_eq!(drain(&mut device), [ACK, 0xab, 0x41]);
    }

    #[test]
    fn firmware_and_windows_initialization() {
        let mut dev = controller();
        command(&mut dev, 0xad);
        command(&mut dev, 0xa7);
        command(&mut dev, 0xaa);
        assert_eq!(drain(&mut dev), [0x55]);
        command(&mut dev, 0x60);
        data(&mut dev, 0x67);
        data(&mut dev, 0xed);
        data(&mut dev, 0);
        assert_eq!(drain(&mut dev), [ACK, ACK]);
        command(&mut dev, 0xae);
        command(&mut dev, 0xa8);
        data(&mut dev, 0xff);
        assert_eq!(drain(&mut dev), [ACK, 0xaa]);
    }

    #[test]
    fn lock_led_acknowledgement_survives_snapshot_and_repeated_make() {
        let mut device = controller();
        device.inject_key(0x58, false, true).unwrap();
        device.inject_key(0x58, false, true).unwrap();
        device.inject_key(0x58, false, false).unwrap();
        assert_eq!(device.keyboard_leds(), 0);
        assert_eq!(device.keyboard_pending_leds(), 4);
        let mut restored = controller();
        restored.restore_state(&device.save_state()).unwrap();
        assert_eq!(restored.keyboard_pending_leds(), 4);
        drain(&mut restored);
        data(&mut restored, 0xed);
        data(&mut restored, 0);
        assert_eq!(restored.keyboard_pending_leds(), 4);
        data(&mut restored, 0xed);
        data(&mut restored, 4);
        assert_eq!(restored.keyboard_leds(), 4);
        assert_eq!(restored.keyboard_pending_leds(), 0);
        restored.inject_key(0x77, false, true).unwrap();
        assert_eq!(restored.keyboard_pending_leds(), 2);
        data(&mut restored, 0xed);
        data(&mut restored, 6);
        assert_eq!(restored.keyboard_pending_leds(), 0);
    }

    #[test]
    fn ignored_or_rejected_lock_make_does_not_require_led_acknowledgement() {
        let mut device = controller();
        data(&mut device, 0xf5);
        device.inject_key(0x58, false, true).unwrap();
        assert_eq!(device.keyboard_pending_leds(), 0);
        data(&mut device, 0xf4);
        drain(&mut device);
        for _ in 0..QUEUE_CAPACITY {
            device.inject_key(0x1c, false, true).unwrap();
        }
        device.inject_key(0x58, false, true).unwrap_err();
        assert_eq!(device.keyboard_pending_leds(), 0);
    }

    #[test]
    fn keyboard_translation_and_special_keys() {
        let mut dev = controller();
        dev.inject_key(0x1c, false, true).unwrap();
        dev.inject_key(0x1c, false, false).unwrap();
        dev.inject_key(0x75, true, false).unwrap();
        assert_eq!(drain(&mut dev), [0x1c, 0xf0, 0x1c, 0xe0, 0xf0, 0x75]);
        command(&mut dev, 0x60);
        data(&mut dev, 0x45);
        dev.inject_key(0x1c, false, true).unwrap();
        dev.inject_key(0x1c, false, false).unwrap();
        dev.inject_key(0x75, true, false).unwrap();
        assert_eq!(drain(&mut dev), [0x1e, 0x9e, 0xe0, 0xc8]);
        dev.inject_pause().unwrap();
        dev.inject_print_screen(false).unwrap();
        assert_eq!(
            drain(&mut dev),
            [0xe1, 0x1d, 0x45, 0xe1, 0x9d, 0xc5, 0xe0, 0xb7, 0xe0, 0xaa]
        );
        data(&mut dev, 0xf0);
        data(&mut dev, 0);
        assert_eq!(drain(&mut dev), [ACK, ACK, 0x41]);
    }

    #[test]
    fn inhibited_input_is_saved_and_controller_replies_bypass_it() {
        let mut dev = controller();
        command(&mut dev, 0x60);
        data(&mut dev, KBD_IRQ | AUX_IRQ);
        command(&mut dev, 0xad);
        command(&mut dev, 0xa7);
        dev.inject_key(0x1c, false, true).unwrap();
        dev.inject_key(0x1c, false, false).unwrap();
        assert_eq!(read(&mut dev, 4) & 1, 0);
        dev.kbd_interrupt_evt.read().unwrap_err();
        command(&mut dev, 0x20);
        assert_eq!(dev.kbd_interrupt_evt.read().unwrap(), 1);
        assert_eq!(read(&mut dev, 0), 0x31);
        let mut restored = controller();
        restored.restore_state(&dev.save_state()).unwrap();
        assert_eq!(read(&mut restored, 4) & 1, 0);
        command(&mut restored, 0xae);
        for byte in [0x1c, 0xf0, 0x1c] {
            assert_eq!(restored.kbd_interrupt_evt.read().unwrap(), 1);
            assert_eq!(read(&mut restored, 4) & 0x21, 1);
            assert_eq!(read(&mut restored, 0), byte);
        }
        assert_eq!(read(&mut restored, 4) & 1, 0);
    }

    #[test]
    fn inhibited_queue_is_bounded_and_reset_discards_device_input() {
        let mut dev = controller();
        command(&mut dev, 0xad);
        for _ in 0..QUEUE_CAPACITY {
            dev.inject_key(0x1c, false, true).unwrap();
        }
        dev.inject_key(0x1c, false, true).unwrap_err();
        assert_eq!(dev.state.pending_input.len(), QUEUE_CAPACITY);
        command(&mut dev, 0x20);
        assert_eq!(drain(&mut dev), [0x35]);
        let mut state = dev.save_state();
        state.pending_input.push_back(0);
        dev.restore_state(&state).unwrap_err();
        data(&mut dev, 0xff);
        assert_eq!(drain(&mut dev), [ACK, 0xaa]);
        assert!(dev.state.pending_input.is_empty());
    }

    #[test]
    fn device_commands_reenable_inhibited_keyboard() {
        let mut dev = controller();
        command(&mut dev, 0x60);
        data(&mut dev, KBD_IRQ | AUX_IRQ | TRANSLATE);
        command(&mut dev, 0xad);
        command(&mut dev, 0xa7);
        assert_eq!(dev.save_state().control, 0x71);
        dev.inject_key(0x5a, false, true).unwrap();
        assert!(drain(&mut dev).is_empty());

        data(&mut dev, 0xf4);
        assert_eq!(drain(&mut dev), [ACK, 0x1c]);
        assert_eq!(dev.save_state().control, 0x61);
        dev.kbd_interrupt_evt.read().unwrap();
        dev.inject_key(0x5a, false, true).unwrap();
        dev.inject_key(0x5a, false, false).unwrap();
        assert_eq!(dev.kbd_interrupt_evt.read().unwrap(), 1);
        assert_eq!(read(&mut dev, 0), 0x1c);
        assert_eq!(dev.kbd_interrupt_evt.read().unwrap(), 1);
        assert_eq!(read(&mut dev, 0), 0x9c);
    }

    #[test]
    fn keyboard_irq_follows_each_queued_byte() {
        let mut dev = controller();
        dev.inject_key(0x1c, false, true).unwrap();
        data(&mut dev, 0xf2);
        for value in [0x1c, ACK, 0xab, 0x83] {
            assert_eq!(dev.kbd_interrupt_evt.read().unwrap(), 1);
            assert_eq!(read(&mut dev, 4) & 0x21, 1);
            assert_eq!(read(&mut dev, 0), value);
        }
        assert_eq!(read(&mut dev, 4) & 0x21, 0);
    }

    #[test]
    fn commands_do_not_discard_pending_keys() {
        let mut dev = controller();
        dev.inject_key(0x1c, false, true).unwrap();
        command(&mut dev, 0x20);
        data(&mut dev, 0xf2);
        assert_eq!(drain(&mut dev), [0x1c, 0x25, ACK, 0xab, 0x83]);
        data(&mut dev, 0xf5);
        dev.inject_key(0x1c, false, true).unwrap();
        assert_eq!(drain(&mut dev), [ACK]);
        data(&mut dev, 0xf4);
        dev.inject_key(0x1c, false, true).unwrap();
        assert_eq!(drain(&mut dev), [ACK, 0x1c]);
    }

    #[test]
    fn snapshot_mid_command_and_discarded_auxiliary_write() {
        let mut original = controller();
        data(&mut original, 0xf0);
        command(&mut original, 0xd4);
        let state = original.save_state();
        let mut restored = controller();
        restored.restore_state(&state).unwrap();
        data(&mut restored, 200);
        data(&mut restored, 1);
        assert_eq!(drain(&mut restored), [ACK, ACK]);
        restored.inject_key(0x1c, false, true).unwrap();
        assert_eq!(drain(&mut restored), [0x1e]);
        let mut invalid = state;
        invalid.keyboard.scan_set = 3;
        assert!(matches!(
            restored.restore_state(&invalid),
            Err(I8042Error::InvalidState)
        ));
    }

    #[test]
    fn bounded_queue_rejects_whole_key_sequence() {
        let mut dev = controller();
        dev.enqueue(&[0; QUEUE_CAPACITY - 1]).unwrap();
        assert!(matches!(
            dev.inject_key(0x75, true, false),
            Err(I8042Error::InternalBufferFull)
        ));
        assert_eq!(dev.state.queue.len(), QUEUE_CAPACITY - 1);
    }

    #[test]
    fn reset_paths_and_access_widths() {
        let mut dev = controller();
        let mut bytes = [1, 2];
        dev.read(0, 0, &mut bytes);
        assert_eq!(bytes, [1, 2]);
        dev.write(0, 4, &[0xfe, 0]);
        dev.reset_evt.read().unwrap_err();
        command(&mut dev, 0xfe);
        assert_eq!(dev.reset_evt.read().unwrap(), 1);
        command(&mut dev, 0xd1);
        data(&mut dev, 2);
        assert_eq!(dev.reset_evt.read().unwrap(), 1);
    }
}
