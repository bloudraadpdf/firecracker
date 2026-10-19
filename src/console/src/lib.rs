// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#[cfg(all(target_os = "linux", feature = "bridge"))]
use std::io;

#[cfg(all(target_os = "linux", feature = "bridge"))]
mod auth;
#[cfg(all(target_os = "linux", feature = "bridge"))]
mod bridge;
#[cfg(all(target_os = "linux", feature = "bridge"))]
pub use bridge::run;

#[cfg(all(target_os = "linux", feature = "bridge"))]
const MAX_FRAME: usize = 32 * 1024 * 1024;

#[cfg(all(target_os = "linux", feature = "bridge"))]
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(all(target_os = "linux", feature = "bridge"))]
struct Frame {
    width: u16,
    height: u16,
    pixels: Vec<u8>,
}
