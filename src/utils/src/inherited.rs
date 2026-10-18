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

pub fn drives(
    arguments: &crate::arg_parser::Arguments,
) -> io::Result<std::collections::BTreeMap<String, RawFd>> {
    let mut drives = std::collections::BTreeMap::new();
    for value in arguments.multiple_values("drive-fd").unwrap_or_default() {
        let (id, fd) = value.split_once('=').ok_or(io::ErrorKind::InvalidInput)?;
        if id.is_empty() || id.contains('/') || drives.insert(id.to_owned(), number(fd)?).is_some()
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
    }
    Ok(drives)
}

pub fn block(file: &File, read_only: bool) -> io::Result<()> {
    use std::os::unix::fs::FileTypeExt;
    let kind = file.metadata()?.file_type();
    if !kind.is_file() && !kind.is_block_device() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    // SAFETY: file owns a live descriptor; F_GETFL takes no pointer argument.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let access = flags & libc::O_ACCMODE;
    if flags & (libc::O_PATH | libc::O_APPEND) != 0
        || access == libc::O_WRONLY
        || (!read_only && access != libc::O_RDWR)
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn block_rejects_special_files_and_read_only_writes() {
        let file = File::open("/dev/null").unwrap();
        assert!(block(&file, true).is_err());
        let file = File::open("Cargo.toml").unwrap();
        assert!(block(&file, true).is_ok());
        assert!(block(&file, false).is_err());
        let directory = File::open(".").unwrap();
        assert!(block(&directory, true).is_err());
    }
}
