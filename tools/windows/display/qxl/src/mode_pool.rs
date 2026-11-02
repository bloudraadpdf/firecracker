// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Owned miniport mode records, copied from ROM into VideoPort pool storage.

use core::ffi::c_void;
use core::mem::ManuallyDrop;
use core::ptr::{NonNull, copy_nonoverlapping};

use crate::modes::{ModeError, ModeSet, ModeTable, VideoModeInformation};
use crate::resources::DeviceResources;
use crate::videoport::{VideoPortAllocatePool, VideoPortFreePool};

const INVALID_DATA: u32 = 13;
const NOT_ENOUGH_MEMORY: u32 = 8;

#[derive(Debug)]
struct Allocation {
    device: NonNull<c_void>,
    memory: NonNull<u8>,
}

impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: this allocation has one owner and its extension remains live.
        unsafe { VideoPortFreePool(self.device.as_ptr(), self.memory.as_ptr().cast()) };
    }
}

#[derive(Debug)]
pub struct ModePool {
    // The borrowed records stay at a stable pool address when this owner moves.
    state: ManuallyDrop<ModeSet<'static>>,
    allocation: ManuallyDrop<Allocation>,
}

impl ModePool {
    /// # Safety
    /// Call at PASSIVE_LEVEL with live device resources. Retain the VideoPort
    /// extension through this object's destruction. All access and destruction
    /// must remain at PASSIVE_LEVEL, serialised with miniport mode requests.
    pub unsafe fn new(resources: &DeviceResources) -> Result<Self, u32> {
        let bytes = resources
            .mode_count
            .checked_mul(32)
            .and_then(|bytes| bytes.checked_add(4))
            .ok_or(INVALID_DATA)? as usize;
        let device = resources.device();
        // SAFETY: the extension is live and pool allocation occurs at PASSIVE_LEVEL.
        // VpPagedPool is 1; the allocation tag preserves the upstream QXL tag.
        let memory = unsafe { VideoPortAllocatePool(device.as_ptr(), 1, bytes, 0x5f6c7871) };
        let allocation = Allocation {
            device,
            memory: NonNull::new(memory.cast()).ok_or(NOT_ENOUGH_MEMORY)?,
        };
        let count = resources.mode_count.to_le_bytes();
        // SAFETY: the allocation has at least four bytes, and count is a distinct
        // live input. Every remaining byte is initialised below before borrowing.
        unsafe { copy_nonoverlapping(count.as_ptr(), allocation.memory.as_ptr(), 4) };
        for index in 0..resources.mode_count {
            let offset = 4 + index * 32;
            let record = resources.rom.snapshot::<32>(
                resources
                    .info
                    .mode_table_offset()
                    .checked_add(offset)
                    .ok_or(INVALID_DATA)?,
            )?;
            // SAFETY: offset comes from the checked allocation size; the stack
            // snapshot and pool storage are distinct and valid for 32 bytes.
            unsafe {
                copy_nonoverlapping(
                    record.as_ptr(),
                    allocation.memory.as_ptr().add(offset as usize),
                    32,
                )
            };
        }
        // SAFETY: all pool bytes are initialised and remain immutable. The slice
        // is private to state, which never escapes this owner. Drop ends that
        // borrow before releasing the stable allocation; methods return copies
        // or iterators limited to the borrow of this owner.
        let records = unsafe { core::slice::from_raw_parts(allocation.memory.as_ptr(), bytes) };
        let table = ModeTable::parse(records, 0).map_err(|_| INVALID_DATA)?;
        let state = ModeSet::new(table, resources.info.primary_surface_bytes())
            .map_err(|_| INVALID_DATA)?;
        Ok(Self {
            state: ManuallyDrop::new(state),
            allocation: ManuallyDrop::new(allocation),
        })
    }

    pub fn count(&self) -> usize {
        self.state.count()
    }

    pub fn entries(&self) -> impl Iterator<Item = Result<VideoModeInformation, ModeError>> + '_ {
        self.state.entries()
    }

    pub fn find(&self, id: u32) -> Result<VideoModeInformation, ModeError> {
        self.state.find(id)
    }

    pub fn current(&self) -> Result<VideoModeInformation, ModeError> {
        self.state.current()
    }

    pub fn set_current(&mut self, id: u32) -> Result<(), ModeError> {
        self.state.set_current(id)
    }

    pub fn set_custom(&mut self, width: u32, height: u32, depth: u32) -> Result<u32, ModeError> {
        self.state.set_custom(width, height, depth)
    }
}

impl Drop for ModePool {
    fn drop(&mut self) {
        // SAFETY: state is initialised exactly once and cannot escape. Ending
        // its borrow precedes releasing the backing allocation. Neither field
        // can be moved out, and each is dropped exactly once here.
        unsafe {
            ManuallyDrop::drop(&mut self.state);
            ManuallyDrop::drop(&mut self.allocation);
        }
    }
}
