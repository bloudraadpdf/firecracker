// Copyright 2024 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

#[cfg(all(target_arch = "x86_64", feature = "windows"))]
use std::sync::{Arc, Mutex};

use acpi_tables::{Aml, aml};
use aws_lc_rs::rand;
use serde::{Deserialize, Serialize};
use vm_memory::{GuestAddress, GuestMemoryError};
use vm_superio::Trigger;
use vmm_sys_util::eventfd::EventFd;

use super::super::legacy::EventFdTrigger;
use crate::logger::debug;
use crate::snapshot::Persist;
use crate::vstate::memory::{Bytes, GuestMemoryExtension, GuestMemoryMmap};
use crate::vstate::resources::ResourceAllocator;

/// Bytes of memory we allocate for VMGenID device
pub const VMGENID_MEM_SIZE: u64 = 16;

/// Selects whether restoring a VMGenID device raises its guest notification.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum VmGenIdRestoreNotification {
    /// Raise the VMGenID interrupt after writing the new generation ID.
    #[default]
    Interrupt,
    /// Write the new generation ID without raising its interrupt.
    Disabled,
}

#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum VmGenIdError {
    /// Could not create EventFd: {0}
    CreateEventFd(std::io::Error),
    /// Could not allocate GSI: {0}
    AllocateGsi(vm_allocator::Error),
    /// Could not allocate guest memory: {0}
    AllocateMemory(vm_allocator::Error),
    /// Could not write generation ID to guest memory: {0}
    WriteGuestMemory(#[from] GuestMemoryError),
    /// Could not notify guest: {0}
    NotifyGuest(std::io::Error),
    /// Invalid generation-ID interrupt routing: {0}
    InvalidInterrupt(u32),
}

#[derive(Debug)]
enum VmGenIdInterrupt {
    Ged {
        event: EventFdTrigger,
        gsi: u32,
    },
    #[cfg(all(target_arch = "x86_64", feature = "windows"))]
    Gpe(Arc<Mutex<crate::devices::legacy::acpi::AcpiPower>>),
}

/// Existing platform interrupt owner used when reconstructing VMGenID.
#[derive(Debug, Default)]
pub struct VmGenIdRestoreArgs {
    #[cfg(all(target_arch = "x86_64", feature = "windows"))]
    /// Conventional PC event hardware; absent for the dedicated GED route.
    pub power: Option<Arc<Mutex<crate::devices::legacy::acpi::AcpiPower>>>,
}

/// Virtual Machine Generation ID device
///
/// VMGenID is an emulated device which exposes to the guest a 128-bit cryptographically random
/// integer value which will be different every time the virtual machine executes from a different
/// configuration file. In Firecracker terms this translates to a different value every time a new
/// microVM is created, either from scratch or restored from a snapshot.
///
/// The device specification can be found here: https://go.microsoft.com/fwlink/?LinkId=260709
#[derive(Debug)]
pub struct VmGenId {
    /// Current generation ID of guest VM
    pub gen_id: u128,
    interrupt: VmGenIdInterrupt,
    /// Guest physical address where VMGenID data lives.
    pub guest_address: GuestAddress,
}

impl VmGenId {
    /// Create a new Vm Generation Id device using an address in the guest for writing the
    /// generation ID and a GSI for sending device notifications.
    pub fn from_parts(guest_address: GuestAddress, gsi: u32) -> Result<Self, VmGenIdError> {
        debug!(
            "vmgenid: building VMGenID device. Address: {:#010x}. IRQ: {}",
            guest_address.0, gsi
        );
        let interrupt_evt = EventFdTrigger::new(
            EventFd::new(libc::EFD_NONBLOCK).map_err(VmGenIdError::CreateEventFd)?,
        );
        let gen_id = Self::make_genid();

        Ok(Self {
            gen_id,
            interrupt: VmGenIdInterrupt::Ged {
                event: interrupt_evt,
                gsi,
            },
            guest_address,
        })
    }

    /// Create a new VMGenID device
    ///
    /// Allocate memory and a GSI for sending notifications and build the device
    pub fn new(resource_allocator: &mut ResourceAllocator) -> Result<Self, VmGenIdError> {
        let gsi = resource_allocator
            .allocate_gsi_legacy(1)
            .map_err(VmGenIdError::AllocateGsi)?[0];
        Self::from_parts(Self::allocate_address(resource_allocator)?, gsi)
    }

    fn allocate_address(
        resource_allocator: &mut ResourceAllocator,
    ) -> Result<GuestAddress, VmGenIdError> {
        resource_allocator
            .system_memory
            .allocate(VMGENID_MEM_SIZE, 8, vm_allocator::AllocPolicy::LastMatch)
            .map(|range| GuestAddress(range.start()))
            .map_err(VmGenIdError::AllocateMemory)
    }

    #[cfg(all(target_arch = "x86_64", feature = "windows"))]
    pub(crate) fn new_with_gpe(
        resource_allocator: &mut ResourceAllocator,
        power: Arc<Mutex<crate::devices::legacy::acpi::AcpiPower>>,
    ) -> Result<Self, VmGenIdError> {
        Ok(Self {
            gen_id: Self::make_genid(),
            interrupt: VmGenIdInterrupt::Gpe(power),
            guest_address: Self::allocate_address(resource_allocator)?,
        })
    }

    pub(crate) fn interrupt(&self) -> Option<(&EventFdTrigger, u32)> {
        match &self.interrupt {
            VmGenIdInterrupt::Ged { event, gsi } => Some((event, *gsi)),
            #[cfg(all(target_arch = "x86_64", feature = "windows"))]
            VmGenIdInterrupt::Gpe(_) => None,
        }
    }

    pub(crate) fn uses_gpe(&self) -> bool {
        self.interrupt().is_none()
    }

    pub(crate) fn gsi(&self) -> u32 {
        match &self.interrupt {
            VmGenIdInterrupt::Ged { gsi, .. } => *gsi,
            #[cfg(all(target_arch = "x86_64", feature = "windows"))]
            VmGenIdInterrupt::Gpe(_) => crate::devices::legacy::acpi::SCI_GSI,
        }
    }

    // Create a 16-bytes random number
    fn make_genid() -> u128 {
        let mut gen_id_bytes = [0u8; 16];
        rand::fill(&mut gen_id_bytes).expect("vmgenid: could not create new generation ID");
        u128::from_le_bytes(gen_id_bytes)
    }

    /// Notify guest after snapshot restore
    ///
    /// This will only have effect if we have updated the generation ID in guest memory, i.e. when
    /// re-creating the device after snapshot resumption.
    pub fn do_post_restore(
        &self,
        notification: VmGenIdRestoreNotification,
    ) -> Result<(), VmGenIdError> {
        if notification == VmGenIdRestoreNotification::Interrupt {
            match &self.interrupt {
                VmGenIdInterrupt::Ged { event, .. } => event.trigger(),
                #[cfg(all(target_arch = "x86_64", feature = "windows"))]
                VmGenIdInterrupt::Gpe(power) => {
                    power.lock().expect("Poisoned lock").notify_vmgenid()
                }
            }
            .map_err(VmGenIdError::NotifyGuest)?;
            debug!("vmgenid: notifying guest about new generation ID");
        }
        Ok(())
    }

    /// Attach the [`VmGenId`] device
    pub fn activate(&self, mem: &GuestMemoryMmap) -> Result<(), VmGenIdError> {
        debug!(
            "vmgenid: writing new generation ID to guest: {:#034x}",
            self.gen_id
        );
        mem.check_range_plugged(self.guest_address, self.gen_id.to_le_bytes().len())?;
        mem.write_slice(&self.gen_id.to_le_bytes(), self.guest_address)
            .map_err(VmGenIdError::WriteGuestMemory)?;

        Ok(())
    }
}

/// Logic to save/restore the state of a VMGenID device

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct VMGenIDState {
    /// GSI used for VMGenID device
    pub gsi: u32,
    /// memory address of generation ID
    pub addr: u64,
}

impl VMGenIDState {
    #[cfg(target_arch = "x86_64")]
    pub(crate) fn validate_pc_route(&self, pc_enabled: bool) -> Result<(), VmGenIdError> {
        let valid = if pc_enabled {
            self.gsi == crate::devices::legacy::acpi::SCI_GSI
        } else {
            (crate::arch::GSI_LEGACY_START..=crate::arch::GSI_LEGACY_END).contains(&self.gsi)
        };
        if !valid {
            return Err(VmGenIdError::InvalidInterrupt(self.gsi));
        }
        Ok(())
    }
}

impl<'a> Persist<'a> for VmGenId {
    type State = VMGenIDState;
    type ConstructorArgs = VmGenIdRestoreArgs;
    type Error = VmGenIdError;

    fn save(&self) -> Self::State {
        VMGenIDState {
            gsi: self.gsi(),
            addr: self.guest_address.0,
        }
    }

    fn restore(_args: Self::ConstructorArgs, state: &Self::State) -> Result<Self, Self::Error> {
        #[cfg(all(target_arch = "x86_64", feature = "windows"))]
        if let Some(power) = _args.power {
            state.validate_pc_route(true)?;
            return Ok(Self {
                gen_id: Self::make_genid(),
                interrupt: VmGenIdInterrupt::Gpe(power),
                guest_address: GuestAddress(state.addr),
            });
        }
        #[cfg(target_arch = "x86_64")]
        state.validate_pc_route(false)?;
        Self::from_parts(GuestAddress(state.addr), state.gsi)
    }
}

impl Aml for VmGenId {
    fn append_aml_bytes(&self, v: &mut Vec<u8>) -> Result<(), aml::AmlError> {
        #[allow(clippy::cast_possible_truncation)]
        let addr_low = self.guest_address.0 as u32;
        let addr_high = (self.guest_address.0 >> 32) as u32;
        aml::Device::new(
            "_SB_.VGEN".try_into()?,
            vec![
                &aml::Name::new("_HID".try_into()?, &"VMGENCTR")?,
                &aml::Name::new("_CID".try_into()?, &"VM_Gen_Counter")?,
                &aml::Name::new("_DDN".try_into()?, &"VM_Gen_Counter")?,
                &aml::Name::new(
                    "ADDR".try_into()?,
                    &aml::Package::new(vec![&addr_low, &addr_high]),
                )?,
            ],
        )
        .append_aml_bytes(v)
    }
}

#[cfg(test)]
mod tests {
    use vm_memory::{Bytes, GuestAddress};

    use crate::arch;
    use crate::devices::acpi::vmgenid::{VMGENID_MEM_SIZE, VmGenId, VmGenIdRestoreNotification};
    use crate::snapshot::Persist;
    use crate::test_utils::single_region_mem;
    use crate::utils::u64_to_usize;
    use crate::vstate::resources::ResourceAllocator;

    const VMGENID_TEST_GUEST_ADDR: GuestAddress =
        GuestAddress(arch::SYSTEM_MEM_START + arch::SYSTEM_MEM_SIZE - VMGENID_MEM_SIZE);

    fn default_vmgenid() -> VmGenId {
        VmGenId::new(&mut ResourceAllocator::new()).unwrap()
    }

    #[test]
    #[cfg(all(target_arch = "x86_64", feature = "windows"))]
    fn pc_restore_writes_id_before_signalling_shared_sci() {
        use crate::devices::legacy::acpi::{AcpiPower, POWER_PORT, SCI_GSI};
        use crate::vstate::bus::BusDevice;
        use vmm_sys_util::eventfd::EventFd;
        let power = std::sync::Arc::new(std::sync::Mutex::new(
            AcpiPower::new(vmm_sys_util::eventfd::EventFd::new(libc::EFD_NONBLOCK).unwrap())
                .unwrap(),
        ));
        let mut allocator = ResourceAllocator::new();
        let device = VmGenId::new_with_gpe(&mut allocator, power.clone()).unwrap();
        assert!(device.interrupt().is_none());
        assert_eq!(device.save().gsi, SCI_GSI);
        assert_eq!(
            allocator.allocate_gsi_legacy(1).unwrap(),
            [crate::arch::GSI_LEGACY_START]
        );
        let mem = single_region_mem(u64_to_usize(arch::SYSTEM_MEM_START + arch::SYSTEM_MEM_SIZE));
        device.activate(&mem).unwrap();
        let saved = device.save();
        power.lock().unwrap().write(POWER_PORT, 13, &[1]);
        let restored = VmGenId::restore(
            super::VmGenIdRestoreArgs {
                power: Some(power.clone()),
            },
            &saved,
        )
        .unwrap();
        restored.activate(&mem).unwrap();
        restored
            .do_post_restore(VmGenIdRestoreNotification::Disabled)
            .unwrap();
        assert_eq!(
            power.lock().unwrap().sci_evt.read().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let value: u128 = mem.read_obj(restored.guest_address).unwrap();
        assert_eq!(value, restored.gen_id);
        assert_ne!(value, device.gen_id);
        restored
            .do_post_restore(VmGenIdRestoreNotification::Interrupt)
            .unwrap();
        assert_eq!(power.lock().unwrap().sci_evt.read().unwrap(), 1);
        let mut registers = [0; 2];
        power.lock().unwrap().read(POWER_PORT, 12, &mut registers);
        assert_eq!(registers, [1, 1]);
        let saved_power = power.lock().unwrap().save();
        let mut restored_power = AcpiPower::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap();
        restored_power.restore(&saved_power).unwrap();
        assert_eq!(restored_power.sci_evt.read().unwrap(), 1);
        let restored_power = std::sync::Arc::new(std::sync::Mutex::new(restored_power));
        let disabled = VmGenId::restore(
            super::VmGenIdRestoreArgs {
                power: Some(restored_power.clone()),
            },
            &saved,
        )
        .unwrap();
        disabled.activate(&mem).unwrap();
        disabled
            .do_post_restore(VmGenIdRestoreNotification::Disabled)
            .unwrap();
        let mut power_state = restored_power.lock().unwrap();
        power_state.read(POWER_PORT, 12, &mut registers);
        assert_eq!(registers, [1, 1]);
        assert_eq!(
            power_state.sci_evt.read().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        drop(power_state);
        assert!(matches!(
            VmGenId::restore(Default::default(), &saved),
            Err(super::VmGenIdError::InvalidInterrupt(gsi)) if gsi == SCI_GSI
        ));
        assert!(matches!(
            VmGenId::restore(
                super::VmGenIdRestoreArgs { power: Some(power) },
                &super::VMGenIDState {
                    gsi: crate::arch::GSI_LEGACY_START,
                    ..saved
                }
            ),
            Err(super::VmGenIdError::InvalidInterrupt(gsi)) if gsi == crate::arch::GSI_LEGACY_START
        ));
    }

    #[test]
    fn test_device_restore_without_notification() {
        let vmgenid = default_vmgenid();
        let mem = single_region_mem(
            u64_to_usize(arch::SYSTEM_MEM_START) + u64_to_usize(arch::SYSTEM_MEM_SIZE),
        );
        vmgenid.activate(&mem).unwrap();

        let state = vmgenid.save();
        let restored = VmGenId::restore(Default::default(), &state).unwrap();
        restored.activate(&mem).unwrap();
        restored
            .do_post_restore(VmGenIdRestoreNotification::Disabled)
            .unwrap();

        assert_eq!(
            restored
                .interrupt()
                .unwrap()
                .0
                .read()
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EAGAIN)
        );
        let guest_data: u128 = mem.read_obj(VMGENID_TEST_GUEST_ADDR).unwrap();
        assert_eq!(guest_data, restored.gen_id);
        assert_ne!(guest_data, vmgenid.gen_id);
    }
}
