// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Weak};

use event_manager::{EventOps, Events, MutEventSubscriber};
use vmm_sys_util::epoll::EventSet;
use vmm_sys_util::eventfd::EventFd;

use crate::logger::error;
use crate::vstate::interrupts::InterruptError;
use crate::vstate::vm::KvmVm;

pub(crate) const PCI_INTX_GSI: u32 = 16;

#[derive(Debug)]
struct Source {
    status: Arc<AtomicU32>,
    masked: bool,
}

/// A wired-OR PCI interrupt line with one ISR per device.
#[derive(Debug)]
pub(crate) struct PciIntxGroup {
    vm: Weak<KvmVm>,
    irq: EventFd,
    resample: EventFd,
    sources: HashMap<u8, Source>,
    asserted: bool,
}

impl PciIntxGroup {
    #[cfg(target_arch = "x86_64")]
    pub(crate) fn new(vm: Arc<KvmVm>) -> Result<Self, InterruptError> {
        let irq = EventFd::new(libc::EFD_NONBLOCK)?;
        let resample = EventFd::new(libc::EFD_NONBLOCK)?;
        vm.resource_allocator()
            .gsi_legacy_allocator
            .allocate_id_at(PCI_INTX_GSI)?;
        if let Err(err) = vm.register_level_irq(&irq, &resample, PCI_INTX_GSI) {
            vm.resource_allocator()
                .gsi_legacy_allocator
                .free_id(PCI_INTX_GSI)?;
            return Err(err.into());
        }
        Ok(Self {
            vm: Arc::downgrade(&vm),
            irq,
            resample,
            sources: HashMap::new(),
            asserted: false,
        })
    }

    pub(crate) fn bind(
        &mut self,
        slot: u8,
        status: Arc<AtomicU32>,
        masked: bool,
    ) -> Result<(), InterruptError> {
        if slot == 0 || slot >= 32 || self.sources.contains_key(&slot) {
            return Err(InterruptError::InvalidVectorIndex(usize::from(slot)));
        }
        self.sources.insert(slot, Source { status, masked });
        if let Err(error) = self.update() {
            self.sources.remove(&slot);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn unbind(&mut self, slot: u8) -> Result<(), InterruptError> {
        self.sources.remove(&slot);
        self.update()
    }

    pub(crate) fn raise(&mut self, slot: u8, bits: u32) -> Result<(), InterruptError> {
        let source = self
            .sources
            .get(&slot)
            .ok_or_else(|| InterruptError::InvalidVectorIndex(usize::from(slot)))?;
        source.status.fetch_or(bits, Ordering::AcqRel);
        self.update()
    }

    pub(crate) fn clear(&mut self, slot: u8) -> Result<u32, InterruptError> {
        let source = self
            .sources
            .get(&slot)
            .ok_or_else(|| InterruptError::InvalidVectorIndex(usize::from(slot)))?;
        let status = source.status.swap(0, Ordering::AcqRel);
        self.update()?;
        Ok(status)
    }

    pub(crate) fn mask(&mut self, slot: u8, masked: bool) -> Result<(), InterruptError> {
        let source = self
            .sources
            .get_mut(&slot)
            .ok_or_else(|| InterruptError::InvalidVectorIndex(usize::from(slot)))?;
        source.masked = masked;
        self.update()
    }

    fn update(&mut self) -> Result<(), InterruptError> {
        let pending = self
            .sources
            .values()
            .any(|source| !source.masked && source.status.load(Ordering::Acquire) != 0);
        if pending && !self.asserted {
            self.irq.write(1)?;
        }
        self.asserted = pending;
        Ok(())
    }
}

impl MutEventSubscriber for PciIntxGroup {
    fn init(&mut self, ops: &mut EventOps) {
        if let Err(err) = ops.add(Events::new(&self.resample, EventSet::IN)) {
            error!("Failed to register PCI INTx resampling: {err}");
        }
    }

    fn process(&mut self, event: Events, _ops: &mut EventOps) {
        if !event.event_set().contains(EventSet::IN) {
            return;
        }
        if let Err(err) = self.resample.read() {
            error!("Failed to read PCI INTx resampling: {err}");
            return;
        }
        self.asserted = false;
        if let Err(err) = self.update() {
            error!("Failed to reassert PCI INTx: {err}");
        }
    }
}

impl Drop for PciIntxGroup {
    fn drop(&mut self) {
        let Some(vm) = self.vm.upgrade() else {
            return;
        };
        if let Err(err) = vm.common.fd.unregister_irqfd(&self.irq, PCI_INTX_GSI) {
            error!("Failed to unregister PCI INTx: {err}");
        }
        if let Err(err) = vm
            .resource_allocator()
            .gsi_legacy_allocator
            .free_id(PCI_INTX_GSI)
        {
            error!("Failed to free PCI INTx GSI: {err}");
        }
    }
}

#[cfg(test)]
#[cfg(target_arch = "x86_64")]
mod tests {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use event_manager::{EventManager, SubscriberOps};
    use kvm_bindings::{KVM_IRQCHIP_IOAPIC, kvm_irqchip, kvm_regs, kvm_segment};
    use kvm_ioctls::{VcpuExit, VcpuFd};
    use vm_memory::{Bytes, GuestAddress};

    use super::*;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    #[test]
    fn group_does_not_keep_vm_alive() {
        let mut vm = setup_vm_with_memory(1 << 20);
        vm.setup_irqchip(true).unwrap();
        let vm = Arc::new(vm);
        let weak = Arc::downgrade(&vm);
        let group = PciIntxGroup::new(vm.clone()).unwrap();
        assert_eq!(Arc::strong_count(&vm), 1);
        drop(vm);
        assert!(weak.upgrade().is_none());
        drop(group);
    }

    #[test]
    fn restored_pending_source_and_gsi_cleanup() {
        let mut vm = setup_vm_with_memory(1 << 20);
        vm.setup_irqchip(true).unwrap();
        let vm = Arc::new(vm);
        let mut group = PciIntxGroup::new(vm.clone()).unwrap();
        let restored_status = Arc::new(AtomicU32::new(1));
        group.bind(3, restored_status.clone(), true).unwrap();
        assert!(!group.asserted);
        group.mask(3, false).unwrap();
        assert!(group.asserted);
        assert_eq!(group.clear(3).unwrap(), 1);
        assert!(!group.asserted);
        group.unbind(3).unwrap();
        drop(group);
        vm.resource_allocator()
            .gsi_legacy_allocator
            .allocate_id_at(PCI_INTX_GSI)
            .unwrap();
        vm.resource_allocator()
            .gsi_legacy_allocator
            .free_id(PCI_INTX_GSI)
            .unwrap();
    }

    fn run_to_port(vcpu: &mut VcpuFd, expected: u16) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match vcpu.run().unwrap() {
                VcpuExit::IoOut(port, _) if port == expected => return,
                VcpuExit::IoOut(0x82, _) => (),
                exit => panic!("Unexpected KVM exit: {exit:?}"),
            }
            assert!(Instant::now() < deadline, "Interrupt delivery timed out");
            std::thread::yield_now();
        }
    }

    #[test]
    fn shared_line_reasserts_after_real_guest_eoi() {
        let mut vm = setup_vm_with_memory(1 << 20);
        vm.setup_irqchip(true).unwrap();
        let vm = Arc::new(vm);
        let group = Arc::new(Mutex::new(PciIntxGroup::new(vm.clone()).unwrap()));
        vm.set_gsi_routes().unwrap();
        let mut vcpu = vm.common.fd.create_vcpu(0).unwrap();
        let mem = vm.guest_memory();
        for (address, value) in [
            (0x500, 0u64),
            (0x508, 0x00cf_9b00_0000_ffff),
            (0x510, 0x00cf_9300_0000_ffff),
        ] {
            mem.write_obj(value, GuestAddress(address)).unwrap();
        }
        mem.write_obj(0x0000_8e00_0008_2000u64, GuestAddress(0x1000 + 0x51 * 8))
            .unwrap();
        // Interrupt entry, EOI, then IRET. Each OUT returns control to the test.
        mem.write_slice(
            &[
                0xb0, 0x51, 0xe6, 0x80, 0xc7, 0x05, 0xb0, 0x00, 0xe0, 0xfe, 0, 0, 0, 0, 0xe6, 0x81,
                0xcf,
            ],
            GuestAddress(0x2000),
        )
        .unwrap();
        mem.write_slice(&[0xe6, 0x82, 0xeb, 0xfc], GuestAddress(0x3000))
            .unwrap();
        let mut sregs = vcpu.get_sregs().unwrap();
        sregs.cs = kvm_segment {
            base: 0,
            limit: u32::MAX,
            selector: 8,
            type_: 11,
            present: 1,
            s: 1,
            g: 1,
            db: 1,
            ..Default::default()
        };
        let data = kvm_segment {
            selector: 16,
            type_: 3,
            ..sregs.cs
        };
        sregs.ds = data;
        sregs.es = data;
        sregs.ss = data;
        sregs.gdt.base = 0x500;
        sregs.gdt.limit = 23;
        sregs.idt.base = 0x1000;
        sregs.idt.limit = 0x7ff;
        sregs.cr0 = 0x11;
        sregs.cr4 = 0;
        sregs.efer = 0;
        vcpu.set_sregs(&sregs).unwrap();
        vcpu.set_regs(&kvm_regs {
            rip: 0x3000,
            rsp: 0x8000,
            rflags: 0x202,
            ..Default::default()
        })
        .unwrap();
        let mut lapic = vcpu.get_lapic().unwrap();
        for (offset, value) in [(0xf0usize, 0x1ffu32), (0x80, 0)] {
            for (byte, target) in value
                .to_le_bytes()
                .into_iter()
                .zip(&mut lapic.regs[offset..offset + 4])
            {
                *target = i8::from_ne_bytes([byte]);
            }
        }
        vcpu.set_lapic(&lapic).unwrap();
        let mut ioapic = kvm_irqchip {
            chip_id: KVM_IRQCHIP_IOAPIC,
            ..Default::default()
        };
        vm.common.fd.get_irqchip(&mut ioapic).unwrap();
        // SAFETY: chip_id selects the IOAPIC member.
        unsafe {
            ioapic.chip.ioapic.redirtbl[PCI_INTX_GSI as usize].bits = 0x8051;
        }
        vm.common.fd.set_irqchip(&ioapic).unwrap();

        let first = Arc::new(AtomicU32::new(0));
        let second = Arc::new(AtomicU32::new(0));
        group.lock().unwrap().bind(1, first.clone(), false).unwrap();
        group
            .lock()
            .unwrap()
            .bind(2, second.clone(), false)
            .unwrap();
        let mut events = EventManager::new().unwrap();
        events.add_subscriber(group.clone());
        group.lock().unwrap().raise(1, 1).unwrap();
        group.lock().unwrap().raise(2, 2).unwrap();
        run_to_port(&mut vcpu, 0x80);
        assert_eq!(group.lock().unwrap().clear(1).unwrap(), 1);
        assert_eq!(second.load(Ordering::Acquire), 2);
        run_to_port(&mut vcpu, 0x81);
        assert_eq!(events.run_with_timeout(100).unwrap(), 1);
        run_to_port(&mut vcpu, 0x80);
        group.lock().unwrap().mask(2, true).unwrap();
        run_to_port(&mut vcpu, 0x81);
        assert_eq!(events.run_with_timeout(100).unwrap(), 1);
        run_to_port(&mut vcpu, 0x82);
        assert!(!group.lock().unwrap().asserted);
        assert_eq!(second.load(Ordering::Acquire), 2);
        group.lock().unwrap().mask(2, false).unwrap();
        run_to_port(&mut vcpu, 0x80);
        assert_eq!(group.lock().unwrap().clear(2).unwrap(), 2);
        run_to_port(&mut vcpu, 0x81);
        assert_eq!(events.run_with_timeout(100).unwrap(), 1);
        run_to_port(&mut vcpu, 0x82);
        assert_eq!(first.load(Ordering::Acquire), 0);
        assert!(!group.lock().unwrap().asserted);
    }
}
