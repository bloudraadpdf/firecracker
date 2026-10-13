// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
#![cfg(target_arch = "x86_64")]

use std::sync::{Arc, Mutex};

use acpi_tables::aml::AmlError;
use acpi_tables::{Aml, aml};

use crate::devices::legacy::acpi::{AcpiPower, PORT_COUNT, POWER_PORT, SCI_GSI};
use crate::devices::legacy::cmos::{CMOS_PORT, RTC_GSI, Rtc};
use crate::devices::legacy::{I8042Device, SerialDevice};
use crate::vstate::bus::BusError;
use crate::vstate::vm::KvmVm;

/// Errors corresponding to the `PortIODeviceManager`.
#[derive(Debug, derive_more::From, thiserror::Error, displaydoc::Display)]
pub enum LegacyDeviceError {
    /// Failed to add legacy device to Bus: {0}
    BusError(BusError),
    /// Failed to create EventFd: {0}
    EventFd(std::io::Error),
}

/// Registers serial, i8042 and ACPI power/timer devices on the I/O bus.
#[derive(Debug)]
pub struct PortIODeviceManager {
    // BusDevice::Serial
    pub stdio_serial: Arc<Mutex<SerialDevice>>,
    // BusDevice::I8042Device
    pub i8042: Arc<Mutex<I8042Device>>,
    /// ACPI reset and shutdown registers.
    pub acpi_power: Arc<Mutex<AcpiPower>>,
    /// Firmware CMOS real-time clock.
    pub cmos: Arc<Mutex<Rtc>>,
}

impl PortIODeviceManager {
    /// Serial port 1
    const COM1_GSI: u32 = 4;
    /// x86 global system interrupt for keyboard port.
    /// See <https://en.wikipedia.org/wiki/Interrupt_request_(PC_architecture)>.
    const KBD_EVT_GSI: u32 = 1;
    /// Legacy serial port device addresses. See
    /// <https://tldp.org/HOWTO/Serial-HOWTO-10.html#ss10.1>.
    const SERIAL_PORT_ADDRESS: u64 = 0x3f8;
    /// Size of legacy serial ports.
    const SERIAL_PORT_SIZE: u64 = 0x8;
    /// i8042 keyboard data register address. See
    /// <https://elixir.bootlin.com/linux/latest/source/drivers/input/serio/i8042-io.h#L41>.
    const I8042_KDB_DATA_REGISTER_ADDRESS: u64 = 0x060;
    /// i8042 keyboard data register size.
    const I8042_KDB_DATA_REGISTER_SIZE: u64 = 0x5;

    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        use vmm_sys_util::eventfd::EventFd;

        use crate::devices::legacy::serial::{SerialOut, SerialOutInner};

        Self {
            stdio_serial: Arc::new(Mutex::new(
                SerialDevice::new(None, SerialOut::new(SerialOutInner::Sink, None), None).unwrap(),
            )),
            i8042: Arc::new(Mutex::new(
                I8042Device::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap(),
            )),
            acpi_power: Arc::new(Mutex::new(
                AcpiPower::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap(),
            )),
            cmos: Arc::new(Mutex::new(Rtc::new().unwrap())),
        }
    }

    /// Register supported legacy devices.
    pub fn register_devices(&mut self, vm: &KvmVm) -> Result<(), LegacyDeviceError> {
        let io_bus = &vm.pio_bus;
        io_bus.insert(
            self.acpi_power.clone(),
            POWER_PORT,
            PORT_COUNT.try_into().unwrap(),
        )?;
        let power = self.acpi_power.lock().expect("Poisoned lock");
        vm.register_level_irq(&power.sci_evt, &power.resample_evt, SCI_GSI)
            .map_err(|e| {
                LegacyDeviceError::EventFd(std::io::Error::from_raw_os_error(e.errno()))
            })?;
        io_bus.insert(self.cmos.clone(), CMOS_PORT, 2)?;
        vm.register_irq(&self.cmos.lock().expect("Poisoned lock").irq_evt, RTC_GSI)
            .map_err(|e| {
                LegacyDeviceError::EventFd(std::io::Error::from_raw_os_error(e.errno()))
            })?;
        io_bus.insert(
            self.stdio_serial.clone(),
            Self::SERIAL_PORT_ADDRESS,
            Self::SERIAL_PORT_SIZE,
        )?;
        io_bus.insert(
            self.i8042.clone(),
            Self::I8042_KDB_DATA_REGISTER_ADDRESS,
            Self::I8042_KDB_DATA_REGISTER_SIZE,
        )?;

        vm.register_irq(
            self.stdio_serial
                .lock()
                .expect("Poisoned lock")
                .serial
                .interrupt_evt(),
            Self::COM1_GSI,
        )
        .map_err(|e| LegacyDeviceError::EventFd(std::io::Error::from_raw_os_error(e.errno())))?;

        vm.register_irq(
            &self.i8042.lock().expect("Poisoned lock").kbd_interrupt_evt,
            Self::KBD_EVT_GSI,
        )
        .map_err(|e| LegacyDeviceError::EventFd(std::io::Error::from_raw_os_error(e.errno())))?;

        Ok(())
    }

    pub(crate) fn append_aml_bytes(
        bytes: &mut Vec<u8>,
        scope: crate::acpi::DeviceScope,
    ) -> Result<(), AmlError> {
        aml::Device::new(
            "_SB_.PRES".try_into()?,
            vec![
                &aml::Name::new("_HID".try_into()?, &aml::EisaName::new("PNP0C02")?)?,
                &aml::Name::new("_UID".try_into()?, &"POWER")?,
                &aml::Name::new(
                    "_CRS".try_into()?,
                    &aml::ResourceTemplate::new(vec![&aml::Io::new(
                        POWER_PORT.try_into().unwrap(),
                        POWER_PORT.try_into().unwrap(),
                        1,
                        PORT_COUNT.try_into().unwrap(),
                    )]),
                )?,
            ],
        )
        .append_aml_bytes(bytes)?;
        aml::Device::new(
            scope.device_path("RTC0")?,
            vec![
                &aml::Name::new("_HID".try_into()?, &aml::EisaName::new("PNP0B00")?)?,
                &aml::Name::new(
                    "_CRS".try_into()?,
                    &aml::ResourceTemplate::new(vec![
                        &aml::Io::new(
                            CMOS_PORT.try_into().unwrap(),
                            CMOS_PORT.try_into().unwrap(),
                            1,
                            2,
                        ),
                        &aml::Interrupt::new(true, true, false, false, RTC_GSI),
                    ]),
                )?,
            ],
        )
        .append_aml_bytes(bytes)?;
        // Setup COM1
        aml::Device::new(
            scope.device_path("COM1")?,
            vec![
                &aml::Name::new("_HID".try_into()?, &aml::EisaName::new("PNP0501")?)?,
                &aml::Name::new("_UID".try_into()?, &0u8)?,
                &aml::Name::new("_DDN".try_into()?, &"COM1")?,
                &aml::Name::new(
                    "_CRS".try_into().unwrap(),
                    &aml::ResourceTemplate::new(vec![
                        &aml::Interrupt::new(true, true, false, false, Self::COM1_GSI),
                        &aml::Io::new(
                            Self::SERIAL_PORT_ADDRESS.try_into().unwrap(),
                            Self::SERIAL_PORT_ADDRESS.try_into().unwrap(),
                            1,
                            Self::SERIAL_PORT_SIZE.try_into().unwrap(),
                        ),
                    ]),
                )?,
            ],
        )
        .append_aml_bytes(bytes)?;
        // Setup i8042
        aml::Device::new(
            scope.device_path("PS2_")?,
            vec![
                &aml::Name::new("_HID".try_into()?, &aml::EisaName::new("PNP0303")?)?,
                &aml::Method::new(
                    "_STA".try_into()?,
                    0,
                    false,
                    vec![&aml::Return::new(&0x0fu8)],
                ),
                &aml::Name::new(
                    "_CRS".try_into()?,
                    &aml::ResourceTemplate::new(vec![
                        &aml::Io::new(
                            PortIODeviceManager::I8042_KDB_DATA_REGISTER_ADDRESS
                                .try_into()
                                .unwrap(),
                            PortIODeviceManager::I8042_KDB_DATA_REGISTER_ADDRESS
                                .try_into()
                                .unwrap(),
                            1u8,
                            1u8,
                        ),
                        // Fake a command port so Linux stops complaining
                        &aml::Io::new(0x0064, 0x0064, 1u8, 1u8),
                        &aml::Interrupt::new(true, true, false, false, Self::KBD_EVT_GSI),
                    ]),
                )?,
            ],
        )
        .append_aml_bytes(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    #[test]
    fn power_reservation_has_unique_id() {
        let mut bytes = Vec::new();
        PortIODeviceManager::append_aml_bytes(&mut bytes, crate::acpi::DeviceScope::SystemBus)
            .unwrap();
        let end = bytes.windows(4).position(|name| name == b"RTC0").unwrap();
        let uid = b"\x08_UID\x0dPOWER\0";
        assert!(bytes[..end].windows(uid.len()).any(|entry| entry == uid));
    }

    #[test]
    fn test_register_legacy_devices() {
        let vm = setup_vm_with_memory(0x1000);
        vm.setup_irqchip().unwrap();
        let mut ldm = PortIODeviceManager::for_tests();
        ldm.register_devices(&vm).unwrap();
        vm.set_gsi_routes().unwrap();
        ldm.cmos.lock().unwrap().irq_evt.write(1).unwrap();
        let mut pic = kvm_bindings::kvm_irqchip {
            chip_id: kvm_bindings::KVM_IRQCHIP_PIC_SLAVE,
            ..Default::default()
        };
        for _ in 0..100 {
            vm.fd().get_irqchip(&mut pic).unwrap();
            // SAFETY: KVM returned the slave PIC selected above.
            if unsafe { pic.chip.pic.irr } & 1 != 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("RTC IRQ 8 did not reach the slave PIC");
    }
}
