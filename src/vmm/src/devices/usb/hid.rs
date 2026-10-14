// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! USB 1.1 HID absolute tablet. Endpoint zero and the interrupt endpoint use
//! eight-byte packets; snapshot restore validates every variable-length field.

use std::collections::VecDeque;
use std::io;

use serde::{Deserialize, Serialize};

const REPORT_LEN: usize = 6;
const QUEUE_LIMIT: usize = 32;
const REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x09, 0x01, 0xa1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x03,
    0x15, 0x00, 0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05, 0x81, 0x01,
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x00, 0x26, 0xff, 0x7f, 0x75, 0x10, 0x95, 0x02, 0x81,
    0x02, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7f, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06, 0xc0, 0xc0,
];
const DEVICE_DESCRIPTOR: &[u8] = &[
    18, 1, 0x10, 0x01, 0, 0, 0, 8, 0x0f, 0x1d, 0x03, 0xfc, 0, 1, 1, 2, 0, 1,
];
const HID_DESCRIPTOR: &[u8] = &[9, 0x21, 0x11, 0x01, 0, 1, 0x22, 63, 0];
const CONFIG_DESCRIPTOR: &[u8] = &[
    9, 2, 34, 0, 1, 1, 0, 0x80, 25, 9, 4, 0, 0, 1, 3, 0, 0, 0, 9, 0x21, 0x11, 0x01, 0, 1, 0x22, 63,
    0, 7, 5, 0x81, 3, 8, 0, 10,
];

/// A USB transaction result, including timeout for an absent USB address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbResult {
    /// A packet of at most eight bytes was acknowledged.
    Ack {
        /// Packet storage; only the first `len` bytes are valid.
        data: [u8; 8],
        /// Number of valid bytes.
        len: u8,
    },
    /// The endpoint is not ready yet.
    Nak,
    /// The endpoint rejected the request.
    Stall,
    /// No device responds to this address.
    NoDevice,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    #[default]
    Idle,
    DataIn,
    StatusIn,
    StatusOut,
    CompletedOut,
    Stalled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum Commit {
    #[default]
    None,
    Address(u8),
    Configuration(u8),
    Idle(u8),
    Halt(bool),
    Interface,
}

/// Bounded state of a tablet, including an in-flight USB control transfer.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HidTabletState {
    address: u8,
    configuration: u8,
    idle: u8,
    idle_elapsed: u16,
    halted: bool,
    report: [u8; REPORT_LEN],
    reports: VecDeque<[u8; REPORT_LEN]>,
    interrupt_toggle: bool,
    interrupt_last: Option<[u8; REPORT_LEN]>,
    phase: Phase,
    commit: Commit,
    control: Vec<u8>,
    offset: usize,
    zero_packet: bool,
    control_toggle: bool,
    control_last: Option<([u8; 8], u8)>,
}

/// Single USB HID tablet with absolute X/Y axes and three buttons.
#[derive(Debug, Default)]
pub struct HidTablet {
    state: HidTabletState,
}

fn ack(bytes: &[u8]) -> UsbResult {
    let mut data = [0; 8];
    data[..bytes.len()].copy_from_slice(bytes);
    UsbResult::Ack {
        data,
        len: u8::try_from(bytes.len()).expect("USB packet is bounded to eight bytes"),
    }
}

fn valid_report(report: &[u8; REPORT_LEN]) -> bool {
    report[0] <= 7 && report[2] <= 0x7f && report[4] <= 0x7f && report[5] != 0x80
}

impl HidTablet {
    /// Create a tablet in the USB default state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restore validated device state.
    pub fn restore(state: &HidTabletState) -> io::Result<Self> {
        if state.address > 127
            || state.configuration > 1
            || state.idle_elapsed > 1020
            || state.reports.len() > QUEUE_LIMIT
            || !valid_report(&state.report)
            || state.report[5] != 0
            || state.reports.iter().any(|report| !valid_report(report))
            || state
                .interrupt_last
                .as_ref()
                .is_some_and(|report| !valid_report(report))
            || state.control.len() > 128
            || state.offset > state.control.len()
            || state.control_last.is_some_and(|(_, len)| len > 8)
            || matches!(state.commit, Commit::Address(address) if address > 127)
            || matches!(state.commit, Commit::Configuration(configuration) if configuration > 1)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid USB tablet state",
            ));
        }
        Ok(Self {
            state: state.clone(),
        })
    }

    /// Save device state.
    pub fn save(&self) -> HidTabletState {
        self.state.clone()
    }

    /// Reset USB addressing, configuration and endpoint state.
    pub fn reset(&mut self) {
        self.state = HidTabletState::default();
    }

    /// Return whether the guest has configured the tablet.
    pub fn configured(&self) -> bool {
        self.state.configuration == 1 && !self.state.halted
    }

    /// Advance HID idle time by one USB frame (one millisecond).
    pub fn frame_tick(&mut self) {
        self.state.idle_elapsed = self.state.idle_elapsed.saturating_add(1).min(1020);
    }

    /// Queue an absolute position and button state. Preserve button transitions.
    pub fn update(&mut self, x: u16, y: u16, buttons: u8, wheel: i8) -> bool {
        if x > 32767 || y > 32767 || buttons > 7 || wheel == i8::MIN || !self.configured() {
            return false;
        }
        let [xl, xh] = x.to_le_bytes();
        let [yl, yh] = y.to_le_bytes();
        let report = [buttons, xl, xh, yl, yh, wheel.to_le_bytes()[0]];
        if report == self.state.report {
            return true;
        }
        if let Some(last) = self
            .state
            .reports
            .back_mut()
            .filter(|last| last[0] == buttons && last[5] == 0 && wheel == 0)
        {
            *last = report;
        } else if self.state.reports.len() < QUEUE_LIMIT {
            self.state.reports.push_back(report);
        } else {
            return false;
        }
        self.state.report = [buttons, xl, xh, yl, yh, 0];
        true
    }

    /// Release all buttons while retaining the latest absolute position.
    pub fn release(&mut self) -> bool {
        self.state.report[0] = 0;
        self.state.reports.clear();
        if self.state.configuration == 1 {
            self.state.reports.push_back(self.state.report);
        }
        true
    }

    /// Execute a USB SETUP, IN or OUT transaction.
    pub fn transaction(
        &mut self,
        pid: u8,
        address: u8,
        endpoint: u8,
        toggle: bool,
        out: &[u8],
        max_in: usize,
    ) -> UsbResult {
        if address != self.state.address {
            return UsbResult::NoDevice;
        }
        match (pid, endpoint) {
            (0x2d, 0) if out.len() == 8 && !toggle => self.setup(out),
            (0x69, 0) => self.control_in(toggle, max_in),
            (0xe1, 0)
                if out.is_empty()
                    && toggle
                    && matches!(self.state.phase, Phase::StatusOut | Phase::CompletedOut) =>
            {
                self.state.phase = Phase::CompletedOut;
                ack(&[])
            }
            (0x69, 1) => self.interrupt_in(toggle, max_in),
            _ => UsbResult::Stall,
        }
    }

    fn setup(&mut self, setup: &[u8]) -> UsbResult {
        let request_type = setup[0];
        let request = setup[1];
        let value = u16::from_le_bytes([setup[2], setup[3]]);
        let index = u16::from_le_bytes([setup[4], setup[5]]);
        let length = u16::from_le_bytes([setup[6], setup[7]]);
        self.state.phase = Phase::Stalled;
        self.state.commit = Commit::None;
        self.state.control.clear();
        self.state.offset = 0;
        self.state.control_toggle = true;
        self.state.control_last = None;
        self.state.zero_packet = false;
        let data = match (request_type, request) {
            (0x80 | 0x81, 6) => self.descriptor(value, index, request_type),
            (0x80, 0) if value == 0 && index == 0 && length == 2 => Some(vec![0, 0]),
            (0x81, 0)
                if self.state.configuration == 1 && value == 0 && index == 0 && length == 2 =>
            {
                Some(vec![0, 0])
            }
            (0x82, 0) if value == 0 && (index == 0 || index == 0x81) && length == 2 => {
                Some(vec![u8::from(index == 0x81 && self.state.halted), 0])
            }
            (0x80, 8) if value == 0 && index == 0 && length == 1 => {
                Some(vec![self.state.configuration])
            }
            (0x81, 10)
                if self.state.configuration == 1 && value == 0 && index == 0 && length == 1 =>
            {
                Some(vec![0])
            }
            (0xa1, 1) if self.state.configuration == 1 && index == 0 && value == 0x100 => {
                Some(self.state.report.to_vec())
            }
            (0xa1, 2)
                if self.state.configuration == 1 && index == 0 && value == 0 && length == 1 =>
            {
                Some(vec![self.state.idle])
            }
            (0, 5)
                if value <= 127 && index == 0 && length == 0 && self.state.configuration == 0 =>
            {
                self.state.commit =
                    Commit::Address(u8::try_from(value).expect("validated address"));
                Some(Vec::new())
            }
            (0, 9) if value <= 1 && index == 0 && length == 0 => {
                self.state.commit =
                    Commit::Configuration(u8::try_from(value).expect("validated configuration"));
                Some(Vec::new())
            }
            (0x01, 11)
                if self.state.configuration == 1 && value == 0 && index == 0 && length == 0 =>
            {
                Some(Vec::new())
            }
            (0x02, 1 | 3)
                if self.state.configuration == 1 && value == 0 && index == 0x81 && length == 0 =>
            {
                self.state.commit = Commit::Halt(request == 3);
                Some(Vec::new())
            }
            (0x21, 10)
                if self.state.configuration == 1
                    && index == 0
                    && value & 0xff == 0
                    && length == 0 =>
            {
                self.state.commit = Commit::Idle(value.to_le_bytes()[1]);
                Some(Vec::new())
            }
            _ => None,
        };
        if let Some(mut data) = data {
            if request_type & 0x80 != 0 {
                self.state.zero_packet =
                    data.len() < usize::from(length) && data.len().is_multiple_of(8);
                data.truncate(usize::from(length));
                self.state.control = data;
                self.state.phase = if length == 0 {
                    Phase::StatusOut
                } else {
                    Phase::DataIn
                };
            } else {
                self.state.phase = Phase::StatusIn;
            }
        }
        // SETUP is acknowledged even when its data/status stage will STALL.
        ack(&[])
    }

    fn descriptor(&self, value: u16, index: u16, request_type: u8) -> Option<Vec<u8>> {
        match (value, request_type) {
            (0x100, 0x80) if index == 0 => Some(DEVICE_DESCRIPTOR.to_vec()),
            (0x200, 0x80) if index == 0 => Some(CONFIG_DESCRIPTOR.to_vec()),
            (0x300, 0x80) if index == 0 => Some(vec![4, 3, 9, 4]),
            (0x301 | 0x302, 0x80) if index == 0x409 => {
                let text = if value == 0x301 {
                    "Firecracker"
                } else {
                    "Absolute tablet"
                };
                let mut bytes = vec![0, 3];
                bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
                bytes[0] = u8::try_from(bytes.len()).expect("bounded USB string");
                Some(bytes)
            }
            (0x2100, 0x81) if index == 0 => Some(HID_DESCRIPTOR.to_vec()),
            (0x2200, 0x81) if index == 0 => Some(REPORT_DESCRIPTOR.to_vec()),
            _ => None,
        }
    }

    fn control_in(&mut self, toggle: bool, max_in: usize) -> UsbResult {
        if toggle != self.state.control_toggle {
            return self
                .state
                .control_last
                .map_or(UsbResult::Stall, |(data, len)| UsbResult::Ack { data, len });
        }
        let reply = match self.state.phase {
            Phase::DataIn => {
                let end = (self.state.offset + 8).min(self.state.control.len());
                let bytes = &self.state.control[self.state.offset..end];
                if max_in < bytes.len() {
                    return UsbResult::Stall;
                }
                let reply = ack(bytes);
                self.state.offset = end;
                if end == self.state.control.len() {
                    if bytes.is_empty() || !self.state.zero_packet {
                        self.state.phase = Phase::StatusOut;
                    } else {
                        self.state.zero_packet = false;
                    }
                }
                reply
            }
            Phase::StatusIn => {
                self.commit();
                self.state.phase = Phase::Idle;
                ack(&[])
            }
            _ => return UsbResult::Stall,
        };
        if let UsbResult::Ack { data, len } = reply {
            self.state.control_last = Some((data, len));
            self.state.control_toggle = !toggle;
        }
        reply
    }

    fn commit(&mut self) {
        match self.state.commit {
            Commit::None => {}
            Commit::Address(address) => self.state.address = address,
            Commit::Configuration(configuration) => {
                self.state.configuration = configuration;
                self.state.reports.clear();
                self.state.interrupt_toggle = false;
                self.state.interrupt_last = None;
                self.state.halted = false;
                if configuration == 1 {
                    self.state.reports.push_back(self.state.report);
                }
            }
            Commit::Idle(idle) => {
                self.state.idle = idle;
                self.state.idle_elapsed = 0;
            }
            Commit::Interface => {
                self.state.halted = false;
                self.state.interrupt_toggle = false;
                self.state.interrupt_last = None;
            }
            Commit::Halt(halt) => {
                self.state.halted = halt;
                if !halt {
                    self.state.interrupt_toggle = false;
                    self.state.interrupt_last = None;
                }
            }
        }
        self.state.commit = Commit::None;
    }

    fn interrupt_in(&mut self, toggle: bool, max_in: usize) -> UsbResult {
        if !self.configured() || max_in < REPORT_LEN {
            return UsbResult::Stall;
        }
        if toggle != self.state.interrupt_toggle {
            return self
                .state
                .interrupt_last
                .map_or(UsbResult::Nak, |report| ack(&report));
        }
        let report = if let Some(report) = self.state.reports.pop_front() {
            report
        } else if self.state.idle != 0 && self.state.idle_elapsed >= u16::from(self.state.idle) * 4
        {
            self.state.report
        } else {
            return UsbResult::Nak;
        };
        self.state.idle_elapsed = 0;
        self.state.interrupt_toggle = !toggle;
        self.state.interrupt_last = Some(report);
        ack(&report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(tablet: &mut HidTablet, bytes: [u8; 8]) {
        assert_eq!(
            tablet.transaction(0x2d, tablet.state.address, 0, false, &bytes, 0),
            ack(&[])
        );
    }

    fn configure(tablet: &mut HidTablet) {
        setup(tablet, [0, 9, 1, 0, 0, 0, 0, 0]);
        assert!(!tablet.configured());
        assert_eq!(tablet.transaction(0x69, 0, 0, true, &[], 0), ack(&[]));
        assert!(tablet.configured());
    }

    #[test]
    fn descriptors_and_address_status() {
        assert_eq!(REPORT_DESCRIPTOR.len(), usize::from(HID_DESCRIPTOR[7]));
        assert_eq!(&CONFIG_DESCRIPTOR[18..27], HID_DESCRIPTOR);
        assert_eq!(CONFIG_DESCRIPTOR.len(), usize::from(CONFIG_DESCRIPTOR[2]));
        let mut tablet = HidTablet::new();
        setup(&mut tablet, [0x80, 6, 0, 1, 0, 0, 18, 0]);
        for (i, chunk) in DEVICE_DESCRIPTOR.chunks(8).enumerate() {
            let toggle = i % 2 == 0;
            assert_eq!(tablet.transaction(0x69, 0, 0, toggle, &[], 8), ack(chunk));
            assert_eq!(tablet.transaction(0x69, 0, 0, toggle, &[], 8), ack(chunk));
        }
        assert_eq!(tablet.transaction(0xe1, 0, 0, true, &[], 0), ack(&[]));
        setup(&mut tablet, [0, 5, 7, 0, 0, 0, 0, 0]);
        assert_eq!(
            tablet.transaction(0x69, 7, 0, true, &[], 0),
            UsbResult::NoDevice
        );
        assert_eq!(tablet.transaction(0x69, 0, 0, true, &[], 0), ack(&[]));
        assert_eq!(
            tablet.transaction(0x69, 0, 0, true, &[], 0),
            UsbResult::NoDevice
        );
        assert_eq!(tablet.state.address, 7);
    }

    #[test]
    fn absolute_reports_retries_and_button_order() {
        let mut tablet = HidTablet::new();
        configure(&mut tablet);
        assert_eq!(tablet.transaction(0x69, 0, 1, false, &[], 8), ack(&[0; 6]));
        assert!(tablet.update(100, 200, 1, 0));
        assert!(tablet.update(32767, 32767, 1, 0));
        assert!(tablet.update(32767, 32767, 0, 0));
        let pressed = [1, 0xff, 0x7f, 0xff, 0x7f, 0];
        assert_eq!(tablet.transaction(0x69, 0, 1, true, &[], 8), ack(&pressed));
        assert_eq!(tablet.transaction(0x69, 0, 1, true, &[], 8), ack(&pressed));
        assert_eq!(
            tablet.transaction(0x69, 0, 1, false, &[], 8),
            ack(&[0, 0xff, 0x7f, 0xff, 0x7f, 0])
        );
        assert_eq!(tablet.transaction(0x69, 0, 1, true, &[], 8), UsbResult::Nak);
        assert!(!tablet.update(32768, 0, 0, 0));
    }

    #[test]
    fn stalled_request_recovers_and_snapshot_preserves_transfer() {
        let mut tablet = HidTablet::new();
        setup(&mut tablet, [0x80, 6, 0, 0xff, 0, 0, 8, 0]);
        assert_eq!(
            tablet.transaction(0x69, 0, 0, true, &[], 8),
            UsbResult::Stall
        );
        setup(&mut tablet, [0x81, 6, 0, 0x22, 0, 0, 255, 0]);
        assert_eq!(
            tablet.transaction(0x69, 0, 0, true, &[], 8),
            ack(&REPORT_DESCRIPTOR[..8])
        );
        let mut restored = HidTablet::restore(&tablet.save()).unwrap();
        assert_eq!(
            restored.transaction(0x69, 0, 0, false, &[], 8),
            ack(&REPORT_DESCRIPTOR[8..16])
        );
        let mut invalid = tablet.save();
        invalid.offset = 129;
        HidTablet::restore(&invalid).unwrap_err();
        invalid = tablet.save();
        invalid.reports.resize(QUEUE_LIMIT + 1, [0; 6]);
        HidTablet::restore(&invalid).unwrap_err();
    }

    #[test]
    fn control_short_transfer_needs_zero_packet_and_new_setup_aborts_commit() {
        let mut tablet = HidTablet::new();
        setup(&mut tablet, [0, 5, 7, 0, 0, 0, 0, 0]);
        setup(&mut tablet, [0x80, 6, 1, 3, 9, 4, 32, 0]);
        for toggle in [true, false, true] {
            let UsbResult::Ack { len: 8, .. } = tablet.transaction(0x69, 0, 0, toggle, &[], 8)
            else {
                panic!("expected full descriptor packet");
            };
        }
        assert_eq!(tablet.transaction(0x69, 0, 0, false, &[], 8), ack(&[]));
        assert_eq!(tablet.transaction(0xe1, 0, 0, true, &[], 0), ack(&[]));
        assert_eq!(tablet.state.address, 0);
    }

    #[test]
    fn endpoint_halt_can_be_queried_and_cleared() {
        let mut tablet = HidTablet::new();
        configure(&mut tablet);
        setup(&mut tablet, [2, 3, 0, 0, 0x81, 0, 0, 0]);
        assert_eq!(tablet.transaction(0x69, 0, 0, true, &[], 0), ack(&[]));
        assert_eq!(
            tablet.transaction(0x69, 0, 1, false, &[], 8),
            UsbResult::Stall
        );
        setup(&mut tablet, [0x82, 0, 0, 0, 0x81, 0, 2, 0]);
        assert_eq!(tablet.transaction(0x69, 0, 0, true, &[], 8), ack(&[1, 0]));
        setup(&mut tablet, [2, 1, 0, 0, 0x81, 0, 0, 0]);
        assert_eq!(tablet.transaction(0x69, 0, 0, true, &[], 0), ack(&[]));
        assert_eq!(tablet.transaction(0x69, 0, 1, false, &[], 8), ack(&[0; 6]));
    }

    #[test]
    fn wheel_is_relative_and_not_replayed_by_idle_or_get_report() {
        let mut tablet = HidTablet::new();
        configure(&mut tablet);
        assert_eq!(tablet.transaction(0x69, 0, 1, false, &[], 8), ack(&[0; 6]));
        assert!(tablet.update(0, 0, 0, 1));
        assert!(tablet.update(0, 0, 0, -1));
        assert_eq!(
            tablet.transaction(0x69, 0, 1, true, &[], 8),
            ack(&[0, 0, 0, 0, 0, 1])
        );
        assert_eq!(
            tablet.transaction(0x69, 0, 1, false, &[], 8),
            ack(&[0, 0, 0, 0, 0, 255])
        );
        setup(&mut tablet, [0xa1, 1, 0, 1, 0, 0, 6, 0]);
        assert_eq!(tablet.transaction(0x69, 0, 0, true, &[], 8), ack(&[0; 6]));
        setup(&mut tablet, [0x21, 10, 0, 1, 0, 0, 0, 0]);
        assert_eq!(tablet.transaction(0x69, 0, 0, true, &[], 0), ack(&[]));
        for _ in 0..4 {
            tablet.frame_tick();
        }
        assert_eq!(tablet.transaction(0x69, 0, 1, true, &[], 8), ack(&[0; 6]));
        assert!(!tablet.update(0, 0, 0, -128));
    }

    #[test]
    fn idle_rate_and_queue_backpressure() {
        let mut tablet = HidTablet::new();
        configure(&mut tablet);
        assert_eq!(tablet.transaction(0x69, 0, 1, false, &[], 8), ack(&[0; 6]));
        setup(&mut tablet, [0x21, 10, 0, 1, 0, 0, 0, 0]);
        assert_eq!(tablet.transaction(0x69, 0, 0, true, &[], 0), ack(&[]));
        for _ in 0..3 {
            tablet.frame_tick();
        }
        assert_eq!(tablet.transaction(0x69, 0, 1, true, &[], 8), UsbResult::Nak);
        tablet.frame_tick();
        assert_eq!(tablet.transaction(0x69, 0, 1, true, &[], 8), ack(&[0; 6]));
        for i in 0..QUEUE_LIMIT {
            assert!(tablet.update(1, 2, u8::try_from(i % 2 + 1).unwrap(), 0));
        }
        assert!(!tablet.update(1, 2, 0, 0));
        assert!(tablet.release());
        assert_eq!(tablet.state.reports.len(), 1);
        assert_eq!(tablet.state.reports[0], [0, 1, 0, 2, 0, 0]);
        tablet.reset();
        assert!(tablet.release());
        assert!(!tablet.configured());
        assert!(tablet.state.reports.is_empty());
    }
}
