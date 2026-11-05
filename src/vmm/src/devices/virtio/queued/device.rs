// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::fmt::Debug;
use std::io;
use std::sync::Arc;

use event_manager::{EventOps, EventSet, Events, MutEventSubscriber};
use vmm_sys_util::eventfd::EventFd;

use super::QueuedState;
use crate::devices::virtio::ActivateError;
use crate::devices::virtio::buffers::Buffers;
use crate::devices::virtio::device::{ActiveState, DeviceState, VirtioDevice, VirtioDeviceType};
use crate::devices::virtio::persist::{PersistError, VirtioDeviceState};
use crate::devices::virtio::queue::{FIRECRACKER_MAX_QUEUE_SIZE, Queue, QueueConfig};
use crate::devices::virtio::transport::{VirtioInterrupt, VirtioInterruptType};
use crate::logger::warn;
use crate::snapshot::Persist;
use crate::vstate::memory::GuestMemoryMmap;

pub trait Protocol: Debug + Send + 'static {
    type State: Clone + Debug;
    const TYPE: VirtioDeviceType;
    const ID: &'static str;
    const FEATURES: u64;
    const QUEUES: usize;
    const REQUIRED: &'static [usize];
    const REQUEST_LIMIT: usize;
    const RESPONSE_LIMIT: usize;
    fn config(&self) -> &[u8];
    fn write_config(&mut self, offset: u64, bytes: &[u8]);
    fn reset(&mut self);
    fn process(
        &mut self,
        queue: usize,
        request: &[u8],
        capacity: usize,
    ) -> io::Result<Option<Vec<u8>>>;
    fn save(&self) -> Self::State;
    fn restore(state: &Self::State) -> io::Result<Self>
    where
        Self: Sized;
}

#[derive(Debug)]
pub struct QueuedDevice<P: Protocol> {
    pub protocol: P,
    queues: Vec<Queue>,
    events: Vec<EventFd>,
    active: DeviceState,
    acked: u64,
    registration_error: Option<String>,
}

impl<P: Protocol> QueuedDevice<P> {
    pub fn new(protocol: P) -> io::Result<Self> {
        Ok(Self {
            protocol,
            queues: (0..P::QUEUES)
                .map(|_| Queue::new(FIRECRACKER_MAX_QUEUE_SIZE))
                .collect(),
            events: (0..P::QUEUES)
                .map(|_| EventFd::new(libc::EFD_NONBLOCK))
                .collect::<io::Result<_>>()?,
            active: DeviceState::Inactive,
            acked: 0,
            registration_error: Some("device events are not registered".into()),
        })
    }

    pub fn check_registered(&self) -> io::Result<()> {
        self.registration_error
            .as_ref()
            .map_or(Ok(()), |error| Err(io::Error::other(error.clone())))
    }

    fn process_queue(&mut self, index: usize) -> io::Result<()> {
        let Some(active) = self.active.active_state() else {
            return Ok(());
        };
        let queue = &mut self.queues[index];
        if !queue.config.ready {
            return Ok(());
        }
        let mut used = false;
        let result = (|| {
            for _ in 0..queue.config.size {
                let Some(chain) = queue.pop().map_err(io::Error::other)? else {
                    break;
                };
                let head = chain.index;
                let response = (|| {
                    let buffers =
                        Buffers::new(&active.mem, chain, P::REQUEST_LIMIT, P::RESPONSE_LIMIT)
                            .map_err(io::Error::other)?;
                    let Some(response) =
                        self.protocol
                            .process(index, &buffers.request, buffers.capacity)?
                    else {
                        return Ok(None);
                    };
                    buffers
                        .respond(&active.mem, &response)
                        .map(Some)
                        .map_err(io::Error::other)
                })();
                let length = match response {
                    Ok(Some(length)) => length,
                    Ok(None) => {
                        queue.undo_pop();
                        break;
                    }
                    Err(error) => {
                        warn!("{}: invalid queue {index} request: {error}", P::ID);
                        0
                    }
                };
                queue.add_used(head, length).map_err(io::Error::other)?;
                used = true;
            }
            Ok(())
        })();
        queue.advance_used_ring_idx();
        if used && queue.prepare_kick() {
            active
                .interrupt
                .trigger(VirtioInterruptType::Queue(index.try_into().unwrap()))
                .map_err(io::Error::other)?;
        }
        result
    }
}

impl<P: Protocol> MutEventSubscriber for QueuedDevice<P> {
    fn init(&mut self, ops: &mut EventOps) {
        self.registration_error = None;
        for (index, event) in self.events.iter().enumerate() {
            if let Err(error) = ops.add(Events::with_data(
                event,
                index.try_into().unwrap(),
                EventSet::IN,
            )) {
                self.registration_error = Some(error.to_string());
                for (number, previous) in self.events[..index].iter().enumerate() {
                    if let Err(error) = ops.remove(Events::with_data(
                        previous,
                        number.try_into().unwrap(),
                        EventSet::IN,
                    )) {
                        warn!("{}: event registration rollback failed: {error}", P::ID);
                    }
                }
                break;
            }
        }
    }

    fn process(&mut self, event: Events, _ops: &mut EventOps) {
        let index = event.data() as usize;
        if event.event_set() != EventSet::IN || index >= self.events.len() {
            return;
        }
        if let Err(error) = self.events[index].read() {
            warn!("{}: queue event read failed: {error}", P::ID);
            return;
        }
        for queue in (0..P::QUEUES).rev() {
            if let Err(error) = self.process_queue(queue) {
                warn!("{}: queue processing failed: {error}", P::ID);
            }
        }
    }
}

impl<P: Protocol> VirtioDevice for QueuedDevice<P> {
    fn const_device_type() -> VirtioDeviceType {
        P::TYPE
    }
    fn device_type(&self) -> VirtioDeviceType {
        P::TYPE
    }
    fn id(&self) -> &str {
        P::ID
    }
    fn num_queues(&self) -> usize {
        self.queues.len()
    }
    fn queue_config(&self, index: usize) -> Option<&QueueConfig> {
        self.queues.get(index).map(|queue| &queue.config)
    }
    fn queue_config_mut(&mut self, index: usize) -> Option<&mut QueueConfig> {
        self.queues.get_mut(index).map(|queue| &mut queue.config)
    }
    fn queue_event(&self, index: usize) -> Option<&EventFd> {
        self.events.get(index)
    }
    fn reset_queues(&mut self) {
        self.queues.iter_mut().for_each(Queue::reset);
    }
    fn commit_dirty(&self) {
        self.queues.iter().for_each(Queue::commit_dirty);
    }
    fn interrupt_trigger(&self) -> &dyn VirtioInterrupt {
        self.active
            .active_state()
            .expect("device is inactive")
            .interrupt
            .as_ref()
    }
    fn avail_features(&self) -> u64 {
        P::FEATURES
    }
    fn acked_features(&self) -> u64 {
        self.acked
    }
    fn set_acked_features(&mut self, features: u64) {
        self.acked = features & P::FEATURES;
    }
    fn config_as_bytes(&self) -> &[u8] {
        self.protocol.config()
    }
    fn write_config(&mut self, offset: u64, bytes: &[u8]) {
        self.protocol.write_config(offset, bytes);
    }
    fn is_activated(&self) -> bool {
        self.active.is_activated()
    }
    fn deactivate(&mut self) {
        self.active = DeviceState::Inactive;
    }
    fn _reset(&mut self) -> bool {
        self.protocol.reset();
        self.drain_queue_events();
        true
    }
    fn activate(
        &mut self,
        mem: GuestMemoryMmap,
        interrupt: Arc<dyn VirtioInterrupt>,
    ) -> Result<(), ActivateError> {
        assert!(!self.is_activated());
        if self.acked & (1 << 32) == 0 {
            return Err(ActivateError::RequiredFeatureNotAcked("VIRTIO_F_VERSION_1"));
        }
        for (index, queue) in self.queues.iter_mut().enumerate() {
            if queue.config.ready || P::REQUIRED.contains(&index) {
                queue
                    .initialize(&mem)
                    .map_err(ActivateError::QueueMemoryError)?;
            }
        }
        for event in &self.events {
            event.write(1).map_err(|_| ActivateError::EventFd)?;
        }
        self.active = DeviceState::Activated(ActiveState { mem, interrupt });
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum QueuedPersistError {
    #[error("device state: {0}")]
    State(#[from] io::Error),
    #[error("virtio state: {0}")]
    Virtio(#[from] PersistError),
}

impl<P: Protocol> Persist<'_> for QueuedDevice<P> {
    type State = QueuedState<P::State>;
    type ConstructorArgs = GuestMemoryMmap;
    type Error = QueuedPersistError;
    fn save(&self) -> Self::State {
        QueuedState {
            virtio_state: VirtioDeviceState::from_device(self, &self.queues),
            protocol: self.protocol.save(),
        }
    }
    fn restore(mem: Self::ConstructorArgs, state: &Self::State) -> Result<Self, Self::Error> {
        if state.virtio_state.avail_features != P::FEATURES
            || state.virtio_state.acked_features & !P::FEATURES != 0
            || (state.virtio_state.activated && state.virtio_state.acked_features & (1 << 32) == 0)
        {
            return Err(io::Error::other("invalid device features").into());
        }
        let queues = state.virtio_state.build_queues_checked(
            &mem,
            P::TYPE,
            P::QUEUES,
            FIRECRACKER_MAX_QUEUE_SIZE,
        )?;
        if state.virtio_state.activated
            && P::REQUIRED.iter().any(|&index| !queues[index].config.ready)
        {
            return Err(io::Error::other("required device queue is absent").into());
        }
        let mut device = Self::new(P::restore(&state.protocol)?)?;
        device.queues = queues;
        device.acked = state.virtio_state.acked_features;
        Ok(device)
    }
}

#[cfg(test)]
mod tests {
    use vm_memory::Bytes;

    use super::*;
    use crate::devices::virtio::queue::{VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE};
    use crate::devices::virtio::scsi::Scsi;
    use crate::devices::virtio::test_utils::{VirtQueue, default_mem};
    use crate::devices::virtio::transport::mmio::IrqTrigger;
    use crate::vstate::memory::GuestAddress;

    #[test]
    fn scsi_queue_validates_descriptors_and_restores_pending_work() {
        let mem = default_mem();
        let ring = VirtQueue::new(GuestAddress(0x1000), &mem, 8);
        let mut device = QueuedDevice::new(Scsi::new(&[None]).unwrap()).unwrap();
        device.queues[2] = ring.create_queue();
        device.queues[2].config.max_size = FIRECRACKER_MAX_QUEUE_SIZE;
        device.set_acked_features(1 << 32);
        device
            .activate(mem.clone(), Arc::new(IrqTrigger::new()))
            .unwrap();
        let mut command = [0; 51];
        command[0] = 1;
        command[19] = 0x12;
        command[23] = 36;
        mem.write_slice(&command, GuestAddress(0x8000)).unwrap();
        ring.dtable[0].set(0x8000, 51, VIRTQ_DESC_F_NEXT, 1);
        ring.dtable[1].set(0x9000, 144, VIRTQ_DESC_F_WRITE, 0);
        ring.avail.ring[0].set(0);
        ring.avail.idx.set(1);
        let saved = device.save();
        let mut restored = QueuedDevice::<Scsi>::restore(mem.clone(), &saved).unwrap();
        restored
            .activate(mem.clone(), Arc::new(IrqTrigger::new()))
            .unwrap();
        restored.process_queue(2).unwrap();
        ring.check_used_elem(0, 0, 144);
        let mut response = [0; 144];
        mem.read_slice(&mut response, GuestAddress(0x9000)).unwrap();
        assert_eq!(response[108], 5);
        ring.dtable[1].addr.set(u64::MAX);
        ring.avail.ring[1].set(0);
        ring.avail.idx.set(2);
        restored.process_queue(2).unwrap();
        ring.check_used_elem(1, 0, 0);
        let mut invalid = saved;
        invalid.virtio_state.acked_features = 0;
        QueuedDevice::<Scsi>::restore(mem, &invalid).unwrap_err();
    }
}
