// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use vm_memory::MmapRegion;

use super::drawing::Layout;
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

pub(super) fn rect([left, top, right, bottom]: [i32; 4]) -> Vec<u8> {
    [top, left, bottom, right]
        .into_iter()
        .flat_map(i32::to_le_bytes)
        .collect()
}

pub(super) fn drawable(
    layout: Layout,
    opcode: u8,
    bounds: [i32; 4],
    payload: &[u8],
    surface_id: u32,
) -> Vec<u8> {
    let (size, rectangle, clip) = match layout {
        Layout::Modern => (123, 31, 47),
        Layout::Compat => (60, 28, 44),
    };
    let mut data = vec![0xa5; size];
    data[..8].copy_from_slice(&0x1234u64.to_le_bytes());
    match layout {
        Layout::Modern => {
            data[8..12].copy_from_slice(&surface_id.to_le_bytes());
            data[12] = 0;
            data[13] = opcode;
            data[14] = 0;
        }
        Layout::Compat => {
            data[8] = 0;
            data[9] = opcode;
            data[10..12].fill(0);
        }
    }
    data[rectangle..rectangle + 16].copy_from_slice(&rect(bounds));
    data[clip..clip + 4].fill(0);
    data.extend(payload);
    data
}

pub(super) fn fill(color: u32, descriptor: u16) -> Vec<u8> {
    let mut data = vec![0xa5; 39];
    data[..4].copy_from_slice(&1u32.to_le_bytes());
    data[4..8].copy_from_slice(&color.to_le_bytes());
    data[20..22].copy_from_slice(&descriptor.to_le_bytes());
    data[31..39].fill(0);
    data
}
