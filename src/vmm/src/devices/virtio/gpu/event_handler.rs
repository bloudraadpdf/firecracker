// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use event_manager::{EventOps, EventSet, Events, MutEventSubscriber};

use super::Gpu;
use crate::logger::warn;

impl MutEventSubscriber for Gpu {
    fn init(&mut self, ops: &mut EventOps) {
        self.registration_error = None;
        for (index, event) in self.queue_events.iter().enumerate() {
            if let Err(error) = ops.add(Events::with_data(
                event,
                u32::try_from(index).unwrap(),
                EventSet::IN,
            )) {
                self.registration_error = Some(error.to_string());
                for (previous, event) in self.queue_events[..index].iter().enumerate() {
                    if let Err(error) = ops.remove(Events::with_data(
                        event,
                        u32::try_from(previous).unwrap(),
                        EventSet::IN,
                    )) {
                        warn!("gpu: failed to roll back event registration: {error}");
                    }
                }
                return;
            }
        }
    }

    fn process(&mut self, events: Events, _ops: &mut EventOps) {
        if events.event_set() != EventSet::IN {
            return;
        }
        let index = events.data() as usize;
        let Some(event) = self.queue_events.get(index) else {
            return;
        };
        if let Err(error) = event.read() {
            warn!("gpu: failed to read queue event: {error}");
            return;
        }
        if let Err(error) = self.process_queue(index) {
            warn!("gpu: failed to process queue: {error}");
        }
    }
}
