// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use vm_memory::{Bytes, MmapRegion, VolatileMemory};

use super::FRAMEBUFFER_SIZE;

/// Page-aligned VRAM shared with KVM; host accesses never form Rust byte references.
#[derive(Debug)]
pub(super) struct VideoMemory(Arc<MmapRegion>);

impl VideoMemory {
    fn new() -> Result<Self, vm_memory::mmap::MmapRegionError> {
        Ok(Self(Arc::new(MmapRegion::new(FRAMEBUFFER_SIZE)?)))
    }

    pub(super) fn mapping(&self) -> Arc<MmapRegion> {
        self.0.clone()
    }

    pub(super) fn len(&self) -> usize {
        self.0.size()
    }

    pub(super) fn read(&self, offset: usize, bytes: &mut [u8]) {
        self.0
            .get_slice(offset, bytes.len())
            .unwrap()
            .read_slice(bytes, 0)
            .unwrap();
    }

    pub(super) fn write(&self, offset: usize, bytes: &[u8]) {
        self.0
            .get_slice(offset, bytes.len())
            .unwrap()
            .write_slice(bytes, 0)
            .unwrap();
    }

    pub(super) fn byte(&self, offset: usize) -> u8 {
        let mut byte = [0];
        self.read(offset, &mut byte);
        byte[0]
    }

    pub(super) fn set_byte(&self, offset: usize, byte: u8) {
        self.write(offset, &[byte]);
    }

    pub(super) fn clear(&self) {
        let zeroes = [0; 4096];
        for offset in (0..FRAMEBUFFER_SIZE).step_by(zeroes.len()) {
            self.write(offset, &zeroes);
        }
    }

    pub(super) fn from_bytes(bytes: &[u8]) -> Result<Self, vm_memory::mmap::MmapRegionError> {
        let memory = Self::new()?;
        memory.write(0, bytes);
        Ok(memory)
    }

    pub(super) fn bytes(&self) -> Vec<u8> {
        let mut bytes = vec![0; FRAMEBUFFER_SIZE];
        self.read(0, &mut bytes);
        bytes
    }
}

impl Default for VideoMemory {
    fn default() -> Self {
        Self::new().expect("failed to allocate video memory")
    }
}

impl Clone for VideoMemory {
    fn clone(&self) -> Self {
        let clone = Self::default();
        clone.write(0, &self.bytes());
        clone
    }
}
