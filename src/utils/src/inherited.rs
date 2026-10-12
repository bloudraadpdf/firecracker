// Copyright 2026 Bloudraad Ltd
// SPDX-License-Identifier: Apache-2.0

//! Validated, exclusively owned launch descriptors.

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

pub fn number(value: &str) -> io::Result<RawFd> {
    value
        .parse()
        .ok()
        .filter(|fd| *fd > 2)
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))
}

pub fn socket(fd: RawFd, kind: libc::c_int, listening: bool) -> io::Result<()> {
    for (option, expected) in [
        (libc::SO_DOMAIN, libc::AF_UNIX),
        (libc::SO_TYPE, kind),
        (libc::SO_ACCEPTCONN, i32::from(listening)),
    ] {
        let mut value = 0;
        let mut length = libc::socklen_t::try_from(size_of::<i32>()).unwrap();
        // SAFETY: value is an aligned output slot and length describes its size.
        if unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                option,
                (&raw mut value).cast(),
                &raw mut length,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        if length as usize != size_of::<i32>() || value != expected {
            return Err(io::ErrorKind::InvalidInput.into());
        }
    }
    Ok(())
}

/// # Safety
/// The caller must transfer the descriptor exactly once, with no other owner.
pub unsafe fn take(fd: RawFd) -> io::Result<OwnedFd> {
    if fd <= 2 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    // SAFETY: the caller transfers exclusive ownership of this descriptor.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: the owned descriptor is live; the flag affects only this process's table.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(owned)
}

pub fn pipe(file: &File) -> io::Result<()> {
    use std::os::unix::fs::FileTypeExt;
    if !file.metadata()?.file_type().is_fifo() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    // SAFETY: file owns a live descriptor; F_GETFL does not take a third argument.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if flags & libc::O_ACCMODE != libc::O_WRONLY || flags & libc::O_NONBLOCK == 0 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(())
}
