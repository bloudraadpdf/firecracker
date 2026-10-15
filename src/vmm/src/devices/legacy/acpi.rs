// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! CloudHv firmware timer and hardware-reduced ACPI power registers.

use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use vmm_sys_util::eventfd::EventFd;

use crate::logger::{error, info};
use crate::vstate::bus::BusDevice;

/// CloudHv firmware's power control port.
pub const POWER_PORT: u64 = 0x600;
/// CloudHv firmware's free-running PM timer port.
pub const TIMER_PORT: u64 = 0x608;

/// ACPI reset, sleep control and sleep status registers.
#[derive(Debug)]
pub struct AcpiPower {
    exit_evt: EventFd,
}

impl AcpiPower {
    /// Reset and S5 terminate the microVM, as with the i8042 reset device.
    pub fn new(exit_evt: EventFd) -> Self {
        Self { exit_evt }
    }
}

impl BusDevice for AcpiPower {
    fn read(&mut self, _base: u64, _offset: u64, data: &mut [u8]) {
        data.fill(0);
    }

    fn write(&mut self, _base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        // ACPI 6.5: SLP_EN is bit 5 and SLP_TYP is bits 4:2. _S5 supplies 5.
        if offset != 0 || !matches!(data, [1] | [0x34]) {
            return None;
        }
        info!(
            "Guest requested ACPI {}",
            if data == [1] { "reset" } else { "S5 shutdown" }
        );
        if let Err(err) = self.exit_evt.write(1) {
            error!("Failed to signal ACPI power event: {err}");
        }
        None
    }
}

/// Firmware-only 32-bit ACPI PM timer, at 3.579545 MHz.
///
/// The hardware-reduced FADT does not expose this timer to the operating system.
#[derive(Debug)]
pub struct AcpiTimer {
    start: Instant,
}

impl Default for AcpiTimer {
    fn default() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl AcpiTimer {
    fn counter(elapsed: Duration) -> u32 {
        let ticks = elapsed.as_nanos() * 3_579_545 / 1_000_000_000;
        u32::try_from(ticks & u128::from(u32::MAX)).unwrap()
    }
}

impl BusDevice for AcpiTimer {
    fn read(&mut self, _base: u64, offset: u64, data: &mut [u8]) {
        if offset == 0 && data.len() == size_of::<u32>() {
            data.copy_from_slice(&Self::counter(self.start.elapsed()).to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_frequency_and_wrap() {
        assert_eq!(AcpiTimer::counter(Duration::ZERO), 0);
        assert_eq!(AcpiTimer::counter(Duration::from_secs(1)), 3_579_545);
        assert_eq!(AcpiTimer::counter(Duration::from_millis(500)), 1_789_772);
        assert_eq!(AcpiTimer::counter(Duration::from_secs(1200)), 486_704);
    }

    #[test]
    fn power_signals_only_reset_and_s5() {
        let evt = EventFd::new(libc::EFD_NONBLOCK).unwrap();
        let mut power = AcpiPower::new(evt.try_clone().unwrap());
        for (offset, value) in [(0, 0), (0, 0x14), (0, 0x20), (1, 1), (1, 0x34)] {
            power.write(POWER_PORT, offset, &[value]);
        }
        power.write(POWER_PORT, 0, &[0x34, 0]);
        assert_eq!(
            evt.read().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        power.write(POWER_PORT, 0, &[1]);
        power.write(POWER_PORT, 0, &[0x34]);
        assert_eq!(evt.read().unwrap(), 2);
    }
}
