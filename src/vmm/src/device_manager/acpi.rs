// Copyright 2024 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use acpi_tables::{Aml, aml};

use crate::devices::acpi::vmclock::{VmClock, VmClockError, VmClockRestoreNotification};
use crate::devices::acpi::vmgenid::{VmGenId, VmGenIdError, VmGenIdRestoreNotification};
use crate::vstate::memory::GuestMemoryMmap;
use crate::vstate::vm::KvmVm;

#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum ACPIDeviceError {
    /// VMGenID: {0}
    VmGenId(#[from] VmGenIdError),
    /// VMClock: {0}
    VmClock(#[from] VmClockError),
    /// Could not register IRQ with KVM: {0}
    RegisterIrq(#[from] kvm_ioctls::Error),
    /// Resource allocator error: {0}
    ResourceAllocator(#[from] vm_allocator::Error),
}

// Instantiate devices when attached to preserve GSI allocation order.
#[derive(Debug, Default)]
pub struct ACPIDeviceManager {
    /// VMGenID device
    vmgenid: Option<VmGenId>,
    /// VMclock device
    vmclock: Option<VmClock>,
}

impl ACPIDeviceManager {
    /// Create a new ACPIDeviceManager object
    pub fn new(vmgenid: VmGenId, vmclock: Option<VmClock>) -> Self {
        ACPIDeviceManager {
            vmgenid: Some(vmgenid),
            vmclock,
        }
    }

    pub fn attach_vmgenid(
        &mut self,
        vm: &KvmVm,
        #[cfg(all(target_arch = "x86_64", feature = "windows"))] power: Option<
            std::sync::Arc<std::sync::Mutex<crate::devices::legacy::acpi::AcpiPower>>,
        >,
    ) -> Result<(), ACPIDeviceError> {
        #[cfg(all(target_arch = "x86_64", feature = "windows"))]
        if let Some(power) = power {
            self.vmgenid = Some(VmGenId::new_with_gpe(&mut vm.resource_allocator(), power)?);
            return Ok(());
        }
        self.vmgenid = Some(VmGenId::new(&mut vm.resource_allocator())?);
        Ok(())
    }

    pub fn attach_vmclock(&mut self, vm: &KvmVm) -> Result<(), ACPIDeviceError> {
        self.vmclock = Some(VmClock::new(&mut vm.resource_allocator())?);
        Ok(())
    }

    pub fn vmgenid(&self) -> &VmGenId {
        self.vmgenid.as_ref().expect("Missing VMGenID device")
    }

    pub fn vmclock(&self) -> Option<&VmClock> {
        self.vmclock.as_ref()
    }

    pub fn activate_vmgenid(&self, vm: &KvmVm) -> Result<(), ACPIDeviceError> {
        if let Some((event, gsi)) = self.vmgenid().interrupt() {
            vm.register_irq(event, gsi)?;
        }
        self.vmgenid().activate(vm.guest_memory())?;
        Ok(())
    }

    pub fn activate_vmclock(&self, vm: &KvmVm) -> Result<(), ACPIDeviceError> {
        if let Some(vmclock) = self.vmclock() {
            vm.register_irq(&vmclock.interrupt_evt, vmclock.gsi)?;
            vmclock.activate(vm.guest_memory())?;
        }
        Ok(())
    }

    pub fn replay_gsi_allocations(&self, vm: &KvmVm) -> Result<(), ACPIDeviceError> {
        let mut resource_allocator = vm.resource_allocator();
        if let Some((_, gsi)) = self.vmgenid().interrupt() {
            resource_allocator
                .gsi_legacy_allocator
                .allocate_id_at(gsi)?;
        }
        if let Some(vmclock) = self.vmclock() {
            resource_allocator
                .gsi_legacy_allocator
                .allocate_id_at(vmclock.gsi)?;
        }
        Ok(())
    }

    pub fn do_post_restore_vmgenid(
        &self,
        notification: VmGenIdRestoreNotification,
    ) -> Result<(), ACPIDeviceError> {
        self.vmgenid().do_post_restore(notification)?;
        Ok(())
    }

    pub fn do_post_restore_vmclock(
        &mut self,
        mem: &GuestMemoryMmap,
        notification: VmClockRestoreNotification,
    ) -> Result<(), ACPIDeviceError> {
        if let Some(vmclock) = &mut self.vmclock {
            vmclock.do_post_restore(mem, notification)?;
        }
        Ok(())
    }
}

impl Aml for ACPIDeviceManager {
    fn append_aml_bytes(&self, v: &mut Vec<u8>) -> Result<(), aml::AmlError> {
        self.append_aml_bytes_in_scope(crate::acpi::DeviceScope::SystemBus, v)
    }
}

impl ACPIDeviceManager {
    pub(crate) fn append_aml_bytes_in_scope(
        &self,
        scope: crate::acpi::DeviceScope,
        v: &mut Vec<u8>,
    ) -> Result<(), aml::AmlError> {
        // AML for [`VmGenId`] device.
        self.vmgenid().append_aml_bytes(v)?;
        // AML for [`VmClock`] device.
        if let Some(vmclock) = self.vmclock() {
            vmclock.append_aml_bytes_in_scope(scope, v)?;
        }

        if self.vmgenid().uses_gpe() {
            return aml::Method::new(
                "\\_GPE._E00".try_into()?,
                0,
                true,
                vec![&aml::Notify::new(
                    &aml::Path::new("\\_SB_.VGEN")?,
                    &0x80usize,
                )],
            )
            .append_aml_bytes(v);
        }
        let (_, gsi) = self.vmgenid().interrupt().unwrap();
        crate::acpi::append_ged(
            scope.device_path("GED_")?,
            0,
            gsi,
            "\\_SB_.VGEN".try_into()?,
            v,
        )
    }
}

#[cfg(test)]
mod tests {
    use acpi_tables::Aml;

    fn aml_with_clock(mut manager: crate::device_manager::DeviceManager, present: bool) -> Vec<u8> {
        if !present {
            manager.acpi_devices.vmclock = None;
        }
        let mut aml = Vec::new();
        manager.acpi_devices.append_aml_bytes(&mut aml).unwrap();
        assert_eq!(aml.windows(8).any(|bytes| bytes == b"AMZNC10C"), present);
        assert_eq!(aml.windows(4).any(|bytes| bytes == b"GCLK"), present);
        aml
    }

    #[test]
    fn vmclock_aml_presence_preserves_genid_and_ged() {
        for present in [false, true] {
            let aml = aml_with_clock(
                crate::device_manager::tests::default_device_manager(),
                present,
            );
            assert!(aml.windows(4).any(|bytes| bytes == b"VGEN"));
            assert!(aml.windows(4).any(|bytes| bytes == b"GED_"));
        }
    }

    #[test]
    #[cfg(all(target_arch = "x86_64", feature = "windows"))]
    fn pc_gpe_executes_generation_notify() {
        for present in [false, true] {
            let aml = aml_with_clock(
                crate::device_manager::tests::default_device_manager_with_pc(),
                present,
            );
            let output = crate::test_utils::execute_aml(aml, "execute \\_GPE._E00");
            assert!(
                output
                    .lines()
                    .any(|line| line.contains("Received a Device Notify on [VGEN]")
                        && line.contains("Value 0x80")),
                "{output}"
            );
            if present {
                let aml = aml_with_clock(
                    crate::device_manager::tests::default_device_manager_with_pc(),
                    true,
                );
                let output = crate::test_utils::execute_aml(aml, "execute \\_SB_.GCLK._EVT 0");
                assert!(
                    output
                        .lines()
                        .any(|line| line.contains("Received a Device Notify on [VCLK]")
                            && line.contains("Value 0x80")),
                    "{output}"
                );
            }
        }
    }
}
