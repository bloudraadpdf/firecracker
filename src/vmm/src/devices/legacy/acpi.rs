// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! ACPI-only PM1 registers, reset/sleep controls and a 32-bit PM timer.

#[cfg(feature = "windows")]
use std::sync::{Arc, Barrier};
#[cfg(feature = "windows")]
use std::time::{Duration, Instant};

#[cfg(feature = "windows")]
use event_manager::{EventOps, Events, MutEventSubscriber};
use serde::{Deserialize, Serialize};
#[cfg(feature = "windows")]
use utils::time::TimerFd;
#[cfg(feature = "windows")]
use vmm_sys_util::epoll::EventSet;
#[cfg(feature = "windows")]
use vmm_sys_util::eventfd::EventFd;

#[cfg(feature = "windows")]
use crate::logger::{error, info};
#[cfg(feature = "windows")]
use crate::vstate::bus::BusDevice;

pub const POWER_PORT: u64 = 0x600;
pub const PM1_EVENT_PORT: u64 = POWER_PORT + 2;
pub const PM1_CONTROL_PORT: u64 = POWER_PORT + 6;
pub const TIMER_PORT: u64 = POWER_PORT + 8;
pub const SCI_GSI: u32 = 3;
pub const GPE0_PORT: u64 = POWER_PORT + 12;
pub const PORT_COUNT: usize = 14;
const POWER_BUTTON_EVENT: u16 = 1 << 8;
const PM1_EVENTS: u16 = 1 | POWER_BUTTON_EVENT;
#[cfg(feature = "windows")]
const TIMER_HZ: u64 = 3_579_545;
#[cfg(feature = "windows")]
const TIMER_HALF_PERIOD: u64 = 1 << 31;

/// Guest-visible state; the host clock anchor and file descriptors are recreated.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AcpiPowerState {
    ticks: u64,
    status: u16,
    enable: u16,
    control: u16,
    gpe_status: u8,
    gpe_enable: u8,
}

impl AcpiPowerState {
    pub(crate) fn validate(&self) -> std::io::Result<()> {
        if self.status & !PM1_EVENTS != 0
            || self.enable & !PM1_EVENTS != 0
            || self.control & !0x1c00 != 0
            || self.gpe_status & !1 != 0
            || self.gpe_enable & !1 != 0
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid ACPI event or control state",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
#[cfg(feature = "windows")]
pub struct AcpiPower {
    exit_evt: EventFd,
    pub(crate) sci_evt: EventFd,
    pub(crate) resample_evt: EventFd,
    timer: TimerFd,
    state: AcpiPowerState,
    start: Instant,
    base_ticks: u64,
    sci_asserted: bool,
    reset_requested: bool,
    registration: Result<(), String>,
}

#[cfg(feature = "windows")]
impl AcpiPower {
    pub fn new(exit_evt: EventFd) -> std::io::Result<Self> {
        let mut device = Self {
            exit_evt,
            sci_evt: EventFd::new(libc::EFD_NONBLOCK)?,
            resample_evt: EventFd::new(libc::EFD_NONBLOCK)?,
            timer: TimerFd::new(),
            state: AcpiPowerState::default(),
            start: Instant::now(),
            base_ticks: 0,
            sci_asserted: false,
            reset_requested: false,
            registration: Err("ACPI events not registered".to_owned()),
        };
        device.arm_timer();
        Ok(device)
    }

    pub(crate) fn check_registered(&self) -> std::io::Result<()> {
        self.registration.clone().map_err(std::io::Error::other)
    }

    pub(crate) fn reset_registration(&mut self) {
        self.registration = Err("ACPI events not registered".to_owned());
    }

    fn elapsed_ticks(elapsed: Duration) -> u64 {
        u64::try_from(elapsed.as_nanos() * u128::from(TIMER_HZ) / 1_000_000_000).unwrap()
    }

    fn advance_to(&mut self, ticks: u64) {
        if (ticks >> 31) != (self.state.ticks >> 31) {
            self.state.status |= 1;
        }
        self.state.ticks = ticks;
    }

    fn update_timer(&mut self) {
        self.advance_to(
            self.base_ticks
                .wrapping_add(Self::elapsed_ticks(self.start.elapsed())),
        );
    }

    fn signal_sci(&mut self) -> std::io::Result<()> {
        let pending = self.state.status & self.state.enable & PM1_EVENTS != 0
            || self.state.gpe_status & self.state.gpe_enable & 1 != 0;
        if pending && !self.sci_asserted {
            self.sci_evt.write(1)?;
        }
        self.sci_asserted = pending;
        Ok(())
    }

    fn update_sci(&mut self) {
        if let Err(err) = self.signal_sci() {
            error!("Failed to signal ACPI SCI: {err}");
        }
    }

    pub(crate) fn notify_power_button(&mut self) -> std::io::Result<()> {
        self.state.status |= POWER_BUTTON_EVENT;
        self.signal_sci()
    }

    pub(crate) fn reset_requested(&self) -> bool {
        self.reset_requested
    }

    pub(crate) fn notify_vmgenid(&mut self) -> std::io::Result<()> {
        self.state.gpe_status |= 1;
        self.signal_sci()
    }

    fn arm_timer(&mut self) {
        let remaining = TIMER_HALF_PERIOD - self.state.ticks % TIMER_HALF_PERIOD;
        self.timer.arm(
            Duration::from_nanos((remaining * 1_000_000_000).div_ceil(TIMER_HZ)),
            None,
        );
    }

    pub fn save(&mut self) -> AcpiPowerState {
        self.update_timer();
        self.update_sci();
        self.state.clone()
    }

    pub fn restore(&mut self, state: &AcpiPowerState) -> std::io::Result<()> {
        state.validate()?;
        self.state = state.clone();
        self.base_ticks = state.ticks;
        self.start = Instant::now();
        self.sci_asserted = false;
        self.arm_timer();
        self.signal_sci()
    }

    fn shutdown(&mut self, reset: bool) {
        self.reset_requested |= reset;
        info!(
            "Guest requested ACPI {}",
            if reset { "reset" } else { "S5 shutdown" }
        );
        if let Err(err) = self.exit_evt.write(1) {
            error!("Failed to signal ACPI power event: {err}");
        }
    }
}

#[cfg(feature = "windows")]
impl BusDevice for AcpiPower {
    fn read(&mut self, _base: u64, offset: u64, data: &mut [u8]) {
        self.update_timer();
        self.update_sci();
        data.fill(0);
        let mut registers = [0u8; PORT_COUNT];
        registers[2..4].copy_from_slice(&self.state.status.to_le_bytes());
        registers[4..6].copy_from_slice(&self.state.enable.to_le_bytes());
        registers[6..8].copy_from_slice(&(self.state.control | 1).to_le_bytes());
        registers[8..12].copy_from_slice(
            &u32::try_from(self.state.ticks & u64::from(u32::MAX))
                .unwrap()
                .to_le_bytes(),
        );
        registers[12] = self.state.gpe_status;
        registers[13] = self.state.gpe_enable;
        if let Ok(offset) = usize::try_from(offset)
            && let Some(end) = offset.checked_add(data.len())
            && let Some(value) = registers.get(offset..end)
        {
            data.copy_from_slice(value);
        }
    }

    fn write(&mut self, _base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        self.update_timer();
        if offset == 0 && matches!(data, [1] | [0x34]) {
            self.shutdown(data == [1]);
        } else if let Ok(offset) = usize::try_from(offset)
            && let Some(end) = offset.checked_add(data.len())
            && offset >= 2
            && end <= PORT_COUNT
        {
            let mut sleep = false;
            for (address, byte) in (offset..end).zip(data) {
                let shift = (address % 2) * 8;
                let value = u16::from(*byte) << shift;
                let mask = 0xffu16 << shift;
                match address {
                    2..4 => self.state.status &= !(value & PM1_EVENTS),
                    4..6 => self.state.enable = (self.state.enable & !mask) | (value & PM1_EVENTS),
                    6..8 => {
                        self.state.control = (self.state.control & !mask) | (value & 0x1c00);
                        sleep |= value & 0x2000 != 0;
                    }
                    12 => self.state.gpe_status &= !(*byte & 1),
                    13 => self.state.gpe_enable = *byte & 1,
                    _ => {}
                }
            }
            if sleep && self.state.control == 0x1400 {
                self.shutdown(false);
            }
        }
        self.update_sci();
        None
    }
}

#[cfg(feature = "windows")]
impl MutEventSubscriber for AcpiPower {
    fn init(&mut self, ops: &mut EventOps) {
        for event in [
            Events::with_data(&self.timer, 0, EventSet::IN),
            Events::with_data(&self.resample_evt, 1, EventSet::IN),
        ] {
            if let Err(err) = ops.add(event) {
                self.registration = Err(format!("Failed to register ACPI event: {err}"));
                return;
            }
        }
        self.registration = Ok(());
    }

    fn process(&mut self, event: Events, _ops: &mut EventOps) {
        if event.event_set() != EventSet::IN {
            return;
        }
        match event.data() {
            0 => {
                self.timer.read();
            }
            1 => {
                if let Err(err) = self.resample_evt.read() {
                    error!("Failed to consume ACPI SCI resample: {err}");
                    return;
                }
                self.sci_asserted = false;
            }
            _ => return,
        }
        self.update_timer();
        self.update_sci();
        self.arm_timer();
    }
}

#[cfg(test)]
#[cfg(feature = "windows")]
mod tests {
    use std::sync::Mutex;

    use event_manager::SubscriberOps;

    use super::*;

    #[test]
    fn power_button_latches_until_enabled_and_guest_acknowledges() {
        let exit = EventFd::new(libc::EFD_NONBLOCK).unwrap();
        let mut power = AcpiPower::new(exit.try_clone().unwrap()).unwrap();
        power.notify_power_button().unwrap();
        assert_eq!(power.state.status, POWER_BUTTON_EVENT);
        assert_eq!(
            power.sci_evt.read().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let mut restored = AcpiPower::new(exit.try_clone().unwrap()).unwrap();
        restored.restore(&power.save()).unwrap();
        restored.write(POWER_PORT, 5, &[1]);
        assert_eq!(restored.sci_evt.read().unwrap(), 1);
        restored.notify_power_button().unwrap();
        assert_eq!(
            restored.sci_evt.read().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        restored.write(POWER_PORT, 2, &[0, 1]);
        assert_eq!(restored.state.status, 0);
        restored.notify_power_button().unwrap();
        assert_eq!(restored.sci_evt.read().unwrap(), 1);
        assert_eq!(
            exit.read().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(!restored.reset_requested());
    }

    #[test]
    fn pending_power_button_restores_and_resamples_with_timer_and_generation() {
        let mut power = AcpiPower::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap();
        power.write(POWER_PORT, 4, &PM1_EVENTS.to_le_bytes());
        power.write(POWER_PORT, 13, &[1]);
        power.advance_to(TIMER_HALF_PERIOD);
        power.notify_power_button().unwrap();
        power.notify_vmgenid().unwrap();
        let mut restored = AcpiPower::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap();
        restored.restore(&power.save()).unwrap();
        assert_eq!(restored.sci_evt.read().unwrap(), 1);
        let restored = Arc::new(Mutex::new(restored));
        let mut events = crate::EventManager::new().unwrap();
        events.add_subscriber(restored.clone());
        {
            let mut restored = restored.lock().unwrap();
            restored.write(POWER_PORT, 3, &[1]);
            assert_eq!(restored.state.status & POWER_BUTTON_EVENT, 0);
            assert_eq!(restored.state.status & 1, 1);
            restored.resample_evt.write(1).unwrap();
        }
        events.run_with_timeout(1000).unwrap();
        {
            let mut restored = restored.lock().unwrap();
            assert_eq!(restored.sci_evt.read().unwrap(), 1);
            restored.write(POWER_PORT, 2, &[1]);
            restored.write(POWER_PORT, 12, &[1]);
            restored.resample_evt.write(1).unwrap();
        }
        events.run_with_timeout(1000).unwrap();
        assert_eq!(
            restored.lock().unwrap().sci_evt.read().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn reset_and_guest_poweroff_have_distinct_host_latches() {
        for reset in [false, true] {
            let exit = EventFd::new(libc::EFD_NONBLOCK).unwrap();
            let mut power = AcpiPower::new(exit.try_clone().unwrap()).unwrap();
            if reset {
                power.write(POWER_PORT, 0, &[1]);
            } else {
                power.write(POWER_PORT, 6, &0x3400u16.to_le_bytes());
            }
            assert_eq!(exit.read().unwrap(), 1);
            assert_eq!(power.reset_requested(), reset);
            let mut restored = AcpiPower::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap();
            restored.restore(&power.save()).unwrap();
            assert!(!restored.reset_requested());
        }
    }

    #[test]
    fn timer_event_and_resample_survive_restore() {
        let exit = EventFd::new(libc::EFD_NONBLOCK).unwrap();
        let device = Arc::new(Mutex::new(AcpiPower::new(exit).unwrap()));
        let mut events = crate::EventManager::new().unwrap();
        events.add_subscriber(device.clone());
        device
            .lock()
            .unwrap()
            .restore(&AcpiPowerState {
                ticks: TIMER_HALF_PERIOD - TIMER_HZ / 100,
                enable: 1,
                ..Default::default()
            })
            .unwrap();
        events.run_with_timeout(1000).unwrap();
        let saved = {
            let mut device = device.lock().unwrap();
            assert_eq!(device.sci_evt.read().unwrap(), 1);
            let saved = device.save();
            assert_eq!(saved.status, 1);
            assert!(saved.ticks >= TIMER_HALF_PERIOD);
            saved
        };
        let mut restored = AcpiPower::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap();
        restored.restore(&saved).unwrap();
        assert_eq!(restored.sci_evt.read().unwrap(), 1);
        assert!(restored.save().ticks >= saved.ticks);
        let restored = Arc::new(Mutex::new(restored));
        let mut events = crate::EventManager::new().unwrap();
        events.add_subscriber(restored.clone());
        restored.lock().unwrap().resample_evt.write(1).unwrap();
        events.run_with_timeout(1000).unwrap();
        {
            let mut restored = restored.lock().unwrap();
            assert_eq!(restored.sci_evt.read().unwrap(), 1);
            restored.write(POWER_PORT, 2, &[1]);
            assert_eq!(restored.state.status, 0);
            restored.resample_evt.write(1).unwrap();
        }
        events.run_with_timeout(1000).unwrap();
        assert_eq!(
            restored.lock().unwrap().sci_evt.read().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn generation_event_latches_masks_and_resamples_both_sources() {
        for first in [2, 12] {
            let power = Arc::new(Mutex::new(
                AcpiPower::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap(),
            ));
            let mut events = crate::EventManager::new().unwrap();
            events.add_subscriber(power.clone());
            {
                let mut power = power.lock().unwrap();
                power.notify_vmgenid().unwrap();
                let mut gpe = [0; 2];
                power.read(POWER_PORT, 12, &mut gpe);
                assert_eq!(gpe, [1, 0]);
                assert_eq!(
                    power.sci_evt.read().unwrap_err().kind(),
                    std::io::ErrorKind::WouldBlock
                );
                power.write(POWER_PORT, 13, &[0xff]);
                assert_eq!(power.sci_evt.read().unwrap(), 1);
                power.advance_to(TIMER_HALF_PERIOD);
                power.write(POWER_PORT, 4, &[1]);
                power.write(POWER_PORT, first, &[1]);
                power.resample_evt.write(1).unwrap();
            }
            events.run_with_timeout(1000).unwrap();
            {
                let mut power = power.lock().unwrap();
                assert_eq!(power.sci_evt.read().unwrap(), 1);
                power.write(POWER_PORT, if first == 2 { 12 } else { 2 }, &[1]);
                power.resample_evt.write(1).unwrap();
            }
            events.run_with_timeout(1000).unwrap();
            let mut power = power.lock().unwrap();
            assert_eq!(
                power.sci_evt.read().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            let mut gpe = [0; 2];
            power.read(POWER_PORT, 12, &mut gpe);
            assert_eq!(gpe, [0, 1]);
        }
    }

    #[test]
    fn disabled_generation_event_survives_restore_until_enabled() {
        let mut source = AcpiPower::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap();
        source.notify_vmgenid().unwrap();
        let mut restored = AcpiPower::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap();
        restored.restore(&source.save()).unwrap();
        assert_eq!(
            restored.sci_evt.read().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        restored.write(POWER_PORT, 13, &[1]);
        assert_eq!(restored.sci_evt.read().unwrap(), 1);
        let mut gpe = [0; 2];
        restored.read(POWER_PORT, 12, &mut gpe);
        assert_eq!(gpe, [1, 1]);
    }

    #[test]
    fn generation_register_bounds_and_restore() {
        let mut power = AcpiPower::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap();
        power.write(POWER_PORT, 12, &[0xff, 0xff]);
        power.notify_vmgenid().unwrap();
        assert_eq!(power.sci_evt.read().unwrap(), 1);
        power.write(POWER_PORT, 13, &[0, 0xff]);
        let mut gpe = [0; 2];
        power.read(POWER_PORT, 12, &mut gpe);
        assert_eq!(gpe, [1, 1]);
        let saved = power.save();
        let mut restored = AcpiPower::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap();
        restored.restore(&saved).unwrap();
        assert_eq!(restored.sci_evt.read().unwrap(), 1);
        restored.write(POWER_PORT, 12, &[1, 0]);
        restored.read(POWER_PORT, 12, &mut gpe);
        assert_eq!(gpe, [0, 0]);
        for state in [
            AcpiPowerState {
                gpe_status: 2,
                ..Default::default()
            },
            AcpiPowerState {
                gpe_enable: 2,
                ..Default::default()
            },
        ] {
            assert_eq!(
                restored.restore(&state).unwrap_err().kind(),
                std::io::ErrorKind::InvalidData
            );
        }
        assert_eq!(restored.save().gpe_status, 0);
        assert_eq!(restored.save().gpe_enable, 0);
    }

    #[test]
    fn timer_latches_both_msb_transitions_and_multiple_wraps() {
        let mut device = AcpiPower::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap();
        for ticks in [TIMER_HALF_PERIOD, 1 << 32, 3 << 32] {
            device.advance_to(ticks);
            assert_eq!(device.state.status, 1);
            device.state.status = 0;
        }
        assert_eq!(device.state.ticks & u64::from(u32::MAX), 0);
        const { assert!(SCI_GSI < crate::arch::GSI_LEGACY_START) };
    }

    #[test]
    fn conventional_control_is_acpi_enabled_and_shuts_down() {
        let exit = EventFd::new(libc::EFD_NONBLOCK).unwrap();
        let mut power = AcpiPower::new(exit.try_clone().unwrap()).unwrap();
        let mut control = [0; 2];
        power.read(POWER_PORT, 6, &mut control);
        assert_eq!(u16::from_le_bytes(control), 1);
        power.write(POWER_PORT, 6, &0x3401u16.to_le_bytes());
        assert_eq!(exit.read().unwrap(), 1);
    }

    #[test]
    fn timer_frequency_and_wrap() {
        assert_eq!(AcpiPower::elapsed_ticks(Duration::ZERO), 0);
        assert_eq!(AcpiPower::elapsed_ticks(Duration::from_secs(1)), 3_579_545);
        assert_eq!(
            AcpiPower::elapsed_ticks(Duration::from_millis(500)),
            1_789_772
        );
        assert_eq!(
            AcpiPower::elapsed_ticks(Duration::from_secs(1200)),
            4_295_454_000
        );
    }

    #[test]
    fn power_signals_only_reset_and_s5() {
        let evt = EventFd::new(libc::EFD_NONBLOCK).unwrap();
        let mut power = AcpiPower::new(evt.try_clone().unwrap()).unwrap();
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
