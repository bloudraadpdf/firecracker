// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Unix stream connections which cannot block on a full listener backlog.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;

/// Validate a nonempty filesystem socket path with room for its terminating NUL.
pub fn validate(path: &Path) -> io::Result<()> {
    let bytes = path.as_os_str().as_bytes();
    let capacity =
        size_of::<libc::sockaddr_un>() - std::mem::offset_of!(libc::sockaddr_un, sun_path);
    if bytes.is_empty() || bytes.len() >= capacity || bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid Unix socket path",
        ));
    }
    Ok(())
}

/// Connect immediately or return an error, then restore blocking I/O mode.
/// The caller must configure read/write deadlines before exchanging data.
pub fn connect(path: &Path) -> io::Result<UnixStream> {
    // SAFETY: sockaddr_un contains only integer fields and an inline byte array.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    validate(path)?;
    let bytes = path.as_os_str().as_bytes();
    address.sun_family = u16::try_from(libc::AF_UNIX).unwrap();
    for (target, byte) in address.sun_path.iter_mut().zip(bytes) {
        *target = libc::c_char::from_ne_bytes([*byte]);
    }
    // SAFETY: socket has no pointer arguments; the returned descriptor is checked and owned.
    let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: socket returned a fresh valid descriptor above.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let stream = UnixStream::from(fd);
    stream.set_nonblocking(true)?;
    // SAFETY: address is a fully initialised sockaddr_un with a terminated pathname.
    let result = unsafe {
        libc::connect(
            stream.as_raw_fd(),
            (&raw const address).cast(),
            libc::socklen_t::try_from(size_of::<libc::sockaddr_un>()).unwrap(),
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;

    struct SocketPath(PathBuf);
    impl Drop for SocketPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn full_unix_backlog_fails_without_blocking() {
        const CHILD: &str = "FIRECRACKER_UNIX_BACKLOG_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "unix_socket::tests::full_unix_backlog_fails_without_blocking",
                    "--test-threads=1",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stdout)
            );
            return;
        }
        let directory = std::env::temp_dir();
        let path = SocketPath(directory.join(format!("unix-backlog-{}", std::process::id())));
        let listener = UnixListener::bind(&path.0).unwrap();
        // SAFETY: listener owns a valid listening socket; backlog zero permits one queued peer.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
        let first = connect(&path.0).unwrap();
        let before = std::fs::read_dir("/proc/self/fd").unwrap().count();
        for _ in 0..64 {
            assert_eq!(
                connect(&path.0).unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
        }
        assert_eq!(std::fs::read_dir("/proc/self/fd").unwrap().count(), before);
        // SAFETY: first remains a live descriptor owned by this scope.
        let flags = unsafe { libc::fcntl(first.as_raw_fd(), libc::F_GETFD) };
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
    }

    #[test]
    fn invalid_unix_paths_are_rejected() {
        for bytes in [vec![], vec![b'a'; 108], vec![b'a', 0, b'b']] {
            let path = Path::new(std::ffi::OsStr::from_bytes(&bytes));
            assert_eq!(
                connect(path).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }
}
