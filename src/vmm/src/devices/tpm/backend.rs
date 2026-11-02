// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! swtpm v0.10.1 control protocol (include/swtpm/tpm_ioctl.h).

use std::io;
#[cfg(feature = "windows")]
use std::io::{Read, Write};
#[cfg(feature = "windows")]
use std::os::fd::AsRawFd;
#[cfg(feature = "windows")]
use std::os::unix::net::UnixStream;
#[cfg(feature = "windows")]
use std::path::Path;
#[cfg(feature = "windows")]
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
#[cfg(feature = "windows")]
use vmm_sys_util::sock_ctrl_msg::ScmSocket;

#[cfg(feature = "windows")]
use super::BUFFER_SIZE;

#[cfg(feature = "windows")]
const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);
pub(super) const STATE_LIMIT: usize = 8 * 1024 * 1024;
#[cfg(feature = "windows")]
const REQUIRED_CAPS: u32 = 1
    | (1 << 2)
    | (1 << 3)
    | (1 << 5)
    | (1 << 8)
    | (1 << 9)
    | (1 << 10)
    | (1 << 12)
    | (1 << 13)
    | (1 << 14)
    | (1 << 16);

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Blob {
    flags: u32,
    bytes: Vec<u8>,
}

#[cfg(all(test, feature = "windows"))]
pub(super) fn maximum_test_blobs() -> Vec<Blob> {
    vec![
        Blob {
            flags: 0,
            bytes: (0..=255).cycle().take(STATE_LIMIT - 2).collect(),
        },
        Blob {
            flags: 0,
            bytes: vec![1],
        },
        Blob {
            flags: 0,
            bytes: vec![2],
        },
    ]
}

impl std::fmt::Debug for Blob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Blob")
            .field("flags", &self.flags)
            .field("length", &self.bytes.len())
            .finish()
    }
}

#[derive(Debug)]
#[cfg(feature = "windows")]
pub(super) struct Backend {
    control: UnixStream,
    pub data: UnixStream,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(super) fn validate(blobs: &[Blob]) -> io::Result<()> {
    if blobs.len() != 3
        || blobs.iter().take(2).any(|blob| blob.bytes.is_empty())
        || blobs.iter().any(|blob| blob.flags & !2 != 0)
        || blobs
            .iter()
            .try_fold(0usize, |total, blob| total.checked_add(blob.bytes.len()))
            .is_none_or(|total| total > STATE_LIMIT)
    {
        return Err(invalid("invalid TPM snapshot blobs"));
    }
    Ok(())
}

#[cfg(feature = "windows")]
impl Backend {
    #[cfg(all(test, feature = "windows"))]
    pub fn disconnected() -> Self {
        Self::cancellable_pair().0
    }

    #[cfg(all(test, feature = "windows"))]
    pub fn pending_pair() -> (Self, UnixStream) {
        let (backend, _, peer) = Self::cancellable_pair();
        (backend, peer)
    }

    #[cfg(all(test, feature = "windows"))]
    pub fn cancellable_pair() -> (Self, UnixStream, UnixStream) {
        let (control, control_peer) = UnixStream::pair().unwrap();
        let (data, data_peer) = UnixStream::pair().unwrap();
        data.set_nonblocking(true).unwrap();
        (Self { control, data }, control_peer, data_peer)
    }

    pub fn connect(path: &Path) -> io::Result<Self> {
        let control = utils::unix_socket::connect(path)?;
        control.set_read_timeout(Some(CONTROL_TIMEOUT))?;
        control.set_write_timeout(Some(CONTROL_TIMEOUT))?;
        let (data, peer) = UnixStream::pair()?;
        data.set_nonblocking(true)?;
        let mut backend = Self { control, data };
        let caps = backend.command::<4>(1, &[])?;
        if u32::from_be_bytes(caps) & REQUIRED_CAPS != REQUIRED_CAPS {
            return Err(invalid("swtpm lacks required capabilities"));
        }
        backend.verify_version()?;
        backend.command::<0>(14, &[])?;
        let command = 16u32.to_be_bytes();
        let sent = backend
            .control
            .send_with_fds(&[command.as_slice()], &[peer.as_raw_fd()])
            .map_err(|error| io::Error::from_raw_os_error(error.errno()))?;
        if sent != command.len() {
            return Err(io::ErrorKind::WriteZero.into());
        }
        backend.result()?;
        let size = backend.command::<12>(17, &u32::try_from(BUFFER_SIZE).unwrap().to_be_bytes())?;
        if u32::from_be_bytes(size[..4].try_into().unwrap()) != u32::try_from(BUFFER_SIZE).unwrap()
        {
            return Err(invalid("swtpm rejected CRB buffer size"));
        }
        Ok(backend)
    }

    fn verify_version(&mut self) -> io::Result<()> {
        let mut request = [0; 16];
        request[..8].copy_from_slice(&1u64.to_be_bytes());
        let header = self.command::<8>(18, &request)?;
        let total = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let length = u32::from_be_bytes(header[4..].try_into().unwrap()) as usize;
        if total != length || !(2..=4096).contains(&length) {
            return Err(invalid("invalid TPM information length"));
        }
        let mut bytes = vec![0; length];
        self.read(&mut bytes)?;
        if bytes.pop() != Some(0) {
            return Err(invalid("unterminated TPM information"));
        }
        let info: serde_json::Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if info["TPMSpecification"]["family"] != "2.0" {
            return Err(invalid("backend must implement TPM 2.0"));
        }
        Ok(())
    }

    fn read(&mut self, bytes: &mut [u8]) -> io::Result<()> {
        let deadline = Instant::now() + CONTROL_TIMEOUT;
        let mut offset = 0;
        while offset < bytes.len() {
            self.control.set_read_timeout(Some(
                deadline
                    .checked_duration_since(Instant::now())
                    .filter(|duration| !duration.is_zero())
                    .ok_or(io::ErrorKind::TimedOut)?,
            ))?;
            match self.control.read(&mut bytes[offset..]) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(count) => offset += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let deadline = Instant::now() + CONTROL_TIMEOUT;
        let mut offset = 0;
        while offset < bytes.len() {
            self.control.set_write_timeout(Some(
                deadline
                    .checked_duration_since(Instant::now())
                    .filter(|duration| !duration.is_zero())
                    .ok_or(io::ErrorKind::TimedOut)?,
            ))?;
            match self.control.write(&bytes[offset..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => offset += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn result(&mut self) -> io::Result<()> {
        let mut result = [0; 4];
        self.read(&mut result)?;
        if result != [0; 4] {
            return Err(io::Error::other(format!(
                "swtpm control error {:#x}",
                u32::from_be_bytes(result)
            )));
        }
        Ok(())
    }

    fn command<const N: usize>(&mut self, command: u32, request: &[u8]) -> io::Result<[u8; N]> {
        let mut message = command.to_be_bytes().to_vec();
        message.extend_from_slice(request);
        self.write(&message)?;
        self.result()?;
        let mut response = [0; N];
        self.read(&mut response)?;
        Ok(response)
    }

    pub fn initialize(&mut self, cold: bool) -> io::Result<bool> {
        self.command::<0>(2, &u32::from(cold).to_be_bytes())?;
        self.command::<0>(5, &[0])?;
        let established = self.command::<4>(4, &[])?;
        if established[0] > 1 {
            return Err(invalid("invalid TPM established flag"));
        }
        Ok(established[0] != 0)
    }

    pub fn cancel(&mut self) -> io::Result<()> {
        self.command::<0>(9, &[]).map(|_| ())
    }

    pub fn export(&mut self) -> io::Result<Vec<Blob>> {
        let mut blobs = Vec::with_capacity(3);
        let mut remaining = STATE_LIMIT;
        for kind in 1u32..=3 {
            let mut request = [0; 12];
            request[4..8].copy_from_slice(&kind.to_be_bytes());
            let mut message = 12u32.to_be_bytes().to_vec();
            message.extend_from_slice(&request);
            self.write(&message)?;
            let mut response = [0; 16];
            self.read(&mut response)?;
            let result = u32::from_be_bytes(response[..4].try_into().unwrap());
            let header = &response[4..];
            let flags = u32::from_be_bytes(header[..4].try_into().unwrap());
            let total = u32::from_be_bytes(header[4..8].try_into().unwrap()) as usize;
            let length = u32::from_be_bytes(header[8..].try_into().unwrap()) as usize;
            // swtpm uses TPM_RETRY for an absent (empty) state file.
            if (result != 0 && !(result == 0x800 && total == 0 && length == 0))
                || flags & !2 != 0
                || total != length
                || length > remaining
            {
                return Err(invalid("invalid or excessive TPM state blob"));
            }
            let mut bytes = vec![0; length];
            self.read(&mut bytes)?;
            remaining -= length;
            blobs.push(Blob { flags, bytes });
        }
        // GET_STATEBLOB(SAVESTATE) releases swtpm's storage lock.
        self.command::<0>(19, &0u32.to_be_bytes())?;
        validate(&blobs)?;
        Ok(blobs)
    }

    pub fn import(&mut self, blobs: &[Blob]) -> io::Result<bool> {
        validate(blobs)?;
        for (index, blob) in blobs.iter().enumerate() {
            if blob.bytes.is_empty() {
                continue;
            }
            let mut message = 13u32.to_be_bytes().to_vec();
            message.extend_from_slice(&blob.flags.to_be_bytes());
            message.extend_from_slice(&u32::try_from(index + 1).unwrap().to_be_bytes());
            message.extend_from_slice(&u32::try_from(blob.bytes.len()).unwrap().to_be_bytes());
            message.extend_from_slice(&blob.bytes);
            self.write(&message)?;
            self.result()?;
        }
        self.initialize(false)
    }
}

#[cfg(test)]
#[cfg(feature = "windows")]
mod tests {
    use super::*;

    #[test]
    fn state_export_rejects_invalid_headers_before_allocation() {
        for (result, flags, total, length) in [
            (0, 0, u32::MAX, u32::MAX),
            (0, 0, 1, 0),
            (0, 4, 0, 0),
            (0x800, 0, 1, 1),
            (1, 0, 0, 0),
        ] {
            let (control, mut peer) = UnixStream::pair().unwrap();
            let (data, _) = UnixStream::pair().unwrap();
            let mut backend = Backend { control, data };
            let server = std::thread::spawn(move || {
                let mut request = [0; 16];
                peer.read_exact(&mut request).unwrap();
                assert_eq!(&request[..4], &12u32.to_be_bytes());
                for value in [result, flags, total, length] {
                    peer.write_all(&value.to_be_bytes()).unwrap();
                }
            });
            assert_eq!(
                backend.export().unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn non_tpm2_backend_is_rejected_before_initialization() {
        let (mut backend, mut control, _) = Backend::cancellable_pair();
        let peer = std::thread::spawn(move || {
            let mut request = [0; 20];
            control.read_exact(&mut request).unwrap();
            assert_eq!(&request[..4], &18u32.to_be_bytes());
            let json = b"{\"TPMSpecification\":{\"family\":\"1.2\"}}\0";
            control.write_all(&[0; 4]).unwrap();
            for _ in 0..2 {
                control
                    .write_all(&u32::try_from(json.len()).unwrap().to_be_bytes())
                    .unwrap();
            }
            control.write_all(json).unwrap();
        });
        assert_eq!(
            backend.verify_version().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        peer.join().unwrap();
    }

    #[test]
    fn snapshot_blob_limits_are_validated_before_import() {
        validate(&[]).unwrap_err();
        let mut blobs = vec![
            Blob {
                flags: 0,
                bytes: vec![]
            };
            3
        ];
        validate(&blobs).unwrap_err();
        blobs[0].bytes.push(0);
        blobs[1].bytes.push(0);
        validate(&blobs).unwrap();
        blobs[1].flags = 1;
        validate(&blobs).unwrap_err();
        blobs[1].flags = 0;
        blobs[0].bytes.resize(STATE_LIMIT, 0);
        blobs[2].bytes.push(0);
        validate(&blobs).unwrap_err();
        assert!(!format!("{:?}", blobs[0]).contains("[0, 0"));
    }
}
