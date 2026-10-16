// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use event_manager::{EventOps, EventSet, Events, MutEventSubscriber};

use super::Input;
use super::protocol::{EVENT_QUEUE, NUM_QUEUES, STATUS_QUEUE};
use crate::devices::virtio::device::VirtioDevice;
use crate::logger::warn;

impl MutEventSubscriber for Input {
    fn init(&mut self, ops: &mut EventOps) {
        self.registration_error = None;
        let events = [
            Events::with_data(
                &self.queue_events[EVENT_QUEUE],
                EVENT_QUEUE.try_into().unwrap(),
                EventSet::IN,
            ),
            Events::with_data(
                &self.queue_events[STATUS_QUEUE],
                STATUS_QUEUE.try_into().unwrap(),
                EventSet::IN,
            ),
            Events::with_data(
                &self.retry_timer,
                NUM_QUEUES.try_into().unwrap(),
                EventSet::IN,
            ),
        ];
        for (index, &event) in events.iter().enumerate() {
            if let Err(error) = ops.add(event) {
                self.registration_error = Some(error.to_string());
                for &previous in &events[..index] {
                    if let Err(error) = ops.remove(previous) {
                        warn!("input: failed to roll back input registration: {error}");
                    }
                }
                self.stop_retry();
                return;
            }
        }
        self.update_retry_timer();
    }

    fn process(&mut self, events: Events, _ops: &mut EventOps) {
        let index = events.data() as usize;
        if events.event_set() != EventSet::IN {
            return;
        }
        if index == NUM_QUEUES {
            self.retry_timer.read();
            self.retry_armed = false;
            if self.is_activated() {
                self.flush_events().unwrap();
            }
            return;
        }
        let Some(event) = self.queue_events.get(index) else {
            return;
        };
        if let Err(error) = event.read() {
            warn!("input: failed to read queue event: {error}");
            return;
        }
        if !self.is_activated() {
            return;
        }
        if index == EVENT_QUEUE {
            self.flush_events().unwrap();
        } else {
            self.process_status().unwrap();
        }
    }
}
