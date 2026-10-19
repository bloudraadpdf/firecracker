// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use vm_memory::MmapRegion;

use super::memory::{Memory, RAM_HEADER, SURFACE_BYTES};
use super::transport::AddressSpace;

pub(super) fn memory() -> (Memory, AddressSpace) {
    let memory = Memory::new(Arc::new(MmapRegion::new(SURFACE_BYTES).unwrap())).unwrap();
    let mut addresses = AddressSpace::new(0x8000_0000, 0x8200_0000).unwrap();
    addresses
        .add(
            1,
            0x8000_0000..0x8000_0000 + u64::try_from(RAM_HEADER).unwrap(),
        )
        .unwrap();
    (memory, addresses)
}

pub(super) fn glyph(
    position: [i32; 2],
    origin: [i32; 2],
    width: u16,
    height: u16,
    data: &[u8],
) -> Vec<u8> {
    let mut bytes: Vec<u8> = position
        .into_iter()
        .chain(origin)
        .flat_map(i32::to_le_bytes)
        .collect();
    bytes.extend(width.to_le_bytes());
    bytes.extend(height.to_le_bytes());
    bytes.extend(data);
    bytes
}

pub(super) fn path(segments: &[(u32, &[[i32; 2]])]) -> Vec<u8> {
    let mut data = Vec::new();
    for &(flags, points) in segments {
        data.extend(flags.to_le_bytes());
        data.extend(u32::try_from(points.len()).unwrap().to_le_bytes());
        for point in points {
            data.extend(point.iter().flat_map(|coordinate| coordinate.to_le_bytes()));
        }
    }
    let length = u32::try_from(data.len()).unwrap();
    let mut result = Vec::new();
    result.extend(length.to_le_bytes());
    result.extend(length.to_le_bytes());
    result.extend([0; 16]);
    result.extend(data);
    result
}
