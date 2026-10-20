// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! QXL device protocol used by the existing Windows XP and Windows 7 drivers.

#[cfg(feature = "windows")]
pub mod bitmap;
#[cfg(feature = "windows")]
pub mod controller;
#[cfg(feature = "windows")]
pub mod cursor;
#[cfg(feature = "windows")]
pub mod drawing;
#[cfg(feature = "windows")]
mod glyph;
#[cfg(feature = "windows")]
pub mod line;
#[cfg(feature = "windows")]
pub mod memory;
#[cfg(feature = "windows")]
mod path;
#[cfg(feature = "windows")]
pub mod pci;
#[cfg(feature = "windows")]
pub mod protocol;
#[cfg(feature = "windows")]
pub mod raster;
pub mod state;
#[cfg(feature = "windows")]
pub mod surface;
#[cfg(feature = "windows")]
pub mod transport;

#[cfg(test)]
#[cfg(feature = "windows")]
mod bitmap_tests;
#[cfg(test)]
#[cfg(feature = "windows")]
mod controller_tests;
#[cfg(test)]
#[cfg(feature = "windows")]
mod cursor_tests;
#[cfg(test)]
#[cfg(feature = "windows")]
mod drawing_tests;
#[cfg(test)]
#[cfg(feature = "windows")]
mod memory_tests;
#[cfg(test)]
#[cfg(feature = "windows")]
mod path_tests;
#[cfg(test)]
#[cfg(feature = "windows")]
mod surface_tests;
#[cfg(test)]
#[cfg(feature = "windows")]
mod test_utils;
#[cfg(test)]
#[cfg(feature = "windows")]
mod tests;
#[cfg(test)]
#[cfg(feature = "windows")]
mod transport_tests;
