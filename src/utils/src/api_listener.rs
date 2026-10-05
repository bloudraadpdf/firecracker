// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Inherited API listening socket descriptor.

use std::io;
use std::os::fd::RawFd;
use std::str::FromStr;

#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum ApiListenerError {
    /// Invalid API socket descriptor {0}: it must be a number above 2
    Number(String),
    /// Failed to inspect API socket descriptor {0}: {1}
    Inspect(RawFd, io::Error),
    /// API socket descriptor {0} is not a listening Unix stream socket
    NotListener(RawFd),
}

/// Descriptor number of an inherited API socket. It is never a standard stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiListenerFd(RawFd);

impl ApiListenerFd {
    pub fn raw(self) -> RawFd {
        self.0
    }
}

impl FromStr for ApiListenerFd {
    type Err = ApiListenerError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.parse() {
            Ok(fd) if fd > libc::STDERR_FILENO => Ok(Self(fd)),
            _ => Err(ApiListenerError::Number(value.to_owned())),
        }
    }
}

/// Checks that `fd` is a listening AF_UNIX stream socket.
pub fn check_api_listener(fd: ApiListenerFd) -> Result<(), ApiListenerError> {
    let fd = fd.raw();
    // SAFETY: A zeroed `stat` is a valid value of this plain C structure.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat writes only into `stat`, which this function owns.
    if unsafe { libc::fstat(fd, &mut stat) } < 0 {
        return Err(ApiListenerError::Inspect(fd, io::Error::last_os_error()));
    }
    let listener = stat.st_mode & libc::S_IFMT == libc::S_IFSOCK
        && socket_option(fd, libc::SO_DOMAIN)? == libc::AF_UNIX
        && socket_option(fd, libc::SO_TYPE)? == libc::SOCK_STREAM
        && socket_option(fd, libc::SO_ACCEPTCONN)? == 1;
    if listener {
        Ok(())
    } else {
        Err(ApiListenerError::NotListener(fd))
    }
}

fn socket_option(fd: RawFd, option: libc::c_int) -> Result<libc::c_int, ApiListenerError> {
    let mut value: libc::c_int = 0;
    let mut length = libc::socklen_t::try_from(size_of::<libc::c_int>()).unwrap();
    // SAFETY: getsockopt writes at most `length` bytes into `value`, which this function owns.
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            option,
            (&raw mut value).cast(),
            &mut length,
        )
    };
    if result < 0 {
        return Err(ApiListenerError::Inspect(fd, io::Error::last_os_error()));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::net::TcpListener;
    use std::os::fd::AsRawFd;
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram, UnixListener, UnixStream};

    use super::*;

    fn api_listener_fd(fd: &impl AsRawFd) -> ApiListenerFd {
        fd.as_raw_fd().to_string().parse().unwrap()
    }

    #[test]
    fn test_number_above_standard_streams() {
        assert_eq!("3".parse::<ApiListenerFd>().unwrap().raw(), 3);
        for value in ["2", "0", "-1", "x", ""] {
            assert!(matches!(
                value.parse::<ApiListenerFd>(),
                Err(ApiListenerError::Number(number)) if number == value
            ));
        }
    }

    #[test]
    fn test_listening_unix_stream_passes() {
        let name = format!("api-listener-{}", std::process::id());
        let address = SocketAddr::from_abstract_name(name).unwrap();
        let listener = UnixListener::bind_addr(&address).unwrap();
        check_api_listener(api_listener_fd(&listener)).unwrap();
    }

    #[test]
    fn test_other_descriptors_fail() {
        let (stream, _) = UnixStream::pair().unwrap();
        let datagram = UnixDatagram::unbound().unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let file = File::open(std::env::current_exe().unwrap()).unwrap();
        for fd in [
            api_listener_fd(&stream),
            api_listener_fd(&datagram),
            api_listener_fd(&tcp),
            api_listener_fd(&file),
        ] {
            assert!(matches!(
                check_api_listener(fd),
                Err(ApiListenerError::NotListener(raw)) if raw == fd.raw()
            ));
        }
    }

    #[test]
    fn test_closed_descriptor_fails() {
        let fd = RawFd::MAX.to_string().parse().unwrap();
        assert!(matches!(
            check_api_listener(fd),
            Err(ApiListenerError::Inspect(_, err)) if err.raw_os_error() == Some(libc::EBADF)
        ));
    }
}
