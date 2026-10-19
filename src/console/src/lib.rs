// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#[cfg(any(all(target_os = "linux", feature = "bridge"), feature = "gui"))]
use std::io;

#[cfg(any(all(target_os = "linux", feature = "bridge"), feature = "gui"))]
mod auth;
#[cfg(all(target_os = "linux", feature = "bridge"))]
mod bridge;
#[cfg(all(target_os = "linux", feature = "bridge", feature = "gui"))]
pub use bridge::LocalBridge;
#[cfg(all(target_os = "linux", feature = "bridge"))]
pub use bridge::run;

#[cfg(any(feature = "gui", all(test, target_os = "linux", feature = "bridge")))]
mod client;
#[cfg(feature = "gui")]
mod graphics;
#[cfg(feature = "gui")]
mod gui;
#[cfg(any(feature = "gui", all(test, target_os = "linux", feature = "bridge")))]
mod input;
#[cfg(feature = "gui")]
pub use gui::{ViewerOptions, view, view_with_child};

#[cfg(any(all(target_os = "linux", feature = "bridge"), feature = "gui"))]
const MAX_FRAME: usize = 32 * 1024 * 1024;

#[cfg(any(all(target_os = "linux", feature = "bridge"), feature = "gui"))]
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(any(all(target_os = "linux", feature = "bridge"), feature = "gui"))]
const RFB_FORMAT: [u8; 16] = [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0];

#[cfg(any(all(target_os = "linux", feature = "bridge"), feature = "gui"))]
fn read<const N: usize>(stream: &mut impl io::Read) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[derive(Clone, Debug)]
#[cfg(any(all(target_os = "linux", feature = "bridge"), feature = "gui"))]
struct Frame {
    width: u16,
    height: u16,
    pixels: Vec<u8>,
}
