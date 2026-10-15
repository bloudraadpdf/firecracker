// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Bounded DMA transport for EDK II's QemuUefiVars MM protocol.
//! Authentication and persistence belong to the separate variable-service helper.

mod client;
mod ppi;
use ppi::PpiRegisters;

use crate::vstate::memory::GuestMemoryMmap;
use client::{Client, MAX_CHECKPOINT, invalid};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;
use vm_memory::{Bytes, GuestAddress};

/// Fixed protected-variable register page, outside PCI BAR allocation.
pub const MMIO_BASE: u64 = 0xfed5_0000;
/// Reserved register aperture.
pub const MMIO_SIZE: u64 = 0x1000;
const MAX_MESSAGE: u32 = 2 * 1024 * 1024;
const MM_HEADER: usize = 24;
const SUCCESS: u16 = 0;
const BUSY: u16 = 1;
const ERROR: u16 = 0x10;
const UNSUPPORTED: u16 = 0x11;
const BAD_SIZE: u16 = 0x12;

/// Register state and complete authenticated store from one paused checkpoint.
#[derive(Clone, Serialize, Deserialize)]
pub struct UefiVarsState {
    status: u16,
    size: u32,
    address: u64,
    checkpoint: Vec<u8>,
    ppi: PpiRegisters,
}

impl std::fmt::Debug for UefiVarsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UefiVarsState")
            .field("status", &self.status)
            .field("checkpoint_bytes", &self.checkpoint.len())
            .finish_non_exhaustive()
    }
}

impl UefiVarsState {
    #[cfg(test)]
    pub(crate) fn maximum_test_state() -> Self {
        Self {
            ppi: PpiRegisters::default(),
            status: SUCCESS,
            size: MAX_MESSAGE,
            address: 0x1000,
            // Exercise all byte values: repeated zeroes could hide size regressions
            // through bitcode's integer compression. No helper interprets this fixture.
            checkpoint: (0..=255).cycle().take(MAX_CHECKPOINT).collect(),
        }
    }

    fn validate(&self) -> io::Result<()> {
        if !self.ppi.valid()
            || self.size > MAX_MESSAGE
            || !matches!(self.status, SUCCESS | ERROR | UNSUPPORTED | BAD_SIZE)
            || self.checkpoint.len() > MAX_CHECKPOINT
        {
            return Err(invalid("invalid UEFI variable checkpoint"));
        }
        Ok(())
    }
}

/// Nonblocking copied-message transport; no guest memory is shared with the helper.
#[derive(Debug)]
pub struct UefiVars {
    client: Client,
    status: u16,
    size: u32,
    address: u64,
    destination: Option<(GuestAddress, usize)>,
    failed: bool,
    ppi: PpiRegisters,
    active: Option<Channel>,
    queued: Option<Request>,
}

#[derive(Debug, Clone, Copy)]
enum Channel {
    Dma,
    Ppi,
}

struct Request {
    channel: Channel,
    payload: Vec<u8>,
    max_reply: usize,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("channel", &self.channel)
            .field("payload_bytes", &self.payload.len())
            .finish_non_exhaustive()
    }
}

impl UefiVars {
    /// Begin a cold boot; clears volatile firmware state through the host-only IPC operation.
    pub fn new(path: &Path) -> io::Result<Self> {
        let mut client = Client::connect(path)?;
        client.exchange(4, &[], 0)?;
        Ok(Self {
            client,
            ppi: PpiRegisters::default(),
            status: SUCCESS,
            size: 0,
            address: 0,
            destination: None,
            failed: false,
            active: None,
            queued: None,
        })
    }

    /// Restore complete state only when the helper's live generation matches the caller's value.
    pub fn restore(
        path: &Path,
        expected_generation: u64,
        state: &UefiVarsState,
    ) -> io::Result<Self> {
        state.validate()?;
        let checkpoint: serde_json::Value = serde_json::from_slice(&state.checkpoint)?;
        let payload = serde_json::to_vec(
            &serde_json::json!({"expected_generation": expected_generation, "checkpoint": checkpoint}),
        )?;
        let mut client = Client::connect(path)?;
        client.exchange(3, &payload, 0)?;
        Ok(Self {
            client,
            ppi: state.ppi.clone(),
            status: state.status,
            size: state.size,
            address: state.address,
            destination: None,
            failed: false,
            active: None,
            queued: None,
        })
    }

    /// Checkpoint only a quiescent transport; an in-flight request requires retry.
    pub fn snapshot(&mut self) -> io::Result<UefiVarsState> {
        if self.failed {
            return Err(invalid("variable service failed"));
        }
        if self.pending() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let checkpoint = self.client.exchange(2, &[], MAX_CHECKPOINT)?;
        Ok(UefiVarsState {
            status: self.status,
            size: self.size,
            address: self.address,
            checkpoint,
            ppi: self.ppi.clone(),
        })
    }

    /// Whether the host must stop the VM after a transport failure.
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// Whether the event loop must progress an outstanding transfer.
    pub fn pending(&self) -> bool {
        !self.failed && (self.client.pending() || self.queued.is_some())
    }

    /// Read the exact register widths from the EDK II interface.
    pub fn read(&self, offset: u64, data: &mut [u8]) {
        if offset >= ppi::BASE {
            self.ppi.read(offset, data);
            return;
        }
        data.fill(0);
        match (offset, data.len()) {
            (0, 2) => data.copy_from_slice(&0x0ef1u16.to_le_bytes()),
            (2, 2) => data.copy_from_slice(&self.status.to_le_bytes()),
            (4, 4) => data.copy_from_slice(&self.size.to_le_bytes()),
            (8, 4) => data.copy_from_slice(&self.address.to_le_bytes()[..4]),
            (12, 4) => data.copy_from_slice(&self.address.to_le_bytes()[4..]),
            _ => (),
        }
    }

    /// Queue a DMA request using one checked copy; guest writes cannot change its destination.
    pub fn write(&mut self, offset: u64, data: &[u8], memory: &GuestMemoryMmap) {
        if self.failed {
            return;
        }
        if offset >= ppi::BASE {
            if let Some(payload) = self.ppi.write(offset, data) {
                self.submit(Request {
                    channel: Channel::Ppi,
                    payload,
                    max_reply: 12,
                });
            }
            return;
        }
        if self.status == BUSY {
            return;
        }
        match (offset, data.len()) {
            (2, 2) => match u16::from_le_bytes(data.try_into().unwrap()) {
                1 => {
                    self.status = SUCCESS;
                    self.size = 0;
                    self.address = 0;
                }
                2 => match self.message(memory) {
                    Ok(message) => {
                        let address = GuestAddress(self.address);
                        let length = message.len();
                        self.destination = Some((address, length));
                        self.status = BUSY;
                        self.submit(Request {
                            channel: Channel::Dma,
                            payload: message,
                            max_reply: length + 4,
                        });
                    }
                    Err(_) => self.status = BAD_SIZE,
                },
                _ => self.status = UNSUPPORTED,
            },
            (4, 4) => self.size = u32::from_le_bytes(data.try_into().unwrap()).min(MAX_MESSAGE),
            (8, 4) => {
                self.address = (self.address & 0xffff_ffff_0000_0000)
                    | u64::from(u32::from_le_bytes(data.try_into().unwrap()))
            }
            (12, 4) => {
                self.address = (self.address & 0xffff_ffff)
                    | (u64::from(u32::from_le_bytes(data.try_into().unwrap())) << 32)
            }
            _ => (),
        }
    }

    fn submit(&mut self, request: Request) {
        if self.active.is_some() {
            assert!(self.queued.is_none());
            self.queued = Some(request);
        } else if self.start(request).is_err() {
            self.fail();
        }
    }

    fn start(&mut self, request: Request) -> io::Result<()> {
        let opcode = match request.channel {
            Channel::Dma => 1,
            Channel::Ppi => 5,
        };
        self.client
            .start(opcode, &request.payload, request.max_reply)?;
        self.active = Some(request.channel);
        Ok(())
    }

    fn fail(&mut self) {
        self.failed = true;
        self.status = ERROR;
        self.ppi.fail();
    }

    fn message(&self, memory: &GuestMemoryMmap) -> io::Result<Vec<u8>> {
        if self.size < u32::try_from(MM_HEADER).unwrap() {
            return Err(invalid("UEFI MM header does not fit"));
        }
        let mut header = [0; MM_HEADER];
        memory
            .read_slice(&mut header, GuestAddress(self.address))
            .map_err(io::Error::other)?;
        let payload = u64::from_le_bytes(header[16..].try_into().unwrap());
        let size = payload
            .checked_add(u64::try_from(MM_HEADER).unwrap())
            .filter(|size| *size <= u64::from(self.size))
            .ok_or_else(|| invalid("UEFI MM message exceeds buffer"))?;
        self.address
            .checked_add(size)
            .ok_or_else(|| invalid("UEFI MM address wraps"))?;
        let mut message = vec![0; usize::try_from(size).unwrap()];
        message[..MM_HEADER].copy_from_slice(&header);
        memory
            .read_slice(
                &mut message[MM_HEADER..],
                GuestAddress(self.address + u64::try_from(MM_HEADER).unwrap()),
            )
            .map_err(io::Error::other)?;
        Ok(message)
    }

    /// Advance one bounded IPC step. Any backend failure must stop the VM.
    pub fn process(&mut self, memory: &GuestMemoryMmap) -> io::Result<()> {
        if self.failed {
            return Err(invalid("variable service failed"));
        }
        let result = (|| {
            let Some(reply) = self.client.progress()? else {
                return Ok(());
            };
            match self
                .active
                .take()
                .ok_or_else(|| invalid("unsolicited variable-service reply"))?
            {
                Channel::Dma => self.complete_dma(&reply, memory)?,
                Channel::Ppi => self.ppi.complete(&reply)?,
            }
            if let Some(request) = self.queued.take() {
                self.start(request)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.fail();
        }
        result
    }

    fn complete_dma(&mut self, reply: &[u8], memory: &GuestMemoryMmap) -> io::Result<()> {
        let (destination, size) = self
            .destination
            .take()
            .ok_or_else(|| invalid("unsolicited variable-service reply"))?;
        if reply.len() != size + 4 {
            return Err(invalid("variable-service message size changed"));
        }
        let status = u32::from_le_bytes(reply[..4].try_into().unwrap());
        if !matches!(status, 0 | 0x10 | 0x11 | 0x12) {
            return Err(invalid("invalid variable-service status"));
        }
        let returned = u64::from_le_bytes(reply[20..28].try_into().unwrap());
        if returned > u64::try_from(size - MM_HEADER).unwrap() {
            return Err(invalid("variable-service payload exceeds request"));
        }
        memory
            .write_slice(&reply[4..], destination)
            .map_err(io::Error::other)?;
        self.status = u16::try_from(status).unwrap();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn fixture() -> (UefiVars, std::os::unix::net::UnixStream, GuestMemoryMmap) {
        fixture_with_memory(0x1000)
    }

    fn fixture_with_memory(
        size: usize,
    ) -> (UefiVars, std::os::unix::net::UnixStream, GuestMemoryMmap) {
        let (client, peer) = client::tests::pair();
        let core = UefiVars {
            client,
            ppi: PpiRegisters::default(),
            status: SUCCESS,
            size: 32,
            address: 0x100,
            destination: None,
            failed: false,
            active: None,
            queued: None,
        };
        let memory = crate::vstate::memory::test_utils::into_region_ext(
            crate::vstate::memory::anonymous(
                [(GuestAddress(0), size)].into_iter(),
                true,
                crate::vmm_config::machine_config::HugePageConfig::None,
            )
            .unwrap(),
        );
        (core, peer, memory)
    }

    #[test]
    fn large_variable_dma_fits_and_maximum_is_bounded() {
        let (mut core, mut peer, memory) = fixture_with_memory(MAX_MESSAGE as usize + 0x1000);
        core.write(4, &u32::MAX.to_le_bytes(), &memory);
        assert_eq!(core.size, MAX_MESSAGE);
        let size = 128 * 1024;
        let mut message = vec![0xff; size];
        message[16..24].copy_from_slice(&(u64::try_from(size - MM_HEADER).unwrap()).to_le_bytes());
        memory
            .write_slice(&message, GuestAddress(core.address))
            .unwrap();
        assert_eq!(core.message(&memory).unwrap(), message);
        *message.last_mut().unwrap() = 0x55;
        let response = message.clone();
        let backend = std::thread::spawn(move || {
            let mut header = [0; 16];
            peer.read_exact(&mut header).unwrap();
            let mut request = vec![0; size];
            peer.read_exact(&mut request).unwrap();
            *request.last_mut().unwrap() = 0x55;
            assert_eq!(request, response);
            header[7] |= 0x80;
            header[12..16].copy_from_slice(&u32::try_from(size + 8).unwrap().to_le_bytes());
            peer.write_all(&header).unwrap();
            peer.write_all(&[0; 8]).unwrap();
            peer.write_all(&response).unwrap();
        });
        core.write(2, &2u16.to_le_bytes(), &memory);
        while core.pending() {
            core.process(&memory).unwrap();
            std::thread::yield_now();
        }
        backend.join().unwrap();
        let mut returned = vec![0; size];
        memory
            .read_slice(&mut returned, GuestAddress(core.address))
            .unwrap();
        assert_eq!(returned, message);
        memory
            .write_slice(
                &u64::from(MAX_MESSAGE).to_le_bytes(),
                GuestAddress(core.address + 16),
            )
            .unwrap();
        core.message(&memory).unwrap_err();
        let mut state = UefiVarsState::maximum_test_state();
        state.validate().unwrap();
        state.size = MAX_MESSAGE + 1;
        state.validate().unwrap_err();
    }

    #[test]
    fn reset_only_resets_transport_and_never_sends_host_cold_boot() {
        let (mut core, mut peer, memory) = fixture();
        peer.set_nonblocking(true).unwrap();
        core.write(2, &1u16.to_le_bytes(), &memory);
        assert_eq!(core.size, 0);
        assert_eq!(core.address, 0);
        assert!(!core.pending());
        assert_eq!(
            peer.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn wrapped_and_unmapped_dma_are_rejected_without_backend_access() {
        let (mut core, _peer, memory) = fixture();
        for address in [u64::MAX - 8, 0x2000] {
            core.address = address;
            core.write(2, &2u16.to_le_bytes(), &memory);
            assert_eq!(core.status, BAD_SIZE);
            assert!(!core.pending());
        }
        core.address = 0x100;
        memory
            .write_slice(&u64::MAX.to_le_bytes(), GuestAddress(0x110))
            .unwrap();
        core.write(2, &2u16.to_le_bytes(), &memory);
        assert_eq!(core.status, BAD_SIZE);
        assert!(!core.pending());
    }

    #[test]
    fn request_and_destination_are_frozen_until_complete() {
        let (mut core, mut peer, memory) = fixture();
        memory
            .write_slice(&8u64.to_le_bytes(), GuestAddress(0x110))
            .unwrap();
        core.write(2, &2u16.to_le_bytes(), &memory);
        assert_eq!(core.status, BUSY);
        core.write(8, &0x800u32.to_le_bytes(), &memory);
        core.write(2, &1u16.to_le_bytes(), &memory);
        assert_eq!(core.address, 0x100);
        core.process(&memory).unwrap();
        let mut request = [0; 48];
        peer.read_exact(&mut request).unwrap();
        request[6..8].copy_from_slice(&0x8001u16.to_le_bytes());
        request[12..16].copy_from_slice(&40u32.to_le_bytes());
        let mut reply = request[..16].to_vec();
        reply.extend_from_slice(&[0; 8]);
        reply.extend_from_slice(&request[16..]);
        *reply.last_mut().unwrap() = 0x55;
        peer.write_all(&reply).unwrap();
        core.process(&memory).unwrap();
        assert!(!core.pending());
        assert_eq!(core.status, SUCCESS);
        let mut byte = [0];
        memory.read_slice(&mut byte, GuestAddress(0x11f)).unwrap();
        assert_eq!(byte, [0x55]);
        memory.read_slice(&mut byte, GuestAddress(0x81f)).unwrap();
        assert_eq!(byte, [0]);
    }
    fn submit_ppi(core: &mut UefiVars, memory: &GuestMemoryMmap) {
        for (offset, value) in [(0x104, 3u32), (0x108, 1), (0x100, 1)] {
            core.write(offset, &value.to_le_bytes(), memory);
        }
    }

    fn reply(peer: &mut std::os::unix::net::UnixStream, payload: &[u8]) -> Vec<u8> {
        let mut header = [0; 16];
        peer.read_exact(&mut header).unwrap();
        let size = u32::from_le_bytes(header[12..16].try_into().unwrap()) as usize;
        let mut request = vec![0; size];
        peer.read_exact(&mut request).unwrap();
        header[7] |= 0x80;
        header[12..16].copy_from_slice(&u32::try_from(payload.len() + 4).unwrap().to_le_bytes());
        peer.write_all(&header).unwrap();
        peer.write_all(&[0; 4]).unwrap();
        peer.write_all(payload).unwrap();
        request
    }

    #[test]
    fn both_channels_accept_one_frozen_request_in_either_order() {
        for ppi_first in [true, false] {
            let (mut core, mut peer, memory) = fixture();
            if ppi_first {
                submit_ppi(&mut core, &memory);
            }
            core.write(2, &2u16.to_le_bytes(), &memory);
            if !ppi_first {
                submit_ppi(&mut core, &memory);
            }
            assert_eq!(core.status, BUSY);
            assert_eq!(
                core.snapshot().unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
            core.write(8, &0x800u32.to_le_bytes(), &memory);
            core.write(0x104, &8u32.to_le_bytes(), &memory);
            core.write(0x100, &1u32.to_le_bytes(), &memory);
            assert_eq!(core.address, 0x100);
            let mut function = [0; 4];
            core.read(0x104, &mut function);
            assert_eq!(function, 3u32.to_le_bytes());
            for is_ppi in [ppi_first, !ppi_first] {
                core.process(&memory).unwrap();
                let payload = if is_ppi { vec![0; 12] } else { vec![0; 28] };
                let request = reply(&mut peer, &payload);
                if is_ppi {
                    assert_eq!(
                        request,
                        [3u32, 1, 0, 0]
                            .into_iter()
                            .flat_map(u32::to_le_bytes)
                            .collect::<Vec<_>>()
                    );
                } else {
                    assert_eq!(request.len(), 24);
                }
                core.process(&memory).unwrap();
            }
            assert!(!core.pending());
            assert_eq!(core.status, SUCCESS);
            assert!(core.ppi.valid());
        }
    }

    #[test]
    fn invalid_ppi_access_never_reaches_helper() {
        let (mut core, mut peer, memory) = fixture();
        peer.set_nonblocking(true).unwrap();
        for (offset, data) in [
            (0x101, vec![1; 4]),
            (0x100, vec![1; 2]),
            (0x120, vec![1; 4]),
        ] {
            core.write(offset, &data, &memory);
        }
        for (function, revision, operation, argument) in
            [(1, 1, 0, 0), (3, 3, 0, 0), (3, 1, 1, 0), (7, 2, 5, 1)]
        {
            for (index, value) in [function, revision, operation, argument]
                .into_iter()
                .enumerate()
            {
                core.write(0x104 + index as u64 * 4, &u32::to_le_bytes(value), &memory);
            }
            core.write(0x100, &1u32.to_le_bytes(), &memory);
            assert!(!core.pending());
            let mut status = [0; 4];
            core.read(0x100, &mut status);
            assert_eq!(status, 2u32.to_le_bytes());
        }
        assert_eq!(
            peer.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn ppi_backend_failure_stops_transport_and_rejects_snapshot() {
        let (mut core, peer, memory) = fixture();
        submit_ppi(&mut core, &memory);
        drop(peer);
        core.process(&memory).unwrap_err();
        assert!(core.failed());
        core.snapshot().unwrap_err();
        assert_eq!(core.status, ERROR);
    }

    #[test]
    fn malformed_ppi_reply_is_fatal() {
        let (mut core, mut peer, memory) = fixture();
        submit_ppi(&mut core, &memory);
        core.process(&memory).unwrap();
        reply(&mut peer, &[0; 8]);
        core.process(&memory).unwrap_err();
        assert!(core.failed());
    }
    #[test]
    fn checkpoint_preserves_ppi_registers_and_rejects_busy_mailbox() {
        let (mut core, _peer, memory) = fixture();
        core.write(0x104, &3u32.to_le_bytes(), &memory);
        core.write(0x108, &2u32.to_le_bytes(), &memory);
        let mut state = UefiVarsState {
            status: core.status,
            size: core.size,
            address: core.address,
            checkpoint: Vec::new(),
            ppi: core.ppi.clone(),
        };
        let encoded = bitcode::serialize(&state).unwrap();
        let decoded: UefiVarsState = bitcode::deserialize(&encoded).unwrap();
        decoded.validate().unwrap();
        let mut revision = [0; 4];
        decoded.ppi.read(0x108, &mut revision);
        assert_eq!(revision, 2u32.to_le_bytes());
        core.write(0x100, &1u32.to_le_bytes(), &memory);
        state.ppi = core.ppi.clone();
        state.validate().unwrap_err();
    }
}
