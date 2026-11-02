// Copyright 2021 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::fs::File;
use std::io::{ErrorKind, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileTypeExt;

use vm_memory::{GuestMemoryBackend, GuestMemoryError, VolatileMemoryError, VolatileSlice};

use crate::vstate::memory::{Bitmap, GuestAddress, GuestMemoryMmap};

#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum SyncIoError {
    /// Discard: {0}
    Discard(std::io::Error),
    /// Flush: {0}
    Flush(std::io::Error),
    /// Seek: {0}
    Seek(std::io::Error),
    /// SyncAll: {0}
    SyncAll(std::io::Error),
    /// Transfer: {0}
    Transfer(GuestMemoryError),
}

#[derive(Debug)]
pub struct SyncFileEngine {
    file: File,
}

// SAFETY: `File` is send and ultimately a POD.
unsafe impl Send for SyncFileEngine {}

const BLKDISCARD: libc::Ioctl = 0x1277;
const FALLOC_FL_KEEP_SIZE: libc::c_int = 0x01;
const FALLOC_FL_PUNCH_HOLE: libc::c_int = 0x02;

pub(super) fn file_discard_range(file: &File, range: (u64, u32)) -> Result<u32, std::io::Error> {
    let file_type = file.metadata()?.file_type();
    let (offset, len) = range;

    if len == 0 {
        return Ok(0);
    }
    let discarded = len;
    let len = u64::from(len);

    if file_type.is_block_device() {
        let mut range = [offset, len];
        // SAFETY: file is a valid fd, BLKDISCARD expects a pointer to two u64 values
        // representing byte offset and byte length.
        let ret = unsafe { libc::ioctl(file.as_raw_fd(), BLKDISCARD, range.as_mut_ptr()) };
        if ret < 0 {
            return Err(std::io::Error::last_os_error());
        }
    } else {
        let off = libc::off_t::try_from(offset).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "discard offset overflow")
        })?;
        let len = libc::off_t::try_from(len).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "discard length overflow")
        })?;
        // SAFETY: file is a valid fd and fallocate does not retain the passed values.
        let ret = unsafe {
            libc::fallocate(
                file.as_raw_fd(),
                FALLOC_FL_KEEP_SIZE | FALLOC_FL_PUNCH_HOLE,
                off,
                len,
            )
        };
        if ret < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    Ok(discarded)
}

impl SyncFileEngine {
    pub fn from_file(file: File) -> SyncFileEngine {
        SyncFileEngine { file }
    }

    pub fn file(&self) -> &File {
        &self.file
    }

    /// Update the backing file of the engine
    pub fn update_file(&mut self, file: File) {
        self.file = file
    }

    pub fn read(
        &mut self,
        offset: u64,
        mem: &GuestMemoryMmap,
        addr: GuestAddress,
        count: u32,
    ) -> Result<u32, SyncIoError> {
        let offset = libc::off_t::try_from(offset)
            .map_err(|e| SyncIoError::Seek(std::io::Error::new(ErrorKind::InvalidInput, e)))?;
        offset.checked_add(i64::from(count)).ok_or_else(|| {
            SyncIoError::Seek(std::io::Error::new(
                ErrorKind::InvalidInput,
                "file offset overflow",
            ))
        })?;
        let slice = mem
            .get_slice(addr, count as usize)
            .map_err(SyncIoError::Transfer)?;
        // SAFETY: `get_slice` checked that `count` bytes at this address are guest memory, which
        // the device keeps mapped. The copy carries no bitmap: vm-memory would mark the whole
        // remainder dirty if a read failed, so marking is done below for the bytes read.
        let mut unmarked =
            unsafe { VolatileSlice::new(slice.ptr_guard_mut().as_ptr(), count as usize) };

        // Read until the buffer is full, then mark dirty exactly what was read. A failed `read`
        // writes nothing, so what the earlier ones filled is all there is to mark either way.
        let mut filled = 0;
        let result = loop {
            if unmarked.is_empty() {
                break Ok(());
            }
            let position = offset + i64::try_from(filled).unwrap();
            let guard = unmarked.ptr_guard_mut();
            // SAFETY: the guard retains the checked writable guest range for this syscall.
            let result = unsafe {
                libc::pread(
                    self.file.as_raw_fd(),
                    guard.as_ptr().cast(),
                    unmarked.len(),
                    position,
                )
            };
            let result = if result < 0 {
                Err(VolatileMemoryError::IOError(std::io::Error::last_os_error()))
            } else {
                Ok(usize::try_from(result).unwrap())
            };
            match result {
                Ok(0) => {
                    break Err(VolatileMemoryError::IOError(std::io::Error::new(
                        ErrorKind::UnexpectedEof,
                        "failed to fill whole buffer",
                    )));
                }
                Ok(read) => {
                    filled += read;
                    // `read` cannot exceed the slice's length.
                    unmarked = unmarked.offset(read).unwrap();
                }
                Err(VolatileMemoryError::IOError(err)) if err.kind() == ErrorKind::Interrupted => {}
                Err(err) => break Err(err),
            }
        };
        slice.bitmap().mark_dirty(0, filled);
        result.map_err(|err| SyncIoError::Transfer(err.into()))?;
        Ok(count)
    }

    pub fn write(
        &mut self,
        offset: u64,
        mem: &GuestMemoryMmap,
        addr: GuestAddress,
        count: u32,
    ) -> Result<u32, SyncIoError> {
        let offset = libc::off_t::try_from(offset)
            .map_err(|e| SyncIoError::Seek(std::io::Error::new(ErrorKind::InvalidInput, e)))?;
        offset.checked_add(i64::from(count)).ok_or_else(|| {
            SyncIoError::Seek(std::io::Error::new(
                ErrorKind::InvalidInput,
                "file offset overflow",
            ))
        })?;
        let slice = mem
            .get_slice(addr, count as usize)
            .map_err(SyncIoError::Transfer)?;
        let mut written = 0;
        while written < slice.len() {
            let position = offset + i64::try_from(written).unwrap();
            let remaining = slice
                .offset(written)
                .map_err(|e| SyncIoError::Transfer(e.into()))?;
            let guard = remaining.ptr_guard();
            // SAFETY: the guard retains the checked readable guest range for this syscall.
            let result = unsafe {
                libc::pwrite(
                    self.file.as_raw_fd(),
                    guard.as_ptr().cast(),
                    remaining.len(),
                    position,
                )
            };
            if result < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == ErrorKind::Interrupted {
                    continue;
                }
                return Err(SyncIoError::Transfer(
                    VolatileMemoryError::IOError(err).into(),
                ));
            }
            if result == 0 {
                return Err(SyncIoError::Transfer(
                    VolatileMemoryError::IOError(std::io::Error::new(
                        ErrorKind::WriteZero,
                        "failed to write whole buffer",
                    ))
                    .into(),
                ));
            }
            written += usize::try_from(result).unwrap();
        }
        Ok(count)
    }

    pub fn flush(&mut self) -> Result<(), SyncIoError> {
        // flush() first to force any cached data out of rust buffers.
        self.file.flush().map_err(SyncIoError::Flush)?;
        // Sync data out to physical media on host.
        self.file.sync_all().map_err(SyncIoError::SyncAll)
    }

    pub fn discard(&mut self, range: (u64, u32)) -> Result<u32, SyncIoError> {
        file_discard_range(&self.file, range).map_err(SyncIoError::Discard)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek, SeekFrom, Write};

    use vmm_sys_util::tempfile::TempFile;

    use super::file_discard_range;

    #[test]
    fn overflowing_range_fails_before_modifying_memory_or_file() {
        use super::{SyncFileEngine, SyncIoError};
        use crate::devices::virtio::test_utils::default_mem;
        use crate::vstate::memory::{Bytes, GuestAddress};

        let file = TempFile::new().unwrap().into_file();
        let mut engine = SyncFileEngine::from_file(file.try_clone().unwrap());
        let mem = default_mem();
        mem.write_slice(b"unchanged", GuestAddress(0)).unwrap();
        let offset = i64::MAX as u64 - 3;
        assert!(matches!(
            engine.read(offset, &mem, GuestAddress(0), 9),
            Err(SyncIoError::Seek(_))
        ));
        assert!(matches!(
            engine.write(offset, &mem, GuestAddress(0), 9),
            Err(SyncIoError::Seek(_))
        ));
        let mut bytes = [0; 9];
        mem.read_slice(&mut bytes, GuestAddress(0)).unwrap();
        assert_eq!(&bytes, b"unchanged");
        assert_eq!(file.metadata().unwrap().len(), 0);
    }

    #[test]
    fn cloned_descriptors_keep_their_shared_cursor() {
        use super::SyncFileEngine;
        use crate::devices::virtio::test_utils::default_mem;
        use crate::vstate::memory::{Bytes, GuestAddress};

        let mut file = TempFile::new().unwrap().into_file();
        file.set_len(4096).unwrap();
        file.seek(SeekFrom::Start(77)).unwrap();
        let mut first = SyncFileEngine::from_file(file.try_clone().unwrap());
        let mut second = SyncFileEngine::from_file(file.try_clone().unwrap());
        let mem = default_mem();
        mem.write_slice(b"first", GuestAddress(0)).unwrap();
        mem.write_slice(b"other", GuestAddress(64)).unwrap();
        first.write(512, &mem, GuestAddress(0), 5).unwrap();
        second.write(1024, &mem, GuestAddress(64), 5).unwrap();
        first.read(1024, &mem, GuestAddress(128), 5).unwrap();
        second.read(512, &mem, GuestAddress(192), 5).unwrap();
        let mut observed = [0; 5];
        mem.read_slice(&mut observed, GuestAddress(128)).unwrap();
        assert_eq!(&observed, b"other");
        mem.read_slice(&mut observed, GuestAddress(192)).unwrap();
        assert_eq!(&observed, b"first");
        assert_eq!(file.stream_position().unwrap(), 77);
    }

    #[test]
    fn test_discard_regular_file() {
        let mut file = TempFile::new().unwrap().into_file();
        file.write_all(&vec![0x5a; 4096]).unwrap();

        match file_discard_range(&file, (1024, 2048)) {
            Ok(discarded) => assert_eq!(discarded, 2048),
            // Some filesystems do not support hole punching; that is a host/filesystem
            // capability limitation, not a test failure for the regular discard path.
            Err(err)
                if matches!(
                    err.raw_os_error(),
                    Some(libc::EOPNOTSUPP) | Some(libc::ENOSYS)
                ) =>
            {
                return;
            }
            Err(err) => panic!("discard failed: {err}"),
        }

        file.seek(SeekFrom::Start(0)).unwrap();
        let mut data = vec![0u8; 4096];
        file.read_exact(&mut data).unwrap();

        assert_eq!(&data[..1024], vec![0x5a; 1024].as_slice());
        assert_eq!(&data[1024..3072], vec![0; 2048].as_slice());
        assert_eq!(&data[3072..], vec![0x5a; 1024].as_slice());
    }
}
