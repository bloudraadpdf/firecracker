// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Polled PC RTC for CloudHv firmware. Alarm and periodic IRQs are not exposed.

use std::sync::{Arc, Barrier};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::vstate::bus::BusDevice;

/// PC RTC index and data ports.
pub const CMOS_PORT: u64 = 0x70;
const TIME_REGISTERS: [usize; 8] = [0, 2, 4, 6, 7, 8, 9, 0x32];

/// CMOS bytes and guest wall-clock adjustment. Wall time advances across restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cmos {
    index: u8,
    registers: [[u8; 32]; 4],
    offset_seconds: i64,
}

impl Default for Cmos {
    fn default() -> Self {
        let mut cmos = Self {
            index: 0,
            registers: [[0; 32]; 4],
            offset_seconds: 0,
        };
        cmos.set(0x0a, 0x26);
        cmos.set(0x0b, 2);
        cmos
    }
}

impl Cmos {
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
        if self.get(0x0b) & 0x80 != 0 {
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
            0x0a => self.set(index, value & 0x7f),
            0x0b => {
                let was_set = self.get(index) & 0x80 != 0;
                self.refresh(now);
                self.set(index, value & !0x70);
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
                self.refresh(Self::now());
                data[0] = match self.index & 0x7f {
                    0x0c => 0,
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
                1 => self.write_register(usize::from(self.index & 0x7f), *value, Self::now()),
                _ => (),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
