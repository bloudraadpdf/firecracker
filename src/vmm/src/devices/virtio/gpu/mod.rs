// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Virtio GPU 2D device and checkpoint state.

#[cfg(feature = "windows")]
mod device;
#[cfg(feature = "windows")]
mod event_handler;
pub mod persist;
pub mod protocol;
#[cfg(feature = "windows")]
pub mod queue;
#[cfg(feature = "windows")]
pub mod renderer;
mod state;

#[cfg(feature = "windows")]
pub use device::{GPU_DEV_ID, Gpu, GpuError};
pub use state::RendererState;

#[cfg(test)]
mod test_utils;
