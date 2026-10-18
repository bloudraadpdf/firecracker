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
    let drives = inherited::drives(arguments)?;
    let control = number("control-fd")?;
    let serial = number("serial-out-fd")?;
    let mut numbers = [api, control, serial]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    numbers.extend(drives.values().copied());
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
    let drives = drives
        .into_iter()
        .map(|(id, fd)| {
            // SAFETY: all descriptor roles are distinct and each is consumed once.
            let file = File::from(unsafe { inherited::take(fd)? });
            inherited::block(&file, true)?;
            Ok((id, file))
        })
        .collect::<io::Result<_>>()?;
    Ok(LaunchIo {
        control,
        serial,
        drives,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, IntoRawFd};
    use utils::arg_parser::{ArgParser, Argument};

    fn parse_values(values: &[String]) -> io::Result<LaunchIo> {
        let mut parser = ArgParser::new();
        for name in ["control-fd", "control-port", "serial-out-fd", "drive-fd"] {
            parser = parser.arg(
                Argument::new(name)
                    .takes_value(true)
                    .allow_multiple(name == "drive-fd"),
            );
        }
        let mut args = parser.arguments().clone();
        args.parse(values).unwrap();
        parse(&args, None)
    }

    #[test]
    fn drive_roles_are_unique_and_retained_until_owner_drop() {
        let fixture = vmm_sys_util::tempfile::TempFile::new().unwrap();
        let file = fixture.as_file().try_clone().unwrap();
        let fd = file.as_raw_fd();
        for values in [
            vec![
                "vm".into(),
                "--drive-fd".into(),
                format!("a={fd}"),
                "--drive-fd".into(),
                format!("b={fd}"),
            ],
            vec![
                "vm".into(),
                "--drive-fd".into(),
                format!("a={fd}"),
                "--serial-out-fd".into(),
                fd.to_string(),
            ],
            vec![
                "vm".into(),
                "--drive-fd".into(),
                format!("a={fd}"),
                "--drive-fd".into(),
                format!("a={}", fd + 1),
            ],
        ] {
            assert!(parse_values(&values).is_err());
            assert!(file.metadata().is_ok());
        }
        let fd = file.into_raw_fd();
        let values = vec!["vm".into(), "--drive-fd".into(), format!("volume={fd}")];
        let owned = parse_values(&values).unwrap();
        assert!(owned.drives["volume"].metadata().is_ok());
        drop(owned);
        use std::os::unix::fs::MetadataExt;
        let expected = fixture.as_file().metadata().unwrap();
        if let Ok(current) = std::fs::metadata(format!("/proc/self/fd/{fd}")) {
            assert_ne!(
                (current.dev(), current.ino()),
                (expected.dev(), expected.ino())
            );
        }
    }
}
