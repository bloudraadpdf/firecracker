// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! QXL device protocol used by the existing Windows XP and Windows 7 drivers.

pub mod bitmap;
pub mod drawing;
mod glyph;
pub mod line;
pub mod memory;
mod path;
pub mod protocol;
pub mod raster;
pub mod surface;
pub mod transport;

#[cfg(test)]
mod bitmap_tests;
#[cfg(test)]
mod drawing_tests;
#[cfg(test)]
mod memory_tests;
#[cfg(test)]
mod path_tests;
#[cfg(test)]
mod surface_tests;
#[cfg(test)]
mod test_utils;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod transport_tests;
