// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Combined keyboard and absolute-pointer virtio device.

#[cfg(feature = "windows")]
mod device;
#[cfg(feature = "windows")]
mod event_handler;
pub mod persist;
mod protocol;

#[cfg(feature = "windows")]
pub use device::Input;

pub const INPUT_DEV_ID: &str = "input";

use crate::devices::virtio::queue::{InvalidAvailIdx, QueueError};
use crate::vstate::interrupts::InterruptError;
use std::io;
use vm_memory::GuestMemoryError;

#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum InputError {
    /// Input I/O failed: {0}
    Io(#[from] io::Error),
    /// Unsupported host input event.
    InvalidHostEvent,
    /// Input frame queue is full; retry the complete update.
    PendingFull,
    /// Invalid input descriptor buffer.
    InvalidBuffer,
    /// Guest memory: {0}
    GuestMemory(#[from] GuestMemoryError),
    /// Input queue: {0}
    Queue(#[from] QueueError),
    /// Invalid available ring: {0}
    Avail(#[from] InvalidAvailIdx),
    /// Input interrupt: {0}
    Interrupt(#[from] InterruptError),
}

#[cfg(test)]
#[cfg(feature = "windows")]
pub(crate) mod test_utils;
#[cfg(test)]
#[cfg(feature = "windows")]
mod tests;
