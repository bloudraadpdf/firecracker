// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::sync::{Arc, Weak};

use kvm_bindings::kvm_userspace_memory_region;
use vm_memory::MmapRegion;

use crate::vstate::vm::KvmVm;

/// Owns a KVM slot and keeps its backing mapped until the slot is removed.
#[derive(Debug)]
pub(crate) struct FramebufferMemory {
    vm: Weak<KvmVm>,
    memory: Arc<MmapRegion>,
    slot: u32,
    address: Option<u64>,
}

impl FramebufferMemory {
    pub(crate) fn new(vm: &Arc<KvmVm>, memory: Arc<MmapRegion>) -> io::Result<Self> {
        let slot = vm
            .next_kvm_slot(1)
            .ok_or_else(|| io::Error::other("no framebuffer KVM slot"))?;
        Ok(Self {
            vm: Arc::downgrade(vm),
            memory,
            slot,
            address: None,
        })
    }

    pub(crate) fn set_address(&mut self, address: Option<u64>) -> io::Result<()> {
        if self.address == address {
            return Ok(());
        }
        let vm = self
            .vm
            .upgrade()
            .ok_or_else(|| io::Error::other("framebuffer VM is gone"))?;
        let region = kvm_userspace_memory_region {
            slot: self.slot,
            flags: 0,
            guest_phys_addr: address.unwrap_or(0),
            memory_size: if address.is_some() {
                self.memory.size() as u64
            } else {
                0
            },
            userspace_addr: self.memory.as_ptr() as u64,
        };
        // KVM invalidates the old slot, synchronises SRCU and flushes shadow mappings
        // before a move/delete; no userspace barrier is needed for running vCPUs.
        vm.set_user_memory_region(region)
            .map_err(io::Error::other)?;
        self.address = address;
        Ok(())
    }
}

impl Drop for FramebufferMemory {
    fn drop(&mut self) {
        if let Some(vm) = self.vm.upgrade()
            && let Err(error) = self.set_address(None)
        {
            crate::logger::error!("Failed to remove framebuffer KVM slot: {error}");
            vm.retain_failed_memory_mapping(self.memory.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    fn backing() -> Arc<MmapRegion> {
        Arc::new(MmapRegion::new(crate::devices::display::FRAMEBUFFER_SIZE).unwrap())
    }

    fn available(vm: &Arc<KvmVm>, address: u64) -> bool {
        FramebufferMemory::new(vm, backing())
            .unwrap()
            .set_address(Some(address))
            .is_ok()
    }

    #[test]
    fn slot_move_disable_failure_and_drop_are_atomic() {
        let vm = Arc::new(setup_vm_with_memory(1 << 20));
        let mut slot = FramebufferMemory::new(&vm, backing()).unwrap();
        slot.set_address(Some(0xc100_0000)).unwrap();
        assert!(!available(&vm, 0xc100_0000));
        slot.set_address(Some(0xc200_0000)).unwrap();
        assert!(available(&vm, 0xc100_0000));
        assert!(!available(&vm, 0xc200_0000));
        slot.set_address(Some(0)).unwrap_err();
        assert!(!available(&vm, 0xc200_0000));
        slot.set_address(None).unwrap();
        assert!(available(&vm, 0xc200_0000));
        slot.set_address(Some(0xc100_0000)).unwrap();
        drop(slot);
        assert!(available(&vm, 0xc100_0000));
    }

    #[test]
    fn backing_released_after_normal_slot_removal() {
        let vm = Arc::new(setup_vm_with_memory(1 << 20));
        let memory = backing();
        let weak = Arc::downgrade(&memory);
        let mut slot = FramebufferMemory::new(&vm, memory).unwrap();
        slot.set_address(Some(0xc100_0000)).unwrap();
        assert!(weak.upgrade().is_some());
        drop(slot);
        assert!(weak.upgrade().is_none());
        assert!(available(&vm, 0xc100_0000));
    }

    #[test]
    fn failed_slot_removal_retains_backing_until_vm_closes() {
        let vm = setup_vm_with_memory(1 << 20);
        #[cfg(target_arch = "aarch64")]
        let vm = {
            let mut vm = vm;
            vm.setup_irqchip(1).unwrap();
            vm
        };
        let vm = Arc::new(vm);
        let memory = backing();
        let weak = Arc::downgrade(&memory);
        let mut slot = FramebufferMemory::new(&vm, memory).unwrap();
        slot.set_address(Some(0xc100_0000)).unwrap();
        // Inject an ioctl failure while the real slot still points at the backing.
        slot.slot = u32::MAX;
        drop(slot);
        assert!(weak.upgrade().is_some());
        drop(vm);
        assert!(weak.upgrade().is_none());
    }
}
