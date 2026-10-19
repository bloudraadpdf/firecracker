// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::sync::{Arc, Mutex};

use vmm_sys_util::eventfd::EventFd;

use super::protocol::Queue as QueueKind;
use super::queue::{GpuQueueError, process_queue};
use super::renderer::{Renderer, RendererError};
use crate::devices::virtio::ActivateError;
use crate::devices::virtio::device::{ActiveState, DeviceState, VirtioDevice, VirtioDeviceType};
use crate::devices::virtio::generated::virtio_config::VIRTIO_F_VERSION_1;
use crate::devices::virtio::queue::{FIRECRACKER_MAX_QUEUE_SIZE, Queue};
use crate::devices::virtio::transport::{VirtioInterrupt, VirtioInterruptType};
use crate::impl_device_type;
use crate::vstate::interrupts::InterruptError;
use crate::vstate::memory::GuestMemoryMmap;

pub(super) const FEATURES: u64 = 1 << VIRTIO_F_VERSION_1;
pub(super) const NUM_QUEUES: usize = 2;
pub const GPU_DEV_ID: &str = "gpu";

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("GPU I/O: {0}")]
    Io(#[from] io::Error),
    #[error("GPU renderer: {0}")]
    Renderer(#[from] RendererError),
    #[error("GPU queue: {0}")]
    Queue(#[from] GpuQueueError),
    #[error("GPU interrupt: {0}")]
    Interrupt(#[from] InterruptError),
}

#[derive(Debug)]
pub struct Gpu {
    pub(super) acked_features: u64,
    pub(super) device_state: DeviceState,
    pub(super) queues: Vec<Queue>,
    pub(super) queue_events: Vec<EventFd>,
    pub(super) config: [u8; 16],
    pub(super) renderer: Arc<Mutex<Renderer>>,
    pub(super) registration_error: Option<String>,
}

impl Gpu {
    pub fn new(width: u32, height: u32) -> Result<Self, GpuError> {
        Self::from_renderer(Renderer::new(width, height)?)
    }

    pub(super) fn from_renderer(renderer: Renderer) -> Result<Self, GpuError> {
        let mut config = [0; 16];
        config[8..12].copy_from_slice(&1u32.to_le_bytes());
        Ok(Self {
            acked_features: 0,
            device_state: DeviceState::Inactive,
            queues: vec![Queue::new(FIRECRACKER_MAX_QUEUE_SIZE); NUM_QUEUES],
            queue_events: (0..NUM_QUEUES)
                .map(|_| EventFd::new(libc::EFD_NONBLOCK))
                .collect::<Result<_, _>>()?,
            config,
            renderer: Arc::new(Mutex::new(renderer)),
            registration_error: Some("GPU events are not registered".into()),
        })
    }

    pub fn renderer(&self) -> Arc<Mutex<Renderer>> {
        self.renderer.clone()
    }

    pub fn check_registered(&self) -> io::Result<()> {
        self.registration_error
            .as_ref()
            .map_or(Ok(()), |error| Err(io::Error::other(error.clone())))
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), GpuError> {
        if self.renderer.lock().unwrap().resize(width, height)? {
            self.config[0] |= 1;
            if let Some(active) = self.device_state.active_state() {
                active.interrupt.trigger(VirtioInterruptType::Config)?;
            }
        }
        Ok(())
    }

    pub(super) fn process_queue(&mut self, index: usize) -> Result<(), GpuError> {
        let Some(active) = self.device_state.active_state() else {
            return Ok(());
        };
        if !self.queues[index].ready {
            return Ok(());
        }
        let kind = if index == 0 {
            QueueKind::Control
        } else {
            QueueKind::Cursor
        };
        if process_queue(
            &mut self.queues[index],
            kind,
            &mut self.renderer.lock().unwrap(),
            &active.mem,
        )? {
            active
                .interrupt
                .trigger(VirtioInterruptType::Queue(u16::try_from(index).unwrap()))?;
        }
        Ok(())
    }
}

impl VirtioDevice for Gpu {
    impl_device_type!(VirtioDeviceType::Gpu);
    fn id(&self) -> &str {
        GPU_DEV_ID
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
            .expect("GPU is not activated")
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
        let mut clear = [0; 4];
        for (index, &value) in data.iter().enumerate() {
            if let Some(position @ 4..8) = offset.checked_add(index) {
                clear[position - 4] = value;
            }
        }
        let events = u32::from_le_bytes(self.config[..4].try_into().unwrap());
        self.config[..4].copy_from_slice(&(events & !u32::from_le_bytes(clear)).to_le_bytes());
    }
    fn is_activated(&self) -> bool {
        self.device_state.is_activated()
    }
    fn deactivate(&mut self) {
        self.device_state = DeviceState::Inactive;
    }
    fn _reset(&mut self) -> bool {
        self.renderer.lock().unwrap().reset();
        self.config[..8].fill(0);
        self.drain_queue_events();
        true
    }
    fn activate(
        &mut self,
        mem: GuestMemoryMmap,
        interrupt: Arc<dyn VirtioInterrupt>,
    ) -> Result<(), ActivateError> {
        assert!(!self.is_activated(), "GPU is already activated");
        if self.acked_features != FEATURES {
            return Err(ActivateError::RequiredFeatureNotAcked("VIRTIO_F_VERSION_1"));
        }
        for (index, queue) in self.queues.iter_mut().enumerate() {
            if index == 0 || queue.ready {
                queue
                    .initialize(&mem)
                    .map_err(ActivateError::QueueMemoryError)?;
            }
        }
        for event in &self.queue_events {
            event.write(1).map_err(|_| ActivateError::EventFd)?;
        }
        if self.config[0] & 1 != 0 {
            interrupt
                .trigger(VirtioInterruptType::Config)
                .map_err(ActivateError::Interrupt)?;
        }
        self.device_state = DeviceState::Activated(ActiveState { mem, interrupt });
        Ok(())
    }
}

#[cfg(test)]
#[path = "device_tests.rs"]
mod tests;
