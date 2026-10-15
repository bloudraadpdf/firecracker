// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! PC RTC calendar, periodic timer, alarm and update interrupts.

use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use event_manager::{EventOps, Events, MutEventSubscriber};
use utils::time::TimerFd;
use vmm_sys_util::epoll::EventSet;
use vmm_sys_util::eventfd::EventFd;

use serde::{Deserialize, Serialize};

use crate::logger::error;
use crate::vstate::bus::BusDevice;

/// PC RTC index and data ports.
pub const CMOS_PORT: u64 = 0x70;
/// PC real-time clock interrupt.
pub const RTC_GSI: u32 = 8;
const TIME_REGISTERS: [usize; 8] = [0, 2, 4, 6, 7, 8, 9, 0x32];

/// CMOS bytes and guest wall-clock adjustment. Wall time advances across restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cmos {
    index: u8,
    registers: [[u8; 32]; 4],
    offset_seconds: i64,
    ticks: u64,
    last_update: i64,
    next_update: u64,
    period_origin: u64,
}

impl Default for Cmos {
    fn default() -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let mut cmos = Self {
            index: 0,
            registers: [[0; 32]; 4],
            offset_seconds: 0,
            ticks: 0,
            last_update: i64::try_from(now.as_secs()).unwrap(),
            next_update: 32768 - u64::from(now.subsec_nanos()) * 32768 / 1_000_000_000,
            period_origin: 0,
        };
        cmos.set(0x0a, 0x26);
        cmos.set(0x0b, 2);
        cmos.refresh(cmos.last_update);
        cmos
    }
}

impl Cmos {
    fn running(&self) -> bool {
        self.get(0x0a) & 0x70 == 0x20
    }

    fn period(&self) -> Option<u64> {
        match self.get(0x0a) & 0x0f {
            0 => None,
            1 => Some(128),
            2 => Some(256),
            rate => Some(1 << (rate - 1)),
        }
    }

    fn advance_calendar(&mut self, now: i64) {
        if self.get(0x0b) & 0x80 == 0 && now != self.last_update {
            self.set(0x0c, self.get(0x0c) | 0x10);
            let elapsed = now.saturating_sub(self.last_update).clamp(1, 86400);
            for second in (now - elapsed + 1)..=now {
                self.refresh(second);
                if [0, 2, 4].into_iter().all(|index| {
                    let alarm = self.get(index + 1);
                    alarm & 0xc0 == 0xc0 || alarm == self.get(index)
                }) {
                    self.set(0x0c, self.get(0x0c) | 0x20);
                    break;
                }
            }
            self.refresh(now);
        }
        self.last_update = now;
    }

    fn advance(&mut self, ticks: u64) {
        if self.running() {
            if let Some(period) = self.period()
                && ticks.wrapping_sub(self.period_origin) / period
                    != self.ticks.wrapping_sub(self.period_origin) / period
            {
                self.set(0x0c, self.get(0x0c) | 0x40);
            }
            if ticks >= self.next_update {
                let updates = 1 + (ticks - self.next_update) / 32768;
                self.next_update = self.next_update.wrapping_add(updates * 32768);
                self.advance_calendar(
                    self.last_update
                        .saturating_add(i64::try_from(updates).unwrap()),
                );
            }
        }
        self.ticks = ticks;
    }

    fn uip(&self) -> bool {
        self.running()
            && self.get(0x0b) & 0x80 == 0
            && self.next_update.saturating_sub(self.ticks) <= 8
    }

    fn get(&self, index: usize) -> u8 {
        self.registers[index / 32][index % 32]
    }

    fn set(&mut self, index: usize, value: u8) {
        self.registers[index / 32][index % 32] = value;
    }

    fn encode(&self, value: i32) -> u8 {
        let value = u8::try_from(value).unwrap();
        if self.get(0x0b) & 4 != 0 {
            value
        } else {
            (value / 10) * 16 + value % 10
        }
    }

    fn decode(&self, index: usize) -> i32 {
        let value = self.get(index) & if index == 4 { 0x7f } else { 0xff };
        i32::from(if self.get(0x0b) & 4 != 0 {
            value
        } else {
            (value >> 4) * 10 + (value & 15)
        })
    }

    fn now() -> i64 {
        i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap()
    }

    fn refresh(&mut self, now: i64) {
        if self.get(0x0b) & 0x80 != 0 || !self.running() {
            return;
        }
        let seconds = now.saturating_add(self.offset_seconds);
        // JUSTIFICATION: libc supplies UTC calendar conversion without a new dependency.
        // SAFETY: both pointers refer to valid, correctly sized objects. A null result
        // leaves the previous register values intact.
        let mut calendar: libc::tm = unsafe { std::mem::zeroed() };
        // SAFETY: seconds and calendar are live, correctly sized objects.
        if unsafe { libc::gmtime_r(&seconds, &mut calendar) }.is_null() {
            return;
        }
        if !(0..=9999).contains(&(calendar.tm_year + 1900)) {
            return;
        }
        let mut hour = calendar.tm_hour;
        let pm = if self.get(0x0b) & 2 == 0 {
            let pm = if hour >= 12 { 0x80 } else { 0 };
            hour = (hour + 11) % 12 + 1;
            pm
        } else {
            0
        };
        for (index, value) in TIME_REGISTERS.into_iter().zip([
            calendar.tm_sec,
            calendar.tm_min,
            hour,
            calendar.tm_wday + 1,
            calendar.tm_mday,
            calendar.tm_mon + 1,
            (calendar.tm_year + 1900) % 100,
            (calendar.tm_year + 1900) / 100,
        ]) {
            self.set(index, self.encode(value));
        }
        self.set(4, self.get(4) | pm);
    }

    fn commit_time(&mut self, now: i64) {
        // JUSTIFICATION: libc supplies UTC calendar conversion.
        // SAFETY: an all-zero tm is valid and timegm receives a live mutable object.
        let mut calendar: libc::tm = unsafe { std::mem::zeroed() };
        calendar.tm_sec = self.decode(0);
        calendar.tm_min = self.decode(2);
        calendar.tm_hour = self.decode(4);
        if self.get(0x0b) & 2 == 0 {
            calendar.tm_hour = calendar.tm_hour % 12 + i32::from(self.get(4) >> 7) * 12;
        }
        calendar.tm_mday = self.decode(7);
        calendar.tm_mon = self.decode(8) - 1;
        calendar.tm_year = self.decode(0x32) * 100 + self.decode(9) - 1900;
        if !(0..60).contains(&calendar.tm_sec)
            || !(0..60).contains(&calendar.tm_min)
            || !(0..24).contains(&calendar.tm_hour)
            || !(1..=31).contains(&calendar.tm_mday)
            || !(0..12).contains(&calendar.tm_mon)
            || !(70..=8099).contains(&calendar.tm_year)
        {
            return;
        }
        // SAFETY: calendar is an initialised, writable tm.
        let seconds = unsafe { libc::timegm(&mut calendar) };
        self.offset_seconds = seconds.saturating_sub(now);
    }

    fn write_register(&mut self, index: usize, value: u8, now: i64) {
        match index {
            0x0a => {
                let was_running = self.running();
                self.set(index, value & 0x7f);
                if !was_running && self.running() {
                    self.period_origin = self.ticks;
                    self.next_update = self.ticks.wrapping_add(16384);
                }
            }
            0x0b => {
                let was_set = self.get(index) & 0x80 != 0;
                self.refresh(now);
                self.set(
                    index,
                    if value & 0x80 != 0 {
                        value & !0x10
                    } else {
                        value
                    },
                );
                if was_set && value & 0x80 == 0 {
                    self.commit_time(now);
                }
            }
            0x0c | 0x0d => (),
            _ => {
                self.set(index, value);
                if TIME_REGISTERS.contains(&index) && self.get(0x0b) & 0x80 == 0 {
                    self.commit_time(now);
                }
            }
        }
    }
}

impl BusDevice for Cmos {
    fn read(&mut self, _base: u64, offset: u64, data: &mut [u8]) {
        if data.len() != 1 {
            return;
        }
        match offset {
            0 => data[0] = self.index,
            1 => {
                data[0] = match self.index & 0x7f {
                    0x0a => self.get(0x0a) | if self.uip() { 0x80 } else { 0 },
                    0x0c => {
                        let status = self.get(0x0c);
                        self.set(0x0c, 0);
                        status
                    }
                    0x0d => 0x80,
                    index => self.get(usize::from(index)),
                };
            }
            _ => (),
        }
    }

    fn write(&mut self, _base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        if let [value] = data {
            match offset {
                0 => self.index = *value,
                1 => self.write_register(usize::from(self.index & 0x7f), *value, self.last_update),
                _ => (),
            }
        }
        None
    }
}

/// RTC interrupt scheduler and guest register state.
#[derive(Debug)]
pub struct Rtc {
    state: Cmos,
    pub(crate) irq_evt: EventFd,
    timer: TimerFd,
    start: Instant,
    base_ticks: u64,
    asserted: bool,
}

impl Rtc {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            state: Cmos::default(),
            irq_evt: EventFd::new(libc::EFD_NONBLOCK)?,
            timer: TimerFd::new(),
            start: Instant::now(),
            base_ticks: 0,
            asserted: false,
        })
    }

    fn update(&mut self) {
        let ticks = self.base_ticks.wrapping_add(
            u64::try_from(self.start.elapsed().as_nanos() * 32768 / 1_000_000_000).unwrap(),
        );
        self.state.advance(ticks);
        self.update_irq();
    }

    fn update_irq(&mut self) {
        let pending = self.state.get(0x0b) & self.state.get(0x0c) & 0x70 != 0;
        self.state.set(
            0x0c,
            (self.state.get(0x0c) & 0x70) | if pending { 0x80 } else { 0 },
        );
        if pending
            && !self.asserted
            && let Err(err) = self.irq_evt.write(1)
        {
            error!("Failed to signal RTC interrupt: {err}");
            return;
        }
        self.asserted = pending;
    }

    fn schedule(&mut self) {
        let mut delay = None;
        if self.state.running() && !self.asserted {
            if self.state.get(0x0b) & 0x40 != 0
                && let Some(period) = self.state.period()
            {
                let ticks =
                    period - self.state.ticks.wrapping_sub(self.state.period_origin) % period;
                delay = Some(Duration::from_nanos(
                    (ticks * 1_000_000_000).div_ceil(32768),
                ));
            }
            if self.state.get(0x0b) & 0x80 == 0 && self.state.get(0x0b) & 0x30 != 0 {
                let ticks = self
                    .state
                    .next_update
                    .saturating_sub(self.state.ticks)
                    .max(1);
                let update = Duration::from_nanos((ticks * 1_000_000_000).div_ceil(32768));
                delay = Some(delay.map_or(update, |period| period.min(update)));
            }
        }
        self.timer.arm(delay.unwrap_or(Duration::ZERO), None);
    }

    pub fn save(&mut self) -> Cmos {
        self.update();
        let mut saved = self.state.clone();
        let now = Cmos::now();
        saved.offset_seconds = saved
            .offset_seconds
            .saturating_add(saved.last_update.saturating_sub(now));
        saved.last_update = now;
        saved
    }

    pub fn restore(&mut self, state: &Cmos) {
        self.state = state.clone();
        let now = Cmos::now();
        if self.state.running() && self.state.get(0x0b) & 0x80 == 0 {
            self.state.advance_calendar(now);
        } else {
            self.state.offset_seconds = self
                .state
                .offset_seconds
                .saturating_add(self.state.last_update.saturating_sub(now));
            self.state.last_update = now;
        }
        self.base_ticks = state.ticks;
        self.start = Instant::now();
        self.asserted = false;
        self.update_irq();
        self.schedule();
    }
}

impl BusDevice for Rtc {
    fn read(&mut self, base: u64, offset: u64, data: &mut [u8]) {
        self.update();
        self.state.read(base, offset, data);
        self.update_irq();
        self.schedule();
    }

    fn write(&mut self, base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        self.update();
        self.state.write(base, offset, data);
        self.update_irq();
        self.schedule();
        None
    }
}

impl MutEventSubscriber for Rtc {
    fn init(&mut self, ops: &mut EventOps) {
        if let Err(err) = ops.add(Events::new(&self.timer, EventSet::IN)) {
            error!("Failed to register RTC timer: {err}");
        }
    }

    fn process(&mut self, event: Events, _ops: &mut EventOps) {
        if event.event_set() == EventSet::IN {
            self.timer.read();
            self.update();
            self.schedule();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divider_restart_waits_half_a_second_and_exposes_uip() {
        let mut rtc = Cmos {
            last_update: 0,
            ..Default::default()
        };
        rtc.refresh(0);
        rtc.write_register(0x0a, 0x66, 0);
        rtc.advance(32768);
        assert_eq!(rtc.get(0), 0);
        rtc.write_register(0x0a, 0x26, 1);
        rtc.advance(32768 + 16376);
        rtc.write(CMOS_PORT, 0, &[0x0a]);
        let mut status = [0];
        rtc.read(CMOS_PORT, 1, &mut status);
        assert_ne!(status[0] & 0x80, 0);
        assert_eq!(rtc.get(0), 0);
        rtc.advance(32768 + 16384);
        assert_eq!(rtc.get(0), 1);
    }

    #[test]
    fn periodic_flags_latch_while_masked_and_set_does_not_stop_them() {
        let mut rtc = Cmos {
            last_update: 0,
            next_update: 32768,
            ..Default::default()
        };
        rtc.advance(32);
        assert_eq!(rtc.get(0x0c), 0x40);
        rtc.write(CMOS_PORT, 0, &[0x0c]);
        let mut status = [0];
        rtc.read(CMOS_PORT, 1, &mut status);
        assert_eq!(status, [0x40]);
        assert_eq!(rtc.get(0x0c), 0);
        rtc.write_register(0x0b, 0xf2, 0);
        assert_eq!(rtc.get(0x0b), 0xe2);
        rtc.advance(64);
        assert_eq!(rtc.get(0x0c), 0x40);
        rtc.set(0x0c, 0);
        rtc.write_register(0x0a, 0x66, 1);
        rtc.advance(96);
        assert_eq!(rtc.get(0x0c), 0);
    }

    #[test]
    fn periodic_rates_and_alarm_update_flags() {
        let mut rtc = Cmos::default();
        for (rate, period) in [
            None,
            Some(128),
            Some(256),
            Some(4),
            Some(8),
            Some(16),
            Some(32),
            Some(64),
            Some(128),
            Some(256),
            Some(512),
            Some(1024),
            Some(2048),
            Some(4096),
            Some(8192),
            Some(16384),
        ]
        .into_iter()
        .enumerate()
        {
            rtc.set(0x0a, 0x20 | u8::try_from(rate).unwrap());
            assert_eq!(rtc.period(), period);
        }
        rtc.last_update = 0;
        rtc.set(0x0a, 0x20);
        rtc.set(1, 5);
        rtc.set(3, 0xc0);
        rtc.set(5, 0xc0);
        rtc.next_update = 32768;
        rtc.advance(10 * 32768);
        assert_eq!(rtc.get(0x0c), 0x30);
        assert_eq!(rtc.get(0), 0x10);
    }

    #[test]
    fn rtc_timer_event_and_pending_restore() {
        use event_manager::SubscriberOps;
        use std::sync::Mutex;

        let rtc = Arc::new(Mutex::new(Rtc::new().unwrap()));
        let mut events = crate::EventManager::new().unwrap();
        events.add_subscriber(rtc.clone());
        {
            let mut rtc = rtc.lock().unwrap();
            rtc.state.set(0x0a, 0x2f);
            rtc.state.set(0x0b, 0x42);
            rtc.state.ticks = 16384 - 327;
            rtc.base_ticks = rtc.state.ticks;
            rtc.start = Instant::now();
            rtc.schedule();
        }
        events.run_with_timeout(1000).unwrap();
        let saved = {
            let mut rtc = rtc.lock().unwrap();
            assert_eq!(rtc.irq_evt.read().unwrap(), 1);
            rtc.save()
        };
        assert_eq!(saved.get(0x0c) & 0xc0, 0xc0);
        let mut restored = Rtc::new().unwrap();
        restored.restore(&saved);
        assert_eq!(restored.irq_evt.read().unwrap(), 1);
        assert!(restored.save().ticks >= saved.ticks);
        restored.write(CMOS_PORT, 0, &[0x0c]);
        let mut status = [0];
        restored.read(CMOS_PORT, 1, &mut status);
        assert_eq!(status[0] & 0xc0, 0xc0);
        assert!(!restored.asserted);
        assert_eq!(restored.state.get(0x0c), 0);
        assert!(restored.timer.is_armed());
    }

    #[test]
    fn rtc_interrupt_enables_are_writable() {
        let mut rtc = Cmos::default();
        rtc.write_register(0x0b, 0x72, 0);
        assert_eq!(rtc.get(0x0b), 0x72);
    }

    #[test]
    fn utc_formats_and_nmi_mask() {
        let mut rtc = Cmos::default();
        rtc.refresh(0);
        assert_eq!(
            [rtc.get(7), rtc.get(8), rtc.get(9), rtc.get(0x32)],
            [1, 1, 0x70, 0x19]
        );
        rtc.set(0x0b, 0);
        rtc.refresh(13 * 3600);
        assert_eq!(rtc.get(4), 0x81);
        rtc.set(0x0b, 6);
        rtc.refresh(13 * 3600);
        assert_eq!(rtc.get(4), 13);
        rtc.write(CMOS_PORT, 0, &[0x8d]);
        let mut value = [0];
        rtc.read(CMOS_PORT, 1, &mut value);
        assert_eq!(value, [0x80]);
    }

    #[test]
    fn set_freezes_clock_and_snapshot_preserves_offset() {
        let mut rtc = Cmos::default();
        rtc.write_register(0x0b, 0x82, 0);
        rtc.write_register(0, 0x30, 10);
        rtc.refresh(20);
        assert_eq!(rtc.get(0), 0x30);
        rtc.write_register(0x0b, 2, 20);
        let bytes = serde_json::to_vec(&rtc).unwrap();
        let mut restored: Cmos = serde_json::from_slice(&bytes).unwrap();
        restored.refresh(25);
        assert_eq!(restored.get(0), 0x35);
        assert_eq!(restored.offset_seconds, 10);
    }
}
