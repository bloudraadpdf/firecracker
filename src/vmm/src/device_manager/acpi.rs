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

// Although both VMGenID and VMClock devices are always present, they should be instantiated when
// they are attached to preserve the existing ordering of GSI allocation.
#[derive(Debug, Default)]
pub struct ACPIDeviceManager {
    /// VMGenID device
    vmgenid: Option<VmGenId>,
    /// VMclock device
    vmclock: Option<VmClock>,
}

impl ACPIDeviceManager {
    /// Create a new ACPIDeviceManager object
    pub fn new(vmgenid: VmGenId, vmclock: VmClock) -> Self {
        ACPIDeviceManager {
            vmgenid: Some(vmgenid),
            vmclock: Some(vmclock),
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

    pub fn vmclock(&self) -> &VmClock {
        self.vmclock.as_ref().expect("Missing VMClock device")
    }

    pub fn activate_vmgenid(&self, vm: &KvmVm) -> Result<(), ACPIDeviceError> {
        if let Some((event, gsi)) = self.vmgenid().interrupt() {
            vm.register_irq(event, gsi)?;
        }
        self.vmgenid().activate(vm.guest_memory())?;
        Ok(())
    }

    pub fn activate_vmclock(&self, vm: &KvmVm) -> Result<(), ACPIDeviceError> {
        vm.register_irq(&self.vmclock().interrupt_evt, self.vmclock().gsi)?;
        self.vmclock().activate(vm.guest_memory())?;
        Ok(())
    }

    pub fn replay_gsi_allocations(&self, vm: &KvmVm) -> Result<(), ACPIDeviceError> {
        let mut resource_allocator = vm.resource_allocator();
        if let Some((_, gsi)) = self.vmgenid().interrupt() {
            resource_allocator
                .gsi_legacy_allocator
                .allocate_id_at(gsi)?;
        }
        resource_allocator
            .gsi_legacy_allocator
            .allocate_id_at(self.vmclock().gsi)?;
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
        self.vmclock
            .as_mut()
            .expect("Missing VMClock device")
            .do_post_restore(mem, notification)?;
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
        self.vmclock().append_aml_bytes_in_scope(scope, v)?;

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
        aml::Device::new(
            scope.device_path("GED_")?,
            vec![
                &aml::Name::new("_HID".try_into()?, &"ACPI0013")?,
                &aml::Name::new(
                    "_CRS".try_into()?,
                    &aml::ResourceTemplate::new(vec![&aml::Interrupt::new(
                        true, true, false, false, gsi,
                    )]),
                )?,
                &aml::Method::new(
                    "_EVT".try_into()?,
                    1,
                    true,
                    vec![&aml::Notify::new(
                        &aml::Path::new("\\_SB_.VGEN")?,
                        &0x80usize,
                    )],
                ),
            ],
        )
        .append_aml_bytes(v)
    }
}

#[cfg(test)]
#[cfg(all(target_arch = "x86_64", feature = "windows"))]
mod tests {
    use std::io::Write;
    use std::process::Command;

    use acpi_tables::{Aml, Sdt};
    use vm_memory::{Bytes, GuestAddress};

    #[test]
    fn pc_gpe_executes_generation_notify() {
        let manager = crate::device_manager::tests::default_device_manager_with_pc();
        let mut aml = Vec::new();
        manager.acpi_devices.append_aml_bytes(&mut aml).unwrap();
        let mut table = acpi_tables::Dsdt::new(*b"FC    ", *b"GPE TEST", 1, aml);
        let memory =
            vm_memory::GuestMemoryMmap::<()>::from_ranges(&[(GuestAddress(0), 0x10000)]).unwrap();
        table.write_to_guest(&memory, GuestAddress(0)).unwrap();
        let mut bytes = vec![0; table.len()];
        memory.read_slice(&mut bytes, GuestAddress(0)).unwrap();
        let file = vmm_sys_util::tempfile::TempFile::new_in(std::path::Path::new(".")).unwrap();
        file.as_file().write_all(&bytes).unwrap();
        let output = Command::new("acpiexec")
            .args(["-to", "20", "-b", "execute \\_GPE._E00"])
            .arg(file.as_path())
            .output()
            .unwrap();
        assert!(output.status.success());
        let output = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.contains("AE_"), "{output}");
        assert!(
            output
                .lines()
                .any(|line| line.contains("Received a Device Notify on [VGEN]")
                    && line.contains("Value 0x80")),
            "{output}"
        );
    }
}
