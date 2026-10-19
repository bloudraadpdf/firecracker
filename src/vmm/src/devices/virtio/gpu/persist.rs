// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

use super::RendererState;
#[cfg(feature = "windows")]
use super::device::{FEATURES, NUM_QUEUES};
#[cfg(feature = "windows")]
use super::renderer::Renderer;
#[cfg(feature = "windows")]
use super::{Gpu, GpuError};
#[cfg(feature = "windows")]
use crate::devices::virtio::device::{VirtioDevice, VirtioDeviceType};
use crate::devices::virtio::persist::VirtioDeviceState;
#[cfg(feature = "windows")]
use crate::devices::virtio::queue::FIRECRACKER_MAX_QUEUE_SIZE;
#[cfg(feature = "windows")]
use crate::snapshot::Persist;
#[cfg(feature = "windows")]
use crate::vstate::memory::GuestMemoryMmap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GpuState {
    pub virtio_state: VirtioDeviceState,
    pub(super) events: u32,
    pub(super) renderer: RendererState,
}

#[cfg(feature = "windows")]
#[derive(Debug)]
pub struct GpuConstructorArgs {
    pub mem: GuestMemoryMmap,
}

#[cfg(feature = "windows")]
#[derive(Debug, thiserror::Error)]
pub enum GpuPersistError {
    #[error("create GPU: {0}")]
    Create(#[from] GpuError),
    #[error("GPU virtio state: {0}")]
    Virtio(#[from] crate::devices::virtio::persist::PersistError),
    #[error("GPU event registration: {0}")]
    Registration(std::io::Error),
    #[error("invalid GPU device state")]
    InvalidState,
}

#[cfg(feature = "windows")]
impl Persist<'_> for Gpu {
    type State = GpuState;
    type ConstructorArgs = GpuConstructorArgs;
    type Error = GpuPersistError;

    fn save(&self) -> Self::State {
        GpuState {
            virtio_state: VirtioDeviceState::from_device(self),
            events: u32::from_le_bytes(self.config[..4].try_into().unwrap()),
            renderer: self.renderer.lock().unwrap().save(),
        }
    }

    fn restore(args: Self::ConstructorArgs, state: &Self::State) -> Result<Self, Self::Error> {
        if state.virtio_state.avail_features != FEATURES
            || state.virtio_state.acked_features & !FEATURES != 0
            || (state.virtio_state.activated && state.virtio_state.acked_features != FEATURES)
            || state.events & !1 != 0
        {
            return Err(GpuPersistError::InvalidState);
        }
        let queues = state.virtio_state.build_queues_checked(
            &args.mem,
            VirtioDeviceType::Gpu,
            NUM_QUEUES,
            FIRECRACKER_MAX_QUEUE_SIZE,
        )?;
        if state.virtio_state.activated && !queues[0].ready {
            return Err(GpuPersistError::InvalidState);
        }
        let renderer = Renderer::restore(&state.renderer, &args.mem).map_err(GpuError::from)?;
        let mut device = Self::from_renderer(renderer)?;
        device.queues = queues;
        device.set_acked_features(state.virtio_state.acked_features);
        device.config[..4].copy_from_slice(&state.events.to_le_bytes());
        Ok(device)
    }
}
