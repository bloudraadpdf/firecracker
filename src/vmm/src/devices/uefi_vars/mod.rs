// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Bounded DMA transport for EDK II's QemuUefiVars MM protocol.
//! Authentication and persistence belong to the separate variable-service helper.

mod client;

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
const MAX_MESSAGE: u32 = 65536;
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
    fn validate(&self) -> io::Result<()> {
        if self.size > MAX_MESSAGE
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
}

impl UefiVars {
    /// Begin a cold boot; clears volatile firmware state through the host-only IPC operation.
    pub fn new(path: &Path) -> io::Result<Self> {
        let mut client = Client::connect(path)?;
        client.exchange(4, &[], 0)?;
        Ok(Self {
            client,
            status: SUCCESS,
            size: 0,
            address: 0,
            destination: None,
            failed: false,
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
            status: state.status,
            size: state.size,
            address: state.address,
            destination: None,
            failed: false,
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
        })
    }

    /// Whether the host must stop the VM after a transport failure.
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// Whether the event loop must progress an outstanding transfer.
    pub fn pending(&self) -> bool {
        !self.failed && self.client.pending()
    }

    /// Read the exact register widths from the EDK II interface.
    pub fn read(&self, offset: u64, data: &mut [u8]) {
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
        if self.failed || self.pending() {
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
                        if self.client.start(1, &message, length + 4).is_err() {
                            self.failed = true;
                            self.status = ERROR;
                            return;
                        }
                        self.destination = Some((address, length));
                        self.status = BUSY;
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
        })();
        if result.is_err() {
            self.failed = true;
            self.status = ERROR;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn fixture() -> (UefiVars, std::os::unix::net::UnixStream, GuestMemoryMmap) {
        let (client, peer) = client::tests::pair();
        let core = UefiVars {
            client,
            status: SUCCESS,
            size: 32,
            address: 0x100,
            destination: None,
            failed: false,
        };
        let memory = crate::vstate::memory::test_utils::into_region_ext(
            crate::vstate::memory::anonymous(
                [(GuestAddress(0), 0x1000)].into_iter(),
                true,
                crate::vmm_config::machine_config::HugePageConfig::None,
            )
            .unwrap(),
        );
        (core, peer, memory)
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
}
