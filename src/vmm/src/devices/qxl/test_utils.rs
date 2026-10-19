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
