// Copyright 2026 Bloudraad.
// Copyright 2022 Microsoft Corporation (CRB register layout).
// SPDX-License-Identifier: Apache-2.0

//! Locality-zero TPM 2.0 CRB, following Cloud Hypervisor c7dd43dbc3b6.
//! The event loop must call `process` at least every millisecond while running.

mod backend;

use std::io::{self, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use backend::{Backend, Blob};

/// Fixed AMD64 TPM CRB aperture.
pub const MMIO_BASE: u64 = 0xfed4_0000;
/// Locality zero occupies one page.
pub const MMIO_SIZE: u64 = 0x1000;
const BUFFER_OFFSET: usize = 0x80;
const BUFFER_SIZE: usize = 4096 - BUFFER_OFFSET;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const INTERFACE: u32 = 1 | (1 << 4) | (3 << 11) | (1 << 14) | (1 << 17);

/// Quiescent CRB and complete backend state, captured with vCPUs paused.
#[derive(Clone, Serialize, Deserialize)]
pub struct TpmState {
    buffer: Vec<u8>,
    locality: bool,
    idle: bool,
    fatal: bool,
    interface_locked: bool,
    cancel_latched: bool,
    established: bool,
    blobs: Vec<Blob>,
}

struct Transfer {
    request: Vec<u8>,
    sent: usize,
    response: Vec<u8>,
    received: usize,
    deadline: Instant,
}

impl std::fmt::Debug for TpmState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TpmState")
            .field("locality", &self.locality)
            .field("idle", &self.idle)
            .field("fatal", &self.fatal)
            .field("blobs", &self.blobs)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Transfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transfer")
            .field("sent", &self.sent)
            .field("received", &self.received)
            .finish_non_exhaustive()
    }
}

/// Bounded CRB device with a private external swtpm backend.
#[derive(Debug)]
pub struct Tpm {
    backend: Backend,
    state: TpmState,
    transfer: Option<Transfer>,
    cancel: bool,
}

impl TpmState {
    /// Validate before connecting to or mutating the destination backend.
    pub fn validate(&self) -> io::Result<()> {
        if self.buffer.len() != BUFFER_SIZE || (self.idle && self.fatal) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid TPM CRB state",
            ));
        }
        Backend::validate(&self.blobs)
    }
}

impl Tpm {
    /// Connect before entering the sandbox. Firmware owns TPM2_Startup.
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        let mut backend = Backend::connect(path.as_ref())?;
        let established = backend.initialize(true)?;
        Ok(Self {
            backend,
            state: TpmState {
                buffer: vec![0; BUFFER_SIZE],
                locality: false,
                idle: true,
                fatal: false,
                interface_locked: false,
                cancel_latched: false,
                established,
                blobs: Vec::new(),
            },
            transfer: None,
            cancel: false,
        })
    }

    /// Import into a dedicated swtpm instance; discard that helper on failure.
    pub fn restore(path: impl AsRef<Path>, state: &TpmState) -> io::Result<Self> {
        state.validate()?;
        let mut backend = Backend::connect(path.as_ref())?;
        let established = backend.import(&state.blobs)?;
        if established != state.established {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "TPM established state changed",
            ));
        }
        let mut state = state.clone();
        state.blobs.clear();
        Ok(Self {
            backend,
            state,
            transfer: None,
            cancel: false,
        })
    }

    /// Export only after pausing vCPUs. Retry after pending command completion.
    pub fn snapshot(&mut self) -> io::Result<TpmState> {
        self.process();
        if self.transfer.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "TPM command is pending",
            ));
        }
        if self.state.fatal {
            return Err(io::Error::other("TPM backend is in a terminal error state"));
        }
        let mut state = self.state.clone();
        state.blobs = match self.backend.export() {
            Ok(blobs) => blobs,
            Err(error) => {
                self.fail();
                return Err(error);
            }
        };
        Ok(state)
    }

    /// Whether the event loop must keep the progress timer armed.
    pub fn pending(&self) -> bool {
        self.transfer.is_some() || self.cancel
    }

    /// Advance nonblocking data I/O even when the guest does not poll CRB.
    pub fn process(&mut self) {
        if self.cancel {
            self.cancel = false;
            if self.backend.cancel().is_err() {
                self.fail();
                return;
            }
        }
        let Some(transfer) = self.transfer.as_mut() else {
            return;
        };
        if Instant::now() >= transfer.deadline {
            self.fail();
            return;
        }
        match transfer.progress(&mut self.backend.data) {
            Ok(true) => {
                let transfer = self.transfer.take().unwrap();
                self.state.buffer.fill(0);
                self.state.buffer[..transfer.response.len()].copy_from_slice(&transfer.response);
            }
            Ok(false) => (),
            Err(_) => self.fail(),
        }
    }

    fn fail(&mut self) {
        self.transfer = None;
        self.cancel = false;
        self.state.fatal = true;
        self.state.idle = false;
        self.state.buffer.fill(0);
    }

    /// Read a little-endian register or bytes from the shared buffer.
    pub fn read(&mut self, offset: u64, data: &mut [u8]) {
        data.fill(0xff);
        let Ok(offset) = usize::try_from(offset) else {
            return;
        };
        if !matches!(data.len(), 1 | 2 | 4 | 8)
            || offset.checked_add(data.len()).is_none_or(|end| end > 4096)
        {
            return;
        }
        if offset >= BUFFER_OFFSET {
            data.copy_from_slice(
                &self.state.buffer[offset - BUFFER_OFFSET..offset - BUFFER_OFFSET + data.len()],
            );
            return;
        }
        let mut registers = [0u8; BUFFER_OFFSET];
        for (position, value) in [
            (
                0,
                0x80 | u32::from(self.state.established) | (u32::from(self.state.locality) << 1),
            ),
            (0x0c, u32::from(self.state.locality)),
            (
                0x30,
                INTERFACE | (u32::from(self.state.interface_locked) << 19),
            ),
            (0x34, 0x1014),
            (
                0x44,
                u32::from(self.state.fatal) | (u32::from(self.state.idle) << 1),
            ),
            (0x48, u32::from(self.state.cancel_latched)),
            (0x4c, u32::from(self.transfer.is_some())),
            (0x58, u32::try_from(BUFFER_SIZE).unwrap()),
            (0x5c, u32::try_from(MMIO_BASE + 0x80).unwrap()),
            (0x64, u32::try_from(BUFFER_SIZE).unwrap()),
            (0x68, u32::try_from(MMIO_BASE + 0x80).unwrap()),
        ] {
            registers[position..position + 4].copy_from_slice(&value.to_le_bytes());
        }
        if let Some(bytes) = registers.get(offset..offset + data.len()) {
            data.copy_from_slice(bytes);
        }
    }

    /// Write CRB control or shared command bytes. No guest-provided DMA address is used.
    pub fn write(&mut self, offset: u64, data: &[u8]) {
        let Ok(offset) = usize::try_from(offset) else {
            return;
        };
        if !matches!(data.len(), 1 | 2 | 4 | 8)
            || offset.checked_add(data.len()).is_none_or(|end| end > 4096)
        {
            return;
        }
        if offset >= BUFFER_OFFSET {
            if self.transfer.is_none() && !self.state.fatal {
                self.state.buffer[offset - BUFFER_OFFSET..offset - BUFFER_OFFSET + data.len()]
                    .copy_from_slice(data);
            }
            return;
        }
        if data.len() != 4 || offset % 4 != 0 {
            return;
        }
        let value = u32::from_le_bytes(data.try_into().unwrap());
        match offset {
            8 if self.transfer.is_none() => {
                if value & 2 != 0 {
                    self.state.locality = false;
                } else if value & 1 != 0 {
                    self.state.locality = true;
                }
            }
            0x30 => self.state.interface_locked |= value & (1 << 19) != 0,
            0x40 if self.state.locality && self.transfer.is_none() && !self.state.fatal => {
                if value & 1 != 0 {
                    self.state.idle = false;
                } else if value & 2 != 0 {
                    self.state.idle = true;
                }
            }
            0x48 if value == 0 => self.state.cancel_latched = false,
            0x48 if value == 1 && self.transfer.is_some() => {
                self.state.cancel_latched = true;
                self.cancel = true;
            }
            0x4c if value & 1 != 0
                && self.state.locality
                && !self.state.idle
                && !self.state.fatal
                && self.transfer.is_none() =>
            {
                let length =
                    u32::from_be_bytes(self.state.buffer[2..6].try_into().unwrap()) as usize;
                if !(10..=BUFFER_SIZE).contains(&length)
                    || !matches!(&self.state.buffer[..2], [0x80, 1 | 2])
                {
                    self.fail();
                    return;
                }
                self.transfer = Some(Transfer {
                    request: self.state.buffer[..length].to_vec(),
                    sent: 0,
                    response: vec![0; 10],
                    received: 0,
                    deadline: Instant::now() + COMMAND_TIMEOUT,
                });
            }
            _ => (),
        }
    }
}

impl Transfer {
    fn progress(&mut self, data: &mut std::os::unix::net::UnixStream) -> io::Result<bool> {
        if self.sent < self.request.len() {
            match data.write(&self.request[self.sent..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => self.sent += count,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    return Ok(false);
                }
                Err(error) => return Err(error),
            }
            if self.sent < self.request.len() {
                return Ok(false);
            }
        }
        match data.read(&mut self.response[self.received..]) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(count) => self.received += count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                return Ok(false);
            }
            Err(error) => return Err(error),
        }
        if self.received < self.response.len() {
            return Ok(false);
        }
        if self.response.len() == 10 {
            let length = u32::from_be_bytes(self.response[2..6].try_into().unwrap()) as usize;
            if !(10..=BUFFER_SIZE).contains(&length)
                || !matches!(&self.response[..2], [0x80, 1 | 2])
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid TPM response header",
                ));
            }
            self.response.resize(length, 0);
        }
        Ok(self.received == self.response.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::net::UnixStream;

    fn disconnected_tpm() -> Tpm {
        Tpm {
            backend: Backend::disconnected(),
            state: TpmState {
                buffer: vec![0; BUFFER_SIZE],
                locality: false,
                idle: true,
                fatal: false,
                interface_locked: false,
                cancel_latched: false,
                established: false,
                blobs: Vec::new(),
            },
            transfer: None,
            cancel: false,
        }
    }

    fn register(tpm: &mut Tpm, offset: u64) -> u32 {
        let mut data = [0; 4];
        tpm.read(offset, &mut data);
        u32::from_le_bytes(data)
    }

    #[test]
    fn crb_locality_ready_and_fixed_buffer_bounds() {
        let mut tpm = disconnected_tpm();
        assert_eq!(register(&mut tpm, 0x44), 2);
        tpm.write(0x40, &1u32.to_le_bytes());
        assert_eq!(register(&mut tpm, 0x44), 2);
        tpm.write(8, &1u32.to_le_bytes());
        assert_eq!(register(&mut tpm, 0), 0x82);
        tpm.write(0x40, &1u32.to_le_bytes());
        assert_eq!(register(&mut tpm, 0x44), 0);
        tpm.write(0x5c, &0u32.to_le_bytes());
        assert_eq!(register(&mut tpm, 0x5c), 0xfed40080);
        tpm.write(4095, &[7, 8]);
        assert_eq!(tpm.state.buffer[BUFFER_SIZE - 1], 0);
        tpm.write(4095, &[7]);
        assert_eq!(tpm.state.buffer[BUFFER_SIZE - 1], 7);
        let mut overflow = [0; 8];
        tpm.read(u64::MAX, &mut overflow);
        assert_eq!(overflow, [0xff; 8]);
    }

    #[test]
    fn repeated_buffer_writes_use_header_length_and_freeze_pending_request() {
        let mut tpm = disconnected_tpm();
        tpm.write(8, &1u32.to_le_bytes());
        tpm.write(0x40, &1u32.to_le_bytes());
        for _ in 0..3 {
            for (offset, byte) in [0x80, 1, 0, 0, 0, 10, 0, 0, 1, 0x43].iter().enumerate() {
                tpm.write(u64::try_from(0x80 + offset).unwrap(), &[*byte]);
            }
        }
        tpm.write(0x4c, &1u32.to_le_bytes());
        assert_eq!(tpm.transfer.as_ref().unwrap().request.len(), 10);
        tpm.write(0x80, &[0]);
        assert_eq!(tpm.state.buffer[0], 0x80);
        tpm.write(8, &2u32.to_le_bytes());
        assert!(tpm.state.locality);
        tpm.transfer.as_mut().unwrap().deadline = Instant::now();
        tpm.process();
        assert!(!tpm.pending());
        assert_eq!(register(&mut tpm, 0x44), 1);
        tpm.snapshot().unwrap_err();
    }

    fn transfer(request: Vec<u8>) -> Transfer {
        Transfer {
            request,
            sent: 0,
            response: vec![0; 10],
            received: 0,
            deadline: Instant::now() + COMMAND_TIMEOUT,
        }
    }

    #[test]
    fn cancellation_is_deferred_and_preserves_backend_response() {
        let mut tpm = disconnected_tpm();
        let (backend, mut control, mut data) = Backend::cancellable_pair();
        tpm.backend = backend;
        tpm.transfer = Some(transfer(vec![]));
        tpm.write(0x48, &1u32.to_le_bytes());
        assert!(tpm.cancel);
        assert_eq!(register(&mut tpm, 0x48), 1);
        let server = std::thread::spawn(move || {
            let mut command = [0; 4];
            control.read_exact(&mut command).unwrap();
            assert_eq!(command, 9u32.to_be_bytes());
            data.write_all(&[0x80, 1, 0, 0, 0, 10, 0, 0, 9, 9]).unwrap();
            control.write_all(&[0; 4]).unwrap();
        });
        tpm.process();
        server.join().unwrap();
        assert!(!tpm.pending());
        assert!(!tpm.state.fatal);
        assert_eq!(&tpm.state.buffer[6..10], &[0, 0, 9, 9]);
        assert_eq!(register(&mut tpm, 0x48), 1);
        tpm.write(0x48, &0u32.to_le_bytes());
        assert_eq!(register(&mut tpm, 0x48), 0);
    }

    #[test]
    fn snapshot_rejects_inflight_command_and_timer_can_finish_without_guest_polling() {
        let mut tpm = disconnected_tpm();
        let (backend, mut peer) = Backend::pending_pair();
        tpm.backend = backend;
        tpm.transfer = Some(transfer(vec![0x80, 1, 0, 0, 0, 10, 0, 0, 1, 0x43]));
        assert_eq!(
            tpm.snapshot().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(tpm.pending());
        let mut command = [0; 10];
        peer.read_exact(&mut command).unwrap();
        peer.write_all(&[0x80, 1, 0, 0, 0, 10, 0, 0, 0, 0]).unwrap();
        tpm.process();
        assert!(!tpm.pending());
        assert!(!tpm.state.fatal);
        assert_eq!(&tpm.state.buffer[..10], &[0x80, 1, 0, 0, 0, 10, 0, 0, 0, 0]);
    }

    #[test]
    fn partial_response_is_bounded_and_validated() {
        let (mut guest, mut peer) = UnixStream::pair().unwrap();
        guest.set_nonblocking(true).unwrap();
        let mut transfer = transfer(vec![1, 2, 3]);
        assert!(!transfer.progress(&mut guest).unwrap());
        let mut request = [0; 3];
        peer.read_exact(&mut request).unwrap();
        assert_eq!(request, [1, 2, 3]);
        peer.write_all(&[0x80, 1, 0, 0]).unwrap();
        assert!(!transfer.progress(&mut guest).unwrap());
        peer.write_all(&[0, 12, 0, 0, 0, 0]).unwrap();
        assert!(!transfer.progress(&mut guest).unwrap());
        peer.write_all(&[42, 43]).unwrap();
        assert!(transfer.progress(&mut guest).unwrap());
        assert_eq!(&transfer.response[10..], &[42, 43]);
    }

    #[test]
    fn malformed_response_cannot_allocate_unbounded_memory() {
        for length in [0u32, 9, 3969, u32::MAX] {
            let (mut guest, mut peer) = UnixStream::pair().unwrap();
            guest.set_nonblocking(true).unwrap();
            let mut transfer = transfer(vec![]);
            let mut response = vec![0x80, 1];
            response.extend_from_slice(&length.to_be_bytes());
            response.extend_from_slice(&[0; 4]);
            peer.write_all(&response).unwrap();
            assert_eq!(
                transfer.progress(&mut guest).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert_eq!(transfer.response.len(), 10);
        }
    }

    #[test]
    fn disconnected_backend_is_terminal() {
        let (mut guest, peer) = UnixStream::pair().unwrap();
        drop(peer);
        assert_eq!(
            transfer(vec![]).progress(&mut guest).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    struct Swtpm {
        process: std::process::Child,
        path: std::path::PathBuf,
    }

    impl Swtpm {
        fn start(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("tpm-test-{}-{name}", std::process::id()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .unwrap();
            let process = std::process::Command::new("swtpm")
                .args(["socket", "--tpm2", "--tpmstate"])
                .arg(format!("dir={}", path.display()))
                .arg("--ctrl")
                .arg(format!("type=unixio,path={}/ctrl", path.display()))
                .args(["--flags", "not-need-init,disable-auto-shutdown"])
                .spawn()
                .unwrap();
            let fixture = Self { process, path };
            for _ in 0..200 {
                if fixture.path.join("ctrl").exists() {
                    return fixture;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("swtpm socket was not created");
        }
    }

    impl Drop for Swtpm {
        fn drop(&mut self) {
            let _ = self.process.kill();
            let _ = self.process.wait();
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn command(tpm: &mut Tpm, command: &[u8]) -> Vec<u8> {
        tpm.write(8, &1u32.to_le_bytes());
        tpm.write(0x40, &1u32.to_le_bytes());
        for (offset, byte) in command.iter().enumerate() {
            tpm.write(u64::try_from(BUFFER_OFFSET + offset).unwrap(), &[*byte]);
        }
        tpm.write(0x4c, &1u32.to_le_bytes());
        let deadline = Instant::now() + Duration::from_secs(5);
        while tpm.transfer.is_some() && Instant::now() < deadline {
            tpm.process();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!tpm.state.fatal);
        assert!(tpm.transfer.is_none());
        let length = u32::from_be_bytes(tpm.state.buffer[2..6].try_into().unwrap()) as usize;
        let response = tpm.state.buffer[..length].to_vec();
        assert_eq!(&response[6..10], &[0; 4]);
        response
    }

    #[test]
    fn real_swtpm_crb_snapshot_restore() {
        let source = Swtpm::start("source");
        let mut tpm = Tpm::new(source.path.join("ctrl")).unwrap();
        let startup = [0x80, 1, 0, 0, 0, 12, 0, 0, 1, 0x44, 0, 0];
        command(&mut tpm, &startup);
        let read_pcr = [
            0x80, 1, 0, 0, 0, 20, 0, 0, 1, 0x7e, 0, 0, 0, 1, 0, 0x0b, 3, 1, 0, 0,
        ];
        let before = command(&mut tpm, &read_pcr);
        let mut extend = vec![
            0x80, 2, 0, 0, 0, 65, 0, 0, 1, 0x82, 0, 0, 0, 0, 0, 0, 0, 9, 0x40, 0, 0, 9, 0, 0, 0, 0,
            0, 0, 0, 0, 1, 0, 0x0b,
        ];
        extend.extend_from_slice(&[0x5a; 32]);
        command(&mut tpm, &extend);
        let after = command(&mut tpm, &read_pcr);
        assert_ne!(before, after);
        let snapshot = tpm.snapshot().unwrap();
        snapshot.validate().unwrap();
        let destination = Swtpm::start("destination");
        let mut restored = Tpm::restore(destination.path.join("ctrl"), &snapshot).unwrap();
        assert_eq!(command(&mut restored, &read_pcr), after);
        assert_eq!(command(&mut tpm, &read_pcr), after);
        let mut invalid = snapshot;
        invalid.buffer.pop();
        invalid.validate().unwrap_err();
    }
}
