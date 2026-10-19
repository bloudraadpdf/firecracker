// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! QXL device protocol used by the existing Windows XP and Windows 7 drivers.

pub mod memory;
pub mod protocol;
pub mod transport;

#[cfg(test)]
mod memory_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod transport_tests;
