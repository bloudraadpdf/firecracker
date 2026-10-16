// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::collections::{BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

#[cfg(feature = "windows")]
use super::Input;
use super::InputError;
#[cfg(feature = "windows")]
use super::device::FEATURES;
use super::protocol::*;
#[cfg(feature = "windows")]
use crate::devices::virtio::device::{VirtioDevice, VirtioDeviceType};
use crate::devices::virtio::persist::PersistError;
use crate::devices::virtio::persist::VirtioDeviceState;
#[cfg(feature = "windows")]
use crate::devices::virtio::queue::FIRECRACKER_MAX_QUEUE_SIZE;
#[cfg(feature = "windows")]
use crate::snapshot::Persist;
#[cfg(feature = "windows")]
use crate::vstate::memory::GuestMemoryMmap;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct Frame {
    pub events: Vec<InputEvent>,
    pub next: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InputState {
    pub virtio_state: VirtioDeviceState,
    pub(super) select: u8,
    pub(super) subsel: u8,
    pub(super) pending: VecDeque<Frame>,
    pub(super) held: BTreeSet<u16>,
    pub(super) delivered: BTreeSet<u16>,
    pub(super) position: [u16; 2],
    pub(super) leds: u8,
    pub(super) expected_leds: u8,
    pub(super) pending_leds: u8,
}

#[derive(Debug)]
#[cfg(feature = "windows")]
pub struct InputConstructorArgs {
    pub mem: GuestMemoryMmap,
}

#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum InputPersistError {
    /// Create input device: {0}
    Create(#[from] InputError),
    /// Virtio input state: {0}
    VirtioState(#[from] PersistError),
    /// Snapshot contains invalid input state.
    InvalidState,
    /// Input event registration failed: {0}
    Registration(std::io::Error),
}

#[cfg(feature = "windows")]
impl InputState {
    #[cfg(all(test, feature = "windows"))]
    pub(crate) fn maximum_test_state() -> Self {
        let mut state = Input::new().unwrap().save();
        let mut events = vec![InputEvent::new(EV_ABS, 0, 0), InputEvent::new(EV_ABS, 1, 0)];
        events.extend(
            (1..=194)
                .filter(|&key| keyboard_key(key))
                .chain(BTN_LEFT..BTN_LEFT + 3)
                .map(|key| InputEvent::new(EV_KEY, key, 0)),
        );
        events.push(InputEvent::SYN);
        state.pending =
            std::iter::repeat_n(Frame { events, next: 0 }, MAX_PENDING_FRAMES).collect();
        state
    }

    fn validate(&self) -> Result<(), InputPersistError> {
        if self.virtio_state.avail_features != FEATURES
            || self.virtio_state.acked_features & !FEATURES != 0
            || (self.virtio_state.activated && self.virtio_state.acked_features != FEATURES)
            || self.pending.len() > MAX_PENDING_FRAMES
            || self.position.iter().any(|&coordinate| coordinate > ABS_MAX)
            || (self.leds | self.expected_leds | self.pending_leds) & !7 != 0
            || self
                .held
                .iter()
                .chain(&self.delivered)
                .any(|&key| !supported_key(key))
            || (!self.virtio_state.activated && !self.delivered.is_empty())
        {
            return Err(InputPersistError::InvalidState);
        }
        let mut held = self.delivered.clone();
        for (index, frame) in self.pending.iter().enumerate() {
            if frame.events.is_empty()
                || frame.events.len() > MAX_FRAME_EVENTS
                || frame.next >= frame.events.len()
                || (index != 0 && frame.next != 0)
                || frame.events.last() != Some(&InputEvent::SYN)
                || frame.events[..frame.events.len() - 1]
                    .iter()
                    .any(|event| !event.is_supported() || event.event_type == EV_SYN)
            {
                return Err(InputPersistError::InvalidState);
            }
            for event in &frame.events[frame.next..] {
                if event.event_type == EV_KEY {
                    Input::update_held(&mut held, event.code, event.value);
                }
            }
        }
        if held != self.held {
            return Err(InputPersistError::InvalidState);
        }
        Ok(())
    }
}

#[cfg(feature = "windows")]
impl Persist<'_> for Input {
    type State = InputState;
    type ConstructorArgs = InputConstructorArgs;
    type Error = InputPersistError;

    fn save(&self) -> Self::State {
        InputState {
            virtio_state: VirtioDeviceState::from_device(self),
            select: self.config[0],
            subsel: self.config[1],
            pending: self.pending.clone(),
            held: self.held.clone(),
            delivered: self.delivered.clone(),
            position: self.position,
            leds: self.leds,
            expected_leds: self.expected_leds,
            pending_leds: self.pending_leds,
        }
    }

    fn restore(args: Self::ConstructorArgs, state: &Self::State) -> Result<Self, Self::Error> {
        state.validate()?;
        let queues = state.virtio_state.build_queues_checked(
            &args.mem,
            VirtioDeviceType::Input,
            NUM_QUEUES,
            FIRECRACKER_MAX_QUEUE_SIZE,
        )?;
        if state.virtio_state.activated && !queues[EVENT_QUEUE].ready {
            return Err(InputPersistError::InvalidState);
        }
        let mut device = Self::new()?;
        device.queues = queues;
        device.set_acked_features(state.virtio_state.acked_features);
        device.config = config(state.select, state.subsel);
        device.pending = state.pending.clone();
        device.held = state.held.clone();
        device.delivered = state.delivered.clone();
        device.position = state.position;
        device.leds = state.leds;
        device.expected_leds = state.expected_leds;
        device.pending_leds = state.pending_leds;
        Ok(device)
    }
}
