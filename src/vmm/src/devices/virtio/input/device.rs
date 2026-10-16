// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::collections::{BTreeSet, VecDeque};
use std::io;
use std::sync::Arc;
use std::time::Duration;

pub(super) use super::persist::Frame;
use utils::time::TimerFd;
use vm_memory::{Bytes, GuestMemoryBackend, GuestMemoryError};
use vmm_sys_util::eventfd::EventFd;

use super::protocol::*;
use super::{INPUT_DEV_ID, InputError};
use crate::devices::virtio::ActivateError;
use crate::devices::virtio::device::{ActiveState, DeviceState, VirtioDevice, VirtioDeviceType};
use crate::devices::virtio::generated::virtio_config::VIRTIO_F_VERSION_1;
use crate::devices::virtio::queue::{
    DescriptorChain, FIRECRACKER_MAX_QUEUE_SIZE, Queue, VIRTQ_DESC_F_WRITE,
};
use crate::devices::virtio::transport::{VirtioInterrupt, VirtioInterruptType};
use crate::impl_device_type;
use crate::logger::warn;
use crate::vstate::memory::{GuestAddress, GuestMemoryMmap};

pub(super) const FEATURES: u64 = 1 << VIRTIO_F_VERSION_1;
// Drivers may replenish event buffers without a queue notification.
const RETRY_DELAY: Duration = Duration::from_millis(10);

struct EventBuffer(Vec<(GuestAddress, usize)>);

impl EventBuffer {
    fn new(
        mem: &GuestMemoryMmap,
        chain: DescriptorChain,
        writable: bool,
    ) -> Result<Self, InputError> {
        let mut segments = Vec::new();
        let mut remaining = EVENT_SIZE;
        for desc in chain.checked_iter(mem) {
            let desc = desc?;
            if (desc.flags & VIRTQ_DESC_F_WRITE != 0) != writable {
                return Err(InputError::InvalidBuffer);
            }
            let address = GuestAddress(desc.addr);
            mem.get_slice(address, desc.len as usize)?;
            let length = remaining.min(desc.len as usize);
            if length != 0 {
                segments.push((address, length));
                remaining -= length;
            }
        }
        if remaining != 0 {
            return Err(InputError::InvalidBuffer);
        }
        Ok(Self(segments))
    }

    fn write(&self, mem: &GuestMemoryMmap, event: InputEvent) -> Result<(), GuestMemoryError> {
        let bytes = event.bytes();
        let mut offset = 0;
        for &(address, length) in &self.0 {
            mem.write_slice(&bytes[offset..offset + length], address)?;
            offset += length;
        }
        Ok(())
    }

    fn read(&self, mem: &GuestMemoryMmap) -> Result<InputEvent, GuestMemoryError> {
        let mut bytes = [0; EVENT_SIZE];
        let mut offset = 0;
        for &(address, length) in &self.0 {
            mem.read_slice(&mut bytes[offset..offset + length], address)?;
            offset += length;
        }
        Ok(InputEvent::from_bytes(bytes))
    }
}

#[derive(Debug)]
pub struct Input {
    pub(super) acked_features: u64,
    pub(super) device_state: DeviceState,
    pub(super) queues: Vec<Queue>,
    pub(super) queue_events: Vec<EventFd>,
    pub(super) retry_timer: TimerFd,
    pub(super) retry_armed: bool,
    pub(super) config: [u8; CONFIG_SIZE],
    pub(super) pending: VecDeque<Frame>,
    pub(super) held: BTreeSet<u16>,
    pub(super) delivered: BTreeSet<u16>,
    pub(super) position: [u16; 2],
    pub(super) leds: u8,
    pub(super) expected_leds: u8,
    pub(super) pending_leds: u8,
    pub(super) registration_error: Option<String>,
}

impl Input {
    pub fn new() -> Result<Self, InputError> {
        Ok(Self {
            acked_features: 0,
            device_state: DeviceState::Inactive,
            queues: vec![Queue::new(FIRECRACKER_MAX_QUEUE_SIZE); NUM_QUEUES],
            queue_events: (0..NUM_QUEUES)
                .map(|_| EventFd::new(libc::EFD_NONBLOCK))
                .collect::<Result<_, _>>()?,
            retry_timer: TimerFd::new(),
            retry_armed: false,
            config: config(0, 0),
            pending: VecDeque::new(),
            held: BTreeSet::new(),
            delivered: BTreeSet::new(),
            position: [0; 2],
            leds: 0,
            expected_leds: 0,
            pending_leds: 0,
            registration_error: Some("input subscriber is not registered".to_owned()),
        })
    }

    fn enqueue(&mut self, mut events: Vec<InputEvent>) -> Result<(), InputError> {
        self.flush_events()?;
        if self.pending.len() == MAX_PENDING_FRAMES {
            return Err(InputError::PendingFull);
        }
        events.push(InputEvent::SYN);
        self.pending.push_back(Frame { events, next: 0 });
        Ok(())
    }

    /// Submit an evdev keyboard transition, including left/right Meta (125/126).
    pub fn key(&mut self, code: u16, down: bool) -> Result<(), InputError> {
        if !keyboard_key(code) {
            return Err(InputError::InvalidHostEvent);
        }
        if !down && !self.held.contains(&code) {
            return Ok(());
        }
        let value = if down {
            if self.held.contains(&code) { 2 } else { 1 }
        } else {
            0
        };
        self.enqueue(vec![InputEvent::new(EV_KEY, code, value)])?;
        Self::update_held(&mut self.held, code, value);
        self.flush_events()
    }

    /// Submit a make-only key as separate press/release reports, admitting both or neither.
    pub fn pulse(&mut self, code: u16) -> Result<(), InputError> {
        if !keyboard_key(code) || self.held.contains(&code) {
            return Err(InputError::InvalidHostEvent);
        }
        if self.pending.len() > MAX_PENDING_FRAMES - 2 {
            return Err(InputError::PendingFull);
        }
        for value in [1, 0] {
            self.pending.push_back(Frame {
                events: vec![InputEvent::new(EV_KEY, code, value), InputEvent::SYN],
                next: 0,
            });
        }
        self.flush_events()
    }

    fn press_lock(&mut self, code: u16) {
        let mask = lock_mask(code);
        if mask == 0 {
            return;
        }
        self.expected_leds =
            (self.expected_leds & self.pending_leds) | (self.leds & !self.pending_leds);
        self.expected_leds ^= mask;
        self.pending_leds |= mask;
    }

    /// Num Lock, Caps Lock and Scroll Lock occupy bits 0, 1 and 2.
    pub fn led_state(&self) -> u8 {
        self.leds
    }

    /// Lock transitions awaiting matching guest status, in the same order as led_state.
    pub fn pending_led_state(&self) -> u8 {
        self.pending
            .iter()
            .flat_map(|frame| &frame.events[frame.next..])
            .filter(|event| event.event_type == EV_KEY && event.value == 1)
            .fold(self.pending_leds, |pending, event| {
                pending | lock_mask(event.code)
            })
    }

    /// Complete updates awaiting guest buffers, including a partly written front update.
    pub fn pending_frames(&self) -> usize {
        self.pending.len()
    }

    /// Coordinates are 0..32767; button bits are left, right, middle; wheel is -127..127.
    pub fn pointer(&mut self, x: u16, y: u16, buttons: u8, wheel: i8) -> Result<(), InputError> {
        if x > ABS_MAX || y > ABS_MAX || buttons & !7 != 0 || wheel == i8::MIN {
            return Err(InputError::InvalidHostEvent);
        }
        let mut events = vec![
            InputEvent::new(EV_ABS, 0, i32::from(x)),
            InputEvent::new(EV_ABS, 1, i32::from(y)),
        ];
        for bit in 0..3 {
            let code = BTN_LEFT + bit;
            let down = buttons & (1 << bit) != 0;
            if down != self.held.contains(&code) {
                events.push(InputEvent::new(EV_KEY, code, i32::from(down)));
            }
        }
        if wheel != 0 {
            events.push(InputEvent::new(EV_REL, 8, i32::from(wheel)));
        }
        self.enqueue(events)?;
        for bit in 0..3 {
            Self::update_held(
                &mut self.held,
                BTN_LEFT + bit,
                i32::from(buttons & (1 << bit) != 0),
            );
        }
        self.position = [x, y];
        self.flush_events()
    }

    /// Finish a started report, then release accepted or written keys on disconnect.
    pub fn release_all(&mut self) -> Result<(), InputError> {
        let partial = self
            .pending
            .front()
            .filter(|frame| frame.next != 0)
            .cloned();
        let mut projected = self.delivered.clone();
        if let Some(frame) = &partial {
            for event in &frame.events[frame.next..] {
                if event.event_type == EV_KEY {
                    Self::update_held(&mut projected, event.code, event.value);
                }
            }
        }
        let mut events = vec![
            InputEvent::new(EV_ABS, 0, i32::from(self.position[0])),
            InputEvent::new(EV_ABS, 1, i32::from(self.position[1])),
        ];
        events.extend(
            self.held
                .union(&projected)
                .map(|&code| InputEvent::new(EV_KEY, code, 0)),
        );
        events.push(InputEvent::SYN);
        self.pending.clear();
        self.pending.extend(partial);
        self.pending.push_back(Frame { events, next: 0 });
        self.held.clear();
        self.flush_events()
    }

    pub(super) fn update_held(held: &mut BTreeSet<u16>, code: u16, value: i32) {
        if value == 0 {
            held.remove(&code);
        } else {
            held.insert(code);
        }
    }

    pub fn check_registered(&self) -> io::Result<()> {
        match &self.registration_error {
            Some(error) => Err(io::Error::other(error.clone())),
            None => Ok(()),
        }
    }

    #[cfg(any(target_arch = "aarch64", test))]
    pub(crate) fn reset_registration(&mut self) {
        self.registration_error = Some("input subscriber is not registered".to_owned());
        self.stop_retry();
    }

    pub(super) fn stop_retry(&mut self) {
        if self.retry_armed {
            self.retry_timer.arm(Duration::ZERO, None);
            self.retry_timer.read();
            self.retry_armed = false;
        }
    }

    pub(super) fn update_retry_timer(&mut self) {
        if self.is_activated() && !self.pending.is_empty() {
            if !self.retry_armed {
                self.retry_timer.arm(RETRY_DELAY, None);
                self.retry_armed = true;
            }
        } else {
            self.stop_retry();
        }
    }

    fn publish_used(&mut self, index: usize, used: bool) -> Result<(), InputError> {
        let queue = &mut self.queues[index];
        queue.advance_used_ring_idx();
        if used && queue.prepare_kick() {
            self.interrupt_trigger()
                .trigger(VirtioInterruptType::Queue(index.try_into().unwrap()))?;
        }
        Ok(())
    }

    pub(super) fn flush_events(&mut self) -> Result<(), InputError> {
        let Some(active) = self.device_state.active_state() else {
            return Ok(());
        };
        let mem = active.mem.clone();
        let mut used = false;
        for _ in 0..self.queues[EVENT_QUEUE].size {
            let Some(frame) = self.pending.front_mut() else {
                break;
            };
            let Some(chain) = self.queues[EVENT_QUEUE].pop()? else {
                break;
            };
            let event = frame.events[frame.next];
            let length = match EventBuffer::new(&mem, chain, true) {
                Ok(buffer) => {
                    buffer.write(&mem, event)?;
                    if event.event_type == EV_KEY {
                        Self::update_held(&mut self.delivered, event.code, event.value);
                    }
                    frame.next += 1;
                    if frame.next == frame.events.len() {
                        self.pending.pop_front();
                    }
                    u32::try_from(EVENT_SIZE).unwrap()
                }
                Err(error) => {
                    warn!("input: rejected event buffer: {error}");
                    0
                }
            };
            if length != 0 && event.event_type == EV_KEY && event.value == 1 {
                self.press_lock(event.code);
            }
            self.queues[EVENT_QUEUE].add_used(chain.index, length)?;
            used = true;
        }
        let result = self.publish_used(EVENT_QUEUE, used);
        self.update_retry_timer();
        result
    }

    pub(super) fn process_status(&mut self) -> Result<(), InputError> {
        let Some(active) = self.device_state.active_state() else {
            return Ok(());
        };
        if !self.queues[STATUS_QUEUE].ready {
            return Ok(());
        }
        let mem = active.mem.clone();
        let mut used = false;
        for _ in 0..self.queues[STATUS_QUEUE].size {
            let Some(chain) = self.queues[STATUS_QUEUE].pop()? else {
                break;
            };
            match EventBuffer::new(&mem, chain, false).and_then(|buffer| Ok(buffer.read(&mem)?)) {
                Ok(event)
                    if event.event_type == EV_LED
                        && event.code < 3
                        && (0..=1).contains(&event.value) =>
                {
                    let mask = 1 << event.code;
                    if event.value == 0 {
                        self.leds &= !mask;
                    } else {
                        self.leds |= mask;
                    }
                    if (self.leds ^ self.expected_leds) & mask == 0 {
                        self.pending_leds &= !mask;
                    }
                }
                Ok(_) => (),
                Err(error) => warn!("input: rejected status buffer: {error}"),
            }
            self.queues[STATUS_QUEUE].add_used(chain.index, 0)?;
            used = true;
        }
        self.publish_used(STATUS_QUEUE, used)
    }
}

impl VirtioDevice for Input {
    impl_device_type!(VirtioDeviceType::Input);

    fn id(&self) -> &str {
        INPUT_DEV_ID
    }
    fn queues(&self) -> &[Queue] {
        &self.queues
    }
    fn queues_mut(&mut self) -> &mut [Queue] {
        &mut self.queues
    }
    fn queue_events(&self) -> &[EventFd] {
        &self.queue_events
    }
    fn interrupt_trigger(&self) -> &dyn VirtioInterrupt {
        self.device_state
            .active_state()
            .expect("Device is not initialized")
            .interrupt
            .as_ref()
    }
    fn avail_features(&self) -> u64 {
        FEATURES
    }
    fn acked_features(&self) -> u64 {
        self.acked_features
    }
    fn set_acked_features(&mut self, features: u64) {
        self.acked_features = features & FEATURES;
    }
    fn config_as_bytes(&self) -> &[u8] {
        &self.config
    }
    fn write_config(&mut self, offset: u64, data: &[u8]) {
        let Ok(offset) = usize::try_from(offset) else {
            return;
        };
        for (index, value) in self.config[..2].iter_mut().enumerate().skip(offset) {
            if let Some(&new) = data.get(index - offset) {
                *value = new;
            }
        }
        self.config = config(self.config[0], self.config[1]);
    }
    fn is_activated(&self) -> bool {
        self.device_state.is_activated()
    }
    fn deactivate(&mut self) {
        self.device_state = DeviceState::Inactive;
        self.stop_retry();
    }
    fn _reset(&mut self) -> bool {
        self.pending.clear();
        self.held.clear();
        self.delivered.clear();
        self.position = [0; 2];
        self.leds = 0;
        self.expected_leds = 0;
        self.pending_leds = 0;
        self.config = config(0, 0);
        self.stop_retry();
        self.drain_queue_events();
        true
    }
    fn activate(
        &mut self,
        mem: GuestMemoryMmap,
        interrupt: Arc<dyn VirtioInterrupt>,
    ) -> Result<(), ActivateError> {
        assert!(!self.is_activated(), "Input device is already activated");
        if self.acked_features & FEATURES == 0 {
            return Err(ActivateError::RequiredFeatureNotAcked("VIRTIO_F_VERSION_1"));
        }
        for (index, queue) in self.queues.iter_mut().enumerate() {
            if index == EVENT_QUEUE || queue.ready {
                queue
                    .initialize(&mem)
                    .map_err(ActivateError::QueueMemoryError)?;
            }
        }
        self.queue_events[EVENT_QUEUE]
            .write(1)
            .map_err(|_| ActivateError::EventFd)?;
        self.device_state = DeviceState::Activated(ActiveState { mem, interrupt });
        self.update_retry_timer();
        Ok(())
    }
}
