// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Read-only ATAPI optical media with bounded PIO transfers.

#[cfg(feature = "windows")]
use std::fs::File;
#[cfg(feature = "windows")]
use std::io;
#[cfg(feature = "windows")]
use std::os::unix::fs::FileExt;

use serde::{Deserialize, Serialize};
#[cfg(feature = "windows")]
use vmm_sys_util::eventfd::EventFd;

/// Primary IDE command-block port.
#[cfg(feature = "windows")]
pub const COMMAND_PORT: u64 = 0x1f0;
/// Primary IDE alternate-status/device-control port.
#[cfg(feature = "windows")]
pub const CONTROL_PORT: u64 = 0x3f6;
/// Primary IDE interrupt.
#[cfg(feature = "windows")]
pub const IRQ: u32 = 14;
#[cfg(feature = "windows")]
const BLOCK_SIZE: u64 = 2048;
#[cfg(feature = "windows")]
const MAX_TRANSFER: usize = 65534;
#[cfg(feature = "windows")]
const READY: u8 = 0x40;
#[cfg(feature = "windows")]
const DRQ: u8 = 8;
#[cfg(feature = "windows")]
const ERROR: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Transfer {
    Idle,
    Packet {
        bytes: [u8; 12],
        position: usize,
    },
    Data {
        bytes: Vec<u8>,
        position: usize,
        next_offset: u64,
        remaining: u64,
        limit: usize,
        packet: bool,
        phase_remaining: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MediaState {
    identity: String,
    size: u64,
}

/// Controller state; media identity must be a verified digest of the opened ISO.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtapiState {
    media: Option<MediaState>,
    tray_open: bool,
    locked: bool,
    registers: [u8; 8],
    control: u8,
    features: u8,
    pending_irq: bool,
    sense: [u8; 3],
    transfer: Transfer,
}

#[cfg(feature = "windows")]
impl AtapiState {
    fn media_size(&self) -> u64 {
        self.media.as_ref().map_or(0, |media| media.size)
    }

    fn media_present(&self) -> bool {
        self.media.is_some() && !self.tray_open
    }
}

/// One read-only optical master; the slave is absent. DMA is not advertised.
#[derive(Debug)]
#[cfg(feature = "windows")]
pub struct Atapi {
    media: Option<File>,
    irq: EventFd,
    legacy_irq_enabled: bool,
    state: AtapiState,
}

#[cfg(feature = "windows")]
impl Atapi {
    /// Creates a drive with optional verified media and a closed tray.
    pub fn new(media: Option<(File, String)>, irq: EventFd) -> io::Result<Self> {
        let (media, media_state) = match media {
            Some((media, identity)) => {
                let metadata = media.metadata()?;
                let size = metadata.len();
                if !metadata.is_file()
                    || size == 0
                    || !size.is_multiple_of(BLOCK_SIZE)
                    || size / BLOCK_SIZE > u64::from(u32::MAX)
                    || identity.is_empty()
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid ISO media",
                    ));
                }
                (Some(media), Some(MediaState { identity, size }))
            }
            None => (None, None),
        };
        let mut device = Self {
            media,
            irq,
            legacy_irq_enabled: true,
            state: AtapiState {
                media: media_state,
                tray_open: false,
                locked: false,
                registers: [0; 8],
                control: 0,
                features: 0,
                pending_irq: false,
                sense: [0; 3],
                transfer: Transfer::Idle,
            },
        };
        device.reset();
        Ok(device)
    }

    /// Selects the fixed legacy IRQ; native PCI delivery is managed by the wrapper.
    pub fn set_legacy_irq_enabled(&mut self, enabled: bool) {
        if enabled
            && !self.legacy_irq_enabled
            && self.interrupt_pending()
            && let Err(error) = self.irq.write(1)
        {
            crate::logger::error!("ATAPI interrupt: {error}");
        }
        self.legacy_irq_enabled = enabled;
    }

    /// Reports the interrupt level after the ATA device-control mask.
    pub fn interrupt_pending(&self) -> bool {
        self.state.pending_irq && self.state.control & 2 == 0
    }

    /// Ejects idle media unless the guest has prevented removal.
    pub fn eject(&mut self) -> io::Result<()> {
        if self.state.locked {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "optical media is locked",
            ));
        }
        if !matches!(self.state.transfer, Transfer::Idle) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "optical transfer is active",
            ));
        }
        self.state.tray_open = true;
        Ok(())
    }

    /// Saves in-flight PIO data, task-file registers and media state.
    pub fn save(&self) -> AtapiState {
        self.state.clone()
    }

    /// Restores against matching optional media and valid transfer bounds.
    pub fn restore(
        media: Option<(File, String)>,
        irq: EventFd,
        state: &AtapiState,
    ) -> io::Result<Self> {
        let mut device = Self::new(media, irq)?;
        let valid_transfer = match &state.transfer {
            Transfer::Idle => true,
            Transfer::Packet { position, .. } => *position < 12,
            Transfer::Data {
                bytes,
                position,
                next_offset,
                remaining,
                limit,
                phase_remaining,
                ..
            } => {
                !bytes.is_empty()
                    && bytes.len() <= MAX_TRANSFER
                    && *position < bytes.len()
                    && *phase_remaining > 0
                    && *phase_remaining <= (bytes.len() - position).min(*limit)
                    && (2..=MAX_TRANSFER).contains(limit)
                    && next_offset
                        .checked_add(*remaining)
                        .is_some_and(|end| end <= state.media_size())
            }
        };
        if state.media != device.state.media || !valid_transfer {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ATAPI media or state mismatch",
            ));
        }
        device.state = state.clone();
        Ok(device)
    }

    fn reset(&mut self) {
        self.state.registers = [0, 1, 1, 1, 0x14, 0xeb, 0xa0, READY];
        self.state.features = 0;
        self.state.pending_irq = false;
        self.state.transfer = Transfer::Idle;
    }

    fn selected(&self) -> bool {
        self.state.registers[6] & 0x10 == 0
    }

    fn interrupt(&mut self) {
        if !self.state.pending_irq {
            self.state.pending_irq = true;
            if self.legacy_irq_enabled
                && self.state.control & 2 == 0
                && let Err(error) = self.irq.write(1)
            {
                crate::logger::error!("ATAPI interrupt: {error}");
            }
        }
    }

    fn complete(&mut self) {
        self.state.transfer = Transfer::Idle;
        self.state.registers[2] = 3;
        self.state.registers[7] = READY;
        self.interrupt();
    }

    fn fail(&mut self, key: u8, asc: u8) {
        self.state.sense = [key, asc, 0];
        self.state.registers[1] = key << 4;
        self.complete();
        self.state.registers[7] |= ERROR;
    }

    /// Reads command-block registers; only status acknowledges the interrupt.
    pub fn read(&mut self, offset: u64, data: &mut [u8]) {
        data.fill(0xff);
        if !self.selected() {
            data.fill(0);
            return;
        }
        if offset == 0 && matches!(data.len(), 1 | 2 | 4) {
            self.read_data(data);
        } else if data.len() == 1 && offset < 8 {
            data[0] = self.state.registers[usize::try_from(offset).unwrap()];
            if offset == 7 {
                self.state.pending_irq = false;
            }
        }
    }

    /// Writes command-block registers or packet words.
    pub fn write(&mut self, offset: u64, data: &[u8]) {
        if offset == 6 && data.len() == 1 {
            self.state.registers[6] = data[0];
        } else if !self.selected() || self.state.control & 4 != 0 {
        } else if offset == 0 && matches!(data.len(), 1 | 2 | 4) {
            if let Transfer::Packet { bytes, position } = &mut self.state.transfer {
                let count = data.len().min(bytes.len() - *position);
                bytes[*position..*position + count].copy_from_slice(&data[..count]);
                *position += count;
                if *position == 12 {
                    let packet = *bytes;
                    self.execute_packet(packet);
                }
            }
        } else if data.len() == 1 {
            match offset {
                1 => self.state.features = data[0],
                2..=5 => self.state.registers[usize::try_from(offset).unwrap()] = data[0],
                7 => self.command(data[0]),
                _ => {}
            }
        }
    }

    /// Reads alternate status without acknowledging the interrupt.
    pub fn read_control(&self) -> u8 {
        if self.selected() {
            self.state.registers[7]
        } else {
            0
        }
    }

    /// Handles interrupt masking and the rising/falling software-reset edges.
    pub fn write_control(&mut self, value: u8) {
        let old = self.state.control;
        self.state.control = value;
        if value & 4 != 0 {
            self.state.registers[7] = 0x80;
            self.state.transfer = Transfer::Idle;
            self.state.pending_irq = false;
        } else if old & 4 != 0 {
            self.reset();
        } else if old & 2 != 0 && value & 2 == 0 && self.state.pending_irq {
            self.state.pending_irq = false;
            self.interrupt();
        }
    }

    fn command(&mut self, command: u8) {
        self.state.pending_irq = false;
        self.state.registers[1] = 0;
        self.state.transfer = Transfer::Idle;
        match command {
            0xa0 if self.state.features == 0
                && u16::from_le_bytes([self.state.registers[4], self.state.registers[5]]) != 1 =>
            {
                self.state.registers[2] = 1;
                self.state.registers[7] = READY | DRQ;
                self.state.transfer = Transfer::Packet {
                    bytes: [0; 12],
                    position: 0,
                };
                // IDENTIFY advertises a polling command-packet handshake.
            }
            0xa1 => self.identify(),
            0x08 | 0x90 => {
                self.reset();
                if command == 0x90 {
                    self.interrupt();
                }
            }
            0xec => {
                self.reset();
                self.state.registers[1] = 4;
                self.state.registers[7] = READY | ERROR;
                self.interrupt();
            }
            0xef if self.state.features == 3
                && (self.state.registers[2] == 0
                    || (8..=12).contains(&self.state.registers[2])) =>
            {
                self.complete()
            }
            0xe5 => {
                self.complete();
                self.state.registers[2] = 0xff;
            }
            _ => {
                self.fail(5, 0x20);
                self.state.registers[1] = 4;
            }
        }
    }

    fn identify(&mut self) {
        let mut bytes = vec![0; 512];
        for (word, value) in [
            (0, 0x8580u16),
            (49, 0x0200),
            (53, 2),
            (64, 3),
            (67, 120),
            (68, 120),
            (80, 0x007e),
        ] {
            bytes[word * 2..word * 2 + 2].copy_from_slice(&value.to_le_bytes());
        }
        for (word, text, length) in [
            (10, "FC-CDROM", 20),
            (23, "1.0", 8),
            (27, "Firecracker read-only optical drive", 40),
        ] {
            let field = &mut bytes[word * 2..word * 2 + length];
            field.fill(b' ');
            field[..text.len()].copy_from_slice(text.as_bytes());
            for pair in field.as_chunks_mut::<2>().0 {
                pair.swap(0, 1);
            }
        }
        self.state.transfer = Transfer::Data {
            bytes,
            position: 0,
            next_offset: 0,
            remaining: 0,
            limit: 512,
            packet: false,
            phase_remaining: 512,
        };
        self.state.registers[7] = READY | DRQ;
        self.interrupt();
    }

    fn transfer_limit(&self) -> usize {
        let limit = usize::from(u16::from_le_bytes([
            self.state.registers[4],
            self.state.registers[5],
        ]));
        if limit == 0 {
            MAX_TRANSFER
        } else {
            limit.clamp(2, MAX_TRANSFER) & !1
        }
    }

    fn response(&mut self, mut bytes: Vec<u8>, allocation: usize) {
        bytes.truncate(allocation);
        if bytes.is_empty() {
            self.complete();
            return;
        }
        let limit = self.transfer_limit();
        self.state.transfer = Transfer::Data {
            bytes,
            position: 0,
            next_offset: 0,
            remaining: 0,
            limit,
            packet: true,
            phase_remaining: 0,
        };
        self.data_phase();
    }

    fn data_phase(&mut self) {
        if let Transfer::Data {
            bytes,
            position,
            limit,
            phase_remaining,
            ..
        } = &mut self.state.transfer
        {
            *phase_remaining = (bytes.len() - *position).min(*limit);
            let count = u16::try_from(*phase_remaining).unwrap();
            [self.state.registers[4], self.state.registers[5]] = count.to_le_bytes();
            self.state.registers[2] = 2;
            self.state.registers[7] = READY | DRQ;
            self.interrupt();
        }
    }

    fn read_data(&mut self, data: &mut [u8]) {
        data.fill(0);
        let Transfer::Data {
            bytes,
            position,
            phase_remaining,
            ..
        } = &mut self.state.transfer
        else {
            return;
        };
        let count = data.len().min(*phase_remaining);
        data[..count].copy_from_slice(&bytes[*position..*position + count]);
        *position += count;
        *phase_remaining -= count;
        if *phase_remaining != 0 {
            return;
        }
        if *position < bytes.len() {
            self.data_phase();
        } else {
            let transfer = std::mem::replace(&mut self.state.transfer, Transfer::Idle);
            if let Transfer::Data {
                next_offset,
                remaining,
                limit,
                packet,
                ..
            } = transfer
            {
                if remaining > 0 {
                    self.read_media(next_offset, remaining, limit);
                } else if packet {
                    self.complete();
                } else {
                    self.state.registers[7] = READY;
                }
            }
        }
    }

    fn read_media(&mut self, offset: u64, remaining: u64, limit: usize) {
        let count = usize::try_from(remaining.min(limit as u64)).unwrap();
        let mut bytes = vec![0; count];
        if self
            .media
            .as_ref()
            .unwrap()
            .read_exact_at(&mut bytes, offset)
            .is_err()
        {
            self.fail(3, 0x11);
            return;
        }
        self.state.transfer = Transfer::Data {
            bytes,
            position: 0,
            next_offset: offset + count as u64,
            remaining: remaining - count as u64,
            limit,
            packet: true,
            phase_remaining: 0,
        };
        self.data_phase();
    }

    fn execute_packet(&mut self, packet: [u8; 12]) {
        self.state.transfer = Transfer::Idle;
        if packet[1] & 0xe0 != 0 {
            self.fail(5, 0x25);
            return;
        }
        if !self.state.media_present()
            && matches!(packet[0], 0x00 | 0x25 | 0x28 | 0xa8 | 0x43 | 0x2b)
        {
            self.fail(2, 0x3a);
            return;
        }
        match packet[0] {
            0x00 | 0x2b | 0xbb => self.complete(),
            0x03 => {
                let [key, asc, ascq] = self.state.sense;
                let mut bytes = vec![0; 18];
                bytes[0] = 0x70;
                bytes[2] = key;
                bytes[7] = 10;
                bytes[12] = asc;
                bytes[13] = ascq;
                self.state.sense = [0; 3];
                self.response(bytes, usize::from(packet[4]));
            }
            0x12 if packet[1] & 1 == 0 && packet[2] == 0 => {
                let mut bytes = vec![0; 36];
                bytes[..8].copy_from_slice(&[5, 0x80, 0, 0x21, 31, 0, 0, 0]);
                bytes[8..16].copy_from_slice(b"FC      ");
                bytes[16..32].copy_from_slice(b"Virtual CD-ROM  ");
                bytes[32..36].copy_from_slice(b"1.0 ");
                self.response(bytes, usize::from(packet[4]));
            }
            0x1b => {
                if packet[4] & 2 != 0 {
                    if packet[4] & 1 == 0 && self.state.locked {
                        self.fail(5, 0x53);
                        return;
                    }
                    self.state.tray_open = packet[4] & 1 == 0;
                }
                self.complete();
            }
            0x1e => {
                self.state.locked = packet[4] & 1 != 0;
                self.complete();
            }
            0x25 => {
                let last = u32::try_from(self.state.media_size() / BLOCK_SIZE - 1).unwrap();
                let mut bytes = last.to_be_bytes().to_vec();
                bytes.extend_from_slice(&u32::try_from(BLOCK_SIZE).unwrap().to_be_bytes());
                self.response(bytes, 8);
            }
            0x28 | 0xa8 => {
                let lba = u32::from_be_bytes(packet[2..6].try_into().unwrap());
                let blocks = if packet[0] == 0x28 {
                    u32::from(u16::from_be_bytes([packet[7], packet[8]]))
                } else {
                    u32::from_be_bytes(packet[6..10].try_into().unwrap())
                };
                let offset = u64::from(lba) * BLOCK_SIZE;
                let length = u64::from(blocks) * BLOCK_SIZE;
                if offset + length > self.state.media_size() {
                    self.fail(5, 0x21);
                } else if length == 0 {
                    self.complete();
                } else {
                    self.read_media(offset, length, self.transfer_limit());
                }
            }
            0x1a | 0x5a => self.mode_sense(packet),
            0x43 => self.read_toc(packet),
            0x4a if packet[1] & 1 != 0 => {
                let bytes = if packet[4] & 0x10 != 0 {
                    vec![
                        0,
                        6,
                        4,
                        0x10,
                        0,
                        u8::from(self.state.tray_open)
                            | (u8::from(self.state.media_present()) << 1),
                        0,
                        0,
                    ]
                } else {
                    vec![0, 2, 0x80, 0x10]
                };
                self.response(
                    bytes,
                    usize::from(u16::from_be_bytes([packet[7], packet[8]])),
                );
            }
            0x2a | 0xaa => self.fail(7, 0x27),
            _ => self.fail(5, 0x20),
        }
    }

    fn mode_sense(&mut self, packet: [u8; 12]) {
        let page = packet[2] & 0x3f;
        if !matches!(page, 0x01 | 0x2a | 0x3f) || packet[2] >> 6 == 3 {
            self.fail(5, 0x24);
            return;
        }
        let ten = packet[0] == 0x5a;
        let mut bytes = vec![0; if ten { 8 } else { 4 }];
        bytes[if ten { 3 } else { 2 }] = 0x80;
        if page == 1 || page == 0x3f {
            bytes.extend_from_slice(&[1, 6, 0, 0, 0, 0, 0, 0]);
        }
        if page == 0x2a || page == 0x3f {
            let mut capabilities = [0; 20];
            capabilities[..8].copy_from_slice(&[0x2a, 18, 0, 0, 0, 0, 0x29, 0]);
            capabilities[8..10].copy_from_slice(&706u16.to_be_bytes());
            capabilities[14..16].copy_from_slice(&706u16.to_be_bytes());
            if packet[2] >> 6 == 1 {
                capabilities[2..].fill(0);
            }
            bytes.extend_from_slice(&capabilities);
        }
        let allocation = if ten {
            let length = u16::try_from(bytes.len() - 2).unwrap();
            bytes[..2].copy_from_slice(&length.to_be_bytes());
            usize::from(u16::from_be_bytes([packet[7], packet[8]]))
        } else {
            bytes[0] = u8::try_from(bytes.len() - 1).unwrap();
            usize::from(packet[4])
        };
        self.response(bytes, allocation);
    }

    fn read_toc(&mut self, packet: [u8; 12]) {
        if packet[2] & 0xf == 1 {
            // MMC-1 5.1.13.3: one complete session, first data track at LBA 0.
            // The session selector is reserved in this format.
            if packet[6] != 0 {
                self.fail(5, 0x24);
                return;
            }
            let mut bytes = vec![0, 10, 1, 1, 0, 0x14, 1, 0, 0, 0, 0, 0];
            if packet[1] & 2 != 0 {
                bytes[10] = 2; // LBA zero is MSF 00:02:00.
            }
            self.response(
                bytes,
                usize::from(u16::from_be_bytes([packet[7], packet[8]])),
            );
            return;
        }
        if packet[2] & 0xf != 0 || !matches!(packet[6], 0 | 1 | 0xaa) {
            self.fail(5, 0x24);
            return;
        }
        let mut bytes = vec![0, 0, 1, 1];
        for (track, lba) in [(1, 0), (0xaa, self.state.media_size() / BLOCK_SIZE)] {
            if packet[6] == 0xaa && track == 1 {
                continue;
            }
            bytes.extend_from_slice(&[0, 0x14, track, 0]);
            if packet[1] & 2 != 0 {
                let sectors = lba + 150;
                if sectors / (75 * 60) > u64::from(u8::MAX) {
                    self.fail(5, 0x24);
                    return;
                }
                bytes.extend_from_slice(&[
                    0,
                    u8::try_from(sectors / (75 * 60)).unwrap(),
                    u8::try_from(sectors / 75 % 60).unwrap(),
                    u8::try_from(sectors % 75).unwrap(),
                ]);
            } else {
                bytes.extend_from_slice(&u32::try_from(lba).unwrap().to_be_bytes());
            }
        }
        let length = u16::try_from(bytes.len() - 2).unwrap();
        bytes[..2].copy_from_slice(&length.to_be_bytes());
        self.response(
            bytes,
            usize::from(u16::from_be_bytes([packet[7], packet[8]])),
        );
    }
}

#[cfg(test)]
#[cfg(feature = "windows")]
mod tests {
    use std::io::Write;

    use super::*;

    fn fixture() -> (Atapi, EventFd, Vec<u8>) {
        let mut file = memfd::MemfdOptions::default()
            .create("atapi-test")
            .unwrap()
            .into_file();
        let bytes: Vec<u8> = (0..8192)
            .map(|index| u8::try_from(index % 251).unwrap())
            .collect();
        file.write_all(&bytes).unwrap();
        let irq = EventFd::new(libc::EFD_NONBLOCK).unwrap();
        (
            Atapi::new(
                Some((file, "verified-test-media".into())),
                irq.try_clone().unwrap(),
            )
            .unwrap(),
            irq,
            bytes,
        )
    }

    fn status(device: &mut Atapi) -> u8 {
        let mut byte = [0];
        device.read(7, &mut byte);
        byte[0]
    }

    fn packet(device: &mut Atapi, command: &[u8], limit: u16) {
        let [low, high] = limit.to_le_bytes();
        device.write(4, &[low]);
        device.write(5, &[high]);
        device.write(7, &[0xa0]);
        assert_eq!(status(device), READY | DRQ);
        let mut bytes = [0; 12];
        bytes[..command.len()].copy_from_slice(command);
        for word in bytes.as_chunks::<2>().0 {
            device.write(0, word);
        }
    }

    fn receive(device: &mut Atapi) -> Vec<u8> {
        let mut bytes = Vec::new();
        while status(device) & DRQ != 0 {
            let count = u16::from_le_bytes([device.state.registers[4], device.state.registers[5]]);
            assert!(count > 0);
            let mut remaining = usize::from(count);
            while remaining > 0 {
                let mut word = [0; 2];
                device.read(0, &mut word);
                bytes.extend_from_slice(&word[..remaining.min(2)]);
                remaining = remaining.saturating_sub(2);
            }
        }
        bytes
    }

    #[test]
    fn empty_drive_identifies_and_restores_without_inventing_media() {
        let irq = || EventFd::new(libc::EFD_NONBLOCK).unwrap();
        let mut device = Atapi::new(None, irq()).unwrap();
        packet(&mut device, &[0x12, 0, 0, 0, 36], 2048);
        let inquiry = receive(&mut device);
        assert_eq!(&inquiry[8..16], b"FC      ");
        for load in [0, 3] {
            packet(&mut device, &[0x1b, 0, 0, 0, load], 2048);
            packet(&mut device, &[0], 2048);
            assert_eq!(status(&mut device), READY | ERROR);
            assert_eq!(device.state.sense, [2, 0x3a, 0]);
        }
        packet(&mut device, &[0x4a, 1, 0, 0, 0x10, 0, 0, 0, 8], 2048);
        assert_eq!(receive(&mut device)[5], 0);
        let saved: AtapiState =
            bitcode::deserialize(&bitcode::serialize(&device.save()).unwrap()).unwrap();
        let mut restored = Atapi::restore(None, irq(), &saved).unwrap();
        packet(&mut restored, &[0x25], 2048);
        assert_eq!(status(&mut restored), READY | ERROR);
        let (loaded, _, _) = fixture();
        Atapi::restore(
            loaded
                .media
                .map(|file| (file, "verified-test-media".into())),
            irq(),
            &saved,
        )
        .unwrap_err();
        Atapi::restore(None, irq(), &loaded.state).unwrap_err();
    }

    #[test]
    fn native_delivery_tracks_level_without_legacy_pulses() {
        let (mut device, irq, _) = fixture();
        device.set_legacy_irq_enabled(false);
        device.write(7, &[0xa1]);
        irq.read().unwrap_err();
        assert!(device.interrupt_pending());
        device.read_control();
        assert!(device.interrupt_pending());
        status(&mut device);
        assert!(!device.interrupt_pending());
        device.write(7, &[0xa1]);
        device.write_control(2);
        assert!(!device.interrupt_pending());
        device.write_control(0);
        assert!(device.interrupt_pending());
        irq.read().unwrap_err();
        device.set_legacy_irq_enabled(true);
        assert_eq!(irq.read().unwrap(), 1);
    }

    #[test]
    fn signature_identify_and_absent_slave() {
        let (mut device, irq, _) = fixture();
        assert_eq!(&device.state.registers[2..6], &[1, 1, 0x14, 0xeb]);
        device.write(6, &[0xb0]);
        assert_eq!(status(&mut device), 0);
        device.write(7, &[0xa1]);
        irq.read().unwrap_err();
        device.write(6, &[0xa0]);
        device.write(7, &[0xa1]);
        assert_eq!(irq.read().unwrap(), 1);
        assert_eq!(status(&mut device), READY | DRQ);
        let mut identify = vec![0; 512];
        for word in identify.as_chunks_mut::<2>().0 {
            device.read(0, word);
        }
        assert_eq!(u16::from_le_bytes([identify[0], identify[1]]), 0x8580);
        assert_eq!(identify[99] & 1, 0); // No DMA.
        assert_eq!(status(&mut device), READY);
        device.write_control(4);
        assert_eq!(device.read_control(), 0x80);
        device.write_control(0);
        assert_eq!(&device.state.registers[2..6], &[1, 1, 0x14, 0xeb]);
    }

    #[test]
    fn ata_identify_abort_preserves_packet_device_signature() {
        let (mut device, irq, _) = fixture();
        device.write(7, &[0xec]);
        assert_eq!(status(&mut device), READY | ERROR);
        assert_eq!(device.state.registers[1], 4);
        assert_eq!(&device.state.registers[2..6], &[1, 1, 0x14, 0xeb]);
        assert_eq!(irq.read().unwrap(), 1);
    }

    #[test]
    fn reads_stream_across_odd_host_limits_without_changing_phase_count() {
        let (mut device, _, bytes) = fixture();
        packet(&mut device, &[0x28, 0, 0, 0, 0, 1, 0, 0, 2], 513);
        assert_eq!(device.state.registers[4..6], [0, 2]);
        let mut first = [0; 2];
        device.read(0, &mut first);
        assert_eq!(device.state.registers[4..6], [0, 2]);
        // Restart to exercise the public register protocol from a full phase.
        packet(&mut device, &[0xa8, 0, 0, 0, 0, 1, 0, 0, 0, 2], 513);
        assert_eq!(receive(&mut device), bytes[2048..6144]);
        assert_eq!(status(&mut device), READY);
    }

    #[test]
    fn irq_mask_and_alternate_status_preserve_pending_interrupt() {
        let (mut device, irq, _) = fixture();
        device.write_control(2);
        packet(&mut device, &[0], 2048);
        irq.read().unwrap_err();
        assert_eq!(device.read_control(), READY);
        assert!(device.state.pending_irq);
        device.write_control(0);
        assert_eq!(irq.read().unwrap(), 1);
        assert_eq!(status(&mut device), READY);
        assert!(!device.state.pending_irq);
    }

    #[test]
    fn restore_mid_phase_and_reject_swapped_media() {
        let (mut device, _, bytes) = fixture();
        packet(&mut device, &[0x28, 0, 0, 0, 0, 0, 0, 0, 4], 2048);
        let mut prefix = [0; 4];
        device.read(0, &mut prefix);
        let state: AtapiState =
            serde_json::from_slice(&serde_json::to_vec(&device.save()).unwrap()).unwrap();
        let make_irq = || EventFd::new(libc::EFD_NONBLOCK).unwrap();
        Atapi::restore(
            Some((
                device.media.as_ref().unwrap().try_clone().unwrap(),
                "other-digest".into(),
            )),
            make_irq(),
            &state,
        )
        .unwrap_err();
        let mut restored = Atapi::restore(
            Some((
                device.media.as_ref().unwrap().try_clone().unwrap(),
                "verified-test-media".into(),
            )),
            make_irq(),
            &state,
        )
        .unwrap();
        let mut tail = Vec::new();
        for _ in 0..(8192 - 4) / 2 {
            status(&mut restored);
            let mut word = [0; 2];
            restored.read(0, &mut word);
            tail.extend_from_slice(&word);
        }
        assert_eq!(tail, bytes[4..]);
        assert_eq!(status(&mut restored), READY);
        let mut invalid = state;
        if let Transfer::Data { position, .. } = &mut invalid.transfer {
            *position = usize::MAX;
        }
        Atapi::restore(
            Some((
                device.media.as_ref().unwrap().try_clone().unwrap(),
                "verified-test-media".into(),
            )),
            make_irq(),
            &invalid,
        )
        .unwrap_err();
    }

    #[test]
    fn errors_report_sense_and_never_write_media() {
        let (mut device, _, original) = fixture();
        for (command, key, asc) in [
            ([0x28, 0, 0xff, 0xff, 0xff, 0xff, 0, 0xff, 0xff], 5, 0x21),
            ([0x2a, 0, 0, 0, 0, 0, 0, 0, 1], 7, 0x27),
        ] {
            packet(&mut device, &command, 2048);
            assert_eq!(status(&mut device), READY | ERROR);
            packet(&mut device, &[3, 0, 0, 0, 18], 2048);
            let sense = receive(&mut device);
            assert_eq!((sense[2], sense[12]), (key, asc));
        }
        let mut actual = vec![0; original.len()];
        device
            .media
            .as_ref()
            .unwrap()
            .read_exact_at(&mut actual, 0)
            .unwrap();
        assert_eq!(actual, original);
        device.write(1, &[1]);
        device.write(7, &[0xa0]);
        assert_eq!(status(&mut device), READY | ERROR);
    }

    #[test]
    fn toc_rejects_media_too_large_for_msf_address() {
        let (mut device, _, _) = fixture();
        device.state.media.as_mut().unwrap().size = 256 * 60 * 75 * BLOCK_SIZE;
        packet(&mut device, &[0x43, 2, 0, 0, 0, 0, 0xaa, 0, 20], 2048);
        assert_eq!(status(&mut device), READY | ERROR);
        assert_eq!(device.state.sense, [5, 0x24, 0]);
    }

    #[test]
    fn inquiry_toc_capacity_and_mode_pages() {
        let (mut device, _, _) = fixture();
        packet(&mut device, &[0x12, 0, 0, 0, 35], 8);
        let inquiry = receive(&mut device);
        assert_eq!(inquiry.len(), 35);
        assert_eq!(&inquiry[..2], &[5, 0x80]);
        packet(&mut device, &[0x25], 2048);
        assert_eq!(receive(&mut device), [0, 0, 0, 3, 0, 0, 8, 0]);
        packet(&mut device, &[0x43, 0, 0, 0, 0, 0, 0, 0, 20], 2048);
        let toc = receive(&mut device);
        assert_eq!(&toc[..4], &[0, 18, 1, 1]);
        assert_eq!(&toc[16..20], &[0, 0, 0, 4]);
        packet(&mut device, &[0x5a, 0, 0x3f, 0, 0, 0, 0, 0, 64], 2048);
        let modes = receive(&mut device);
        assert_eq!(modes[3], 0x80);
        assert_eq!((modes[8], modes[16]), (1, 0x2a));
    }

    #[test]
    fn session_toc_reports_last_complete_session_and_honours_allocation() {
        let (mut device, _, _) = fixture();
        // Session addresses remain representable even when lead-out exceeds MSF.
        device.state.media.as_mut().unwrap().size = 5 * 1024 * 1024 * 1024;
        packet(&mut device, &[0x43, 0, 1, 0, 0, 0, 0, 0, 12], 2048);
        assert_eq!(
            receive(&mut device),
            [0, 10, 1, 1, 0, 0x14, 1, 0, 0, 0, 0, 0]
        );
        packet(&mut device, &[0x43, 2, 1, 0, 0, 0, 0, 0, 12], 2048);
        assert_eq!(
            receive(&mut device),
            [0, 10, 1, 1, 0, 0x14, 1, 0, 0, 0, 2, 0]
        );
        packet(&mut device, &[0x43, 0, 1, 0, 0, 0, 0, 0, 4], 2048);
        assert_eq!(receive(&mut device), [0, 10, 1, 1]);
        packet(&mut device, &[0x43, 0, 1, 0, 0, 0, 1, 0, 12], 2048);
        assert_eq!(status(&mut device), READY | ERROR);
        assert_eq!(device.state.sense, [5, 0x24, 0]);
    }

    #[test]
    fn host_eject_obeys_guest_lock_and_active_transfer() {
        let (mut device, _, _) = fixture();
        packet(&mut device, &[0x1e, 0, 0, 0, 1], 2048);
        assert_eq!(
            device.eject().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        packet(&mut device, &[0x1e], 2048);
        packet(&mut device, &[0x28, 0, 0, 0, 0, 0, 0, 0, 1], 2048);
        assert_eq!(
            device.eject().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(receive(&mut device).len(), 2048);
        device.eject().unwrap();
        assert!(!device.state.media_present());
    }

    #[test]
    fn locked_tray_and_empty_media() {
        let (mut device, _, _) = fixture();
        packet(&mut device, &[0x1e, 0, 0, 0, 1], 2048);
        packet(&mut device, &[0x1b, 0, 0, 0, 2], 2048);
        assert_eq!(status(&mut device), READY | ERROR);
        assert!(device.state.media_present());
        packet(&mut device, &[0x1e], 2048);
        packet(&mut device, &[0x1b, 0, 0, 0, 2], 2048);
        packet(&mut device, &[0x25], 2048);
        assert_eq!(status(&mut device), READY | ERROR);
        assert_eq!(device.state.sense, [2, 0x3a, 0]);
        packet(&mut device, &[0x1b, 0, 0, 0, 3], 2048);
        packet(&mut device, &[0], 2048);
        assert_eq!(status(&mut device), READY);
    }
}
