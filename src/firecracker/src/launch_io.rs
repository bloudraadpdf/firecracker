// Copyright 2026 Bloudraad Ltd
// SPDX-License-Identifier: Apache-2.0

use std::fs::File;
use std::io;
use std::os::unix::net::UnixStream;

use utils::arg_parser::Arguments;
use utils::inherited;
use vmm::devices::virtio::vsock::InitialControl;
use vmm::resources::LaunchIo;

pub fn parse(arguments: &Arguments, api: Option<i32>) -> io::Result<LaunchIo> {
    let number = |name| {
        arguments
            .single_value(name)
            .map(|value| inherited::number(value))
            .transpose()
    };
    let control = number("control-fd")?;
    let serial = number("serial-out-fd")?;
    let mut numbers = [api, control, serial]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    numbers.sort_unstable();
    if numbers.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let port = arguments
        .single_value("control-port")
        .map(|value| value.parse::<u32>())
        .transpose()
        .map_err(|_| io::ErrorKind::InvalidInput)?;
    if port.is_some() != control.is_some() || port == Some(0) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let control = control
        .zip(port)
        .map(|(fd, port)| {
            inherited::socket(fd, libc::SOCK_STREAM, false)?;
            // SAFETY: CLI roles were checked for duplicates and this role is consumed once.
            let stream = UnixStream::from(unsafe { inherited::take(fd)? });
            stream.peer_addr()?;
            stream.set_nonblocking(true)?;
            Ok::<_, io::Error>(InitialControl { port, stream })
        })
        .transpose()?;
    let serial = serial
        .map(|fd| {
            // SAFETY: CLI roles were checked for duplicates and this role is consumed once.
            let file = File::from(unsafe { inherited::take(fd)? });
            inherited::pipe(&file)?;
            Ok::<_, io::Error>(file)
        })
        .transpose()?;
    Ok(LaunchIo { control, serial })
}
