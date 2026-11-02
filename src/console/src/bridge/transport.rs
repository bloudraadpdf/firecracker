// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

use utils::unix_socket::SocketPath;

use super::{TIMEOUT, configure};
use crate::invalid;

pub(super) enum Listener {
    Tcp(TcpListener),
    Unix {
        socket: UnixListener,
        _path: SocketPath,
    },
}

impl Listener {
    pub fn bind(address: &str) -> io::Result<Self> {
        if let Some(path) = address.strip_prefix("unix:") {
            let path = Path::new(path);
            utils::unix_socket::validate(path)?;
            let listener = UnixListener::bind(path)?;
            let owned = SocketPath::capture(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            return Ok(Self::Unix {
                socket: listener,
                _path: owned,
            });
        }
        let address: SocketAddr = address
            .parse()
            .map_err(|_| invalid("invalid bind address"))?;
        if !address.ip().is_loopback() {
            return Err(invalid(
                "console listeners must bind to loopback; use an authenticated remote gateway or SSH tunnel",
            ));
        }
        Ok(Self::Tcp(TcpListener::bind(address)?))
    }

    pub fn address(&self) -> io::Result<String> {
        match self {
            Self::Tcp(listener) => Ok(listener.local_addr()?.to_string()),
            Self::Unix { socket, .. } => socket
                .local_addr()?
                .as_pathname()
                .map(|path| format!("unix:{}", path.display()))
                .ok_or_else(|| invalid("missing Unix socket path")),
        }
    }

    pub fn accept(&self) -> io::Result<Stream> {
        match self {
            Self::Tcp(listener) => listener.accept().map(|(stream, _)| Stream::Tcp(stream)),
            Self::Unix { socket, .. } => socket.accept().map(|(stream, _)| Stream::Unix(stream)),
        }
    }
}

#[derive(Debug)]
pub(super) enum Stream {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl Stream {
    pub fn configure(&self) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => configure(stream),
            Self::Unix(stream) => {
                stream.set_read_timeout(Some(TIMEOUT))?;
                stream.set_write_timeout(Some(TIMEOUT))
            }
        }
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        match self {
            Self::Tcp(stream) => stream.try_clone().map(Self::Tcp),
            Self::Unix(stream) => stream.try_clone().map(Self::Unix),
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.set_read_timeout(timeout),
            Self::Unix(stream) => stream.set_read_timeout(timeout),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.read(buf),
            Self::Unix(stream) => stream.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.write(buf),
            Self::Unix(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.flush(),
            Self::Unix(stream) => stream.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_listener_keeps_existing_paths_and_removes_its_private_socket() {
        let path = format!("console-transport-{}.sock", std::process::id());
        let endpoint = format!("unix:{path}");
        let file = std::fs::File::create_new(&path).unwrap();
        assert!(Listener::bind(&endpoint).is_err());
        assert!(std::fs::metadata(&path).unwrap().is_file());
        drop(file);
        std::fs::remove_file(&path).unwrap();
        let listener = Listener::bind(&endpoint).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(listener);
        assert!(!Path::new(&path).exists());
    }

    #[test]
    fn listener_rejects_invalid_paths_and_non_loopback_addresses() {
        for endpoint in ["unix:", "unix:a\0b", "0.0.0.0:0", "[::]:0", "bad-address"] {
            assert!(Listener::bind(endpoint).is_err(), "{endpoint:?}");
        }
        assert!(Listener::bind(&format!("unix:{}", "a".repeat(108))).is_err());
    }
}
