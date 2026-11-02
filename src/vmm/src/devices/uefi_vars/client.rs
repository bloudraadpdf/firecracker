// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

const MAX_FRAME: usize = 4 * 1024 * 1024;
pub(super) const MAX_CHECKPOINT: usize = MAX_FRAME - 256;
const TIMEOUT: Duration = Duration::from_secs(4);

pub(super) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(super) struct Client {
    stream: UnixStream,
    next_id: u32,
    pending: Option<Transfer>,
}

struct Transfer {
    opcode: u16,
    id: u32,
    request: Vec<u8>,
    sent: usize,
    header: [u8; 16],
    header_read: usize,
    response: Vec<u8>,
    received: usize,
    max_reply: usize,
    deadline: Instant,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UefiVarsClient")
            .field("pending", &self.pending.is_some())
            .finish_non_exhaustive()
    }
}

impl Client {
    pub(super) fn connect(path: &Path) -> io::Result<Self> {
        let stream = utils::unix_socket::connect(path)?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            next_id: 1,
            pending: None,
        })
    }

    pub(super) fn start(
        &mut self,
        opcode: u16,
        payload: &[u8],
        max_reply: usize,
    ) -> io::Result<()> {
        if self.pending.is_some() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        if payload.len() > MAX_FRAME || max_reply > MAX_FRAME {
            return Err(invalid("variable-service frame exceeds limit"));
        }
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or_else(|| invalid("variable-service request IDs exhausted"))?;
        let mut request = Vec::with_capacity(16 + payload.len());
        request.extend_from_slice(b"FCUV");
        request.extend_from_slice(&1u16.to_le_bytes());
        request.extend_from_slice(&opcode.to_le_bytes());
        request.extend_from_slice(&id.to_le_bytes());
        request.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
        request.extend_from_slice(payload);
        self.pending = Some(Transfer {
            opcode,
            id,
            request,
            sent: 0,
            header: [0; 16],
            header_read: 0,
            response: Vec::new(),
            received: 0,
            max_reply,
            deadline: Instant::now() + TIMEOUT,
        });
        Ok(())
    }

    pub(super) fn pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn progress(&mut self) -> io::Result<Option<Vec<u8>>> {
        let Some(p) = &mut self.pending else {
            return Ok(None);
        };
        if Instant::now() >= p.deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        if p.sent < p.request.len() {
            match self.stream.write(&p.request[p.sent..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => p.sent += n,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(e),
            }
            if p.sent < p.request.len() {
                return Ok(None);
            }
        }
        if p.header_read < 16 {
            match self.stream.read(&mut p.header[p.header_read..]) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => p.header_read += n,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(e),
            }
            if p.header_read < 16 {
                return Ok(None);
            }
            if &p.header[..4] != b"FCUV"
                || p.header[4..6] != 1u16.to_le_bytes()
                || p.header[6..8] != (p.opcode | 0x8000).to_le_bytes()
                || p.header[8..12] != p.id.to_le_bytes()
            {
                return Err(invalid("variable-service reply header mismatch"));
            }
            let size =
                usize::try_from(u32::from_le_bytes(p.header[12..16].try_into().unwrap())).unwrap();
            if size < 4 || size - 4 > p.max_reply {
                return Err(invalid("variable-service reply exceeds limit"));
            }
            p.response.resize(size, 0);
        }
        match self.stream.read(&mut p.response[p.received..]) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => p.received += n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e),
        }
        if p.received != p.response.len() {
            return Ok(None);
        }
        let p = self.pending.take().unwrap();
        if p.response[..4] != [0; 4] {
            return Err(invalid("variable service rejected request"));
        }
        Ok(Some(p.response[4..].to_vec()))
    }

    pub(super) fn exchange(
        &mut self,
        opcode: u16,
        payload: &[u8],
        max_reply: usize,
    ) -> io::Result<Vec<u8>> {
        self.start(opcode, payload, max_reply)?;
        loop {
            if let Some(reply) = self.progress()? {
                return Ok(reply);
            }
            let delay = libc::timespec {
                tv_sec: 0,
                tv_nsec: 1_000_000,
            };
            // SAFETY: delay points to a valid timespec; no remaining-time output is requested.
            let result = unsafe {
                libc::clock_nanosleep(libc::CLOCK_MONOTONIC, 0, &delay, std::ptr::null_mut())
            };
            if result != 0 && result != libc::EINTR {
                return Err(io::Error::from_raw_os_error(result));
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(crate) fn pair() -> (Client, UnixStream) {
        let (stream, peer) = UnixStream::pair().unwrap();
        stream.set_nonblocking(true).unwrap();
        (
            Client {
                stream,
                next_id: 1,
                pending: None,
            },
            peer,
        )
    }

    #[test]
    fn largest_checkpoint_fits_restore_envelope() {
        let (mut client, _peer) = pair();
        let checkpoint = serde_json::json!({"data": "x".repeat(MAX_CHECKPOINT - 11)});
        assert_eq!(
            serde_json::to_vec(&checkpoint).unwrap().len(),
            MAX_CHECKPOINT
        );
        let payload = serde_json::to_vec(
            &serde_json::json!({"expected_generation": u64::MAX, "checkpoint": checkpoint}),
        )
        .unwrap();
        client.start(3, &payload, 0).unwrap();
    }

    #[test]
    fn mismatched_reply_id_and_oversized_body_are_rejected_before_allocation() {
        for (id, size) in [(2u32, 4u32), (1, u32::MAX)] {
            let (mut client, mut peer) = pair();
            client.start(2, &[], 32).unwrap();
            assert!(client.progress().unwrap().is_none());
            let mut request = [0; 16];
            peer.read_exact(&mut request).unwrap();
            request[6..8].copy_from_slice(&0x8002u16.to_le_bytes());
            request[8..12].copy_from_slice(&id.to_le_bytes());
            request[12..16].copy_from_slice(&size.to_le_bytes());
            peer.write_all(&request).unwrap();
            client.progress().unwrap_err();
            assert!(client.pending.as_ref().unwrap().response.is_empty());
        }
    }

    #[test]
    fn partial_reply_and_backend_failure_do_not_report_success() {
        let (mut client, mut peer) = pair();
        client.start(4, &[], 0).unwrap();
        client.progress().unwrap();
        let mut header = [0; 16];
        peer.read_exact(&mut header).unwrap();
        header[6..8].copy_from_slice(&0x8004u16.to_le_bytes());
        header[12..16].copy_from_slice(&4u32.to_le_bytes());
        peer.write_all(&header[..8]).unwrap();
        assert!(client.progress().unwrap().is_none());
        peer.write_all(&header[8..]).unwrap();
        peer.write_all(&3u32.to_le_bytes()).unwrap();
        client.progress().unwrap_err();
    }

    #[test]
    fn silent_helper_has_a_bounded_deadline() {
        let (mut client, _peer) = pair();
        client.start(2, &[], 32).unwrap();
        client.pending.as_mut().unwrap().deadline = Instant::now();
        assert_eq!(
            client.progress().unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }
}
