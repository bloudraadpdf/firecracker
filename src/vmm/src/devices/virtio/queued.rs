// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

use super::persist::VirtioDeviceState;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedState<S> {
    pub virtio_state: VirtioDeviceState,
    pub protocol: S,
}

#[cfg(feature = "windows")]
mod device;
#[cfg(feature = "windows")]
pub use device::{Protocol, QueuedDevice, QueuedPersistError};
