// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Virtio GPU 2D protocol. PCI attachment and display presentation are separate.

pub mod protocol;
pub mod queue;
pub mod renderer;
mod state;

pub use state::RendererState;

#[cfg(test)]
mod test_utils;
