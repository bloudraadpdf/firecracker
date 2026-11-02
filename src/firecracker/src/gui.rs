// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::fs::File;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use console::{LocalBridge, ViewerOptions};
use utils::arg_parser::Arguments;

struct Vmm(Child);
impl Drop for Vmm {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn console_path(path: &str) -> io::Result<PathBuf> {
    let mut data = Vec::new();
    File::open(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut data)?;
    if data.len() > 1024 * 1024 {
        return Err(io::Error::other("GUI configuration exceeds 1 MiB"));
    }
    let config: serde_json::Value = serde_json::from_slice(&data)?;
    config
        .get("console")
        .and_then(|c| c.get("socket_path"))
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("--gui requires console.socket_path in --config-file"))
}

pub(super) fn run(arguments: &Arguments<'_>) -> io::Result<()> {
    let launch = arguments.flag_present("gui");
    let address = arguments.single_value("gui-connect");
    let password_file = arguments
        .single_value("gui-password-file")
        .map(PathBuf::from);
    if launch && address.is_some() {
        return Err(io::Error::other(
            "--gui and --gui-connect are mutually exclusive",
        ));
    }
    if let Some(address) = address {
        if arguments.single_value("config-file").is_some() {
            return Err(io::Error::other(
                "--gui-connect does not launch a VM; omit --config-file",
            ));
        }
        return console::view(ViewerOptions {
            address: address.parse().map_err(io::Error::other)?,
            password_file,
        });
    }
    if !launch || password_file.is_some() {
        return Err(io::Error::other(
            "--gui-password-file requires --gui-connect",
        ));
    }
    let config = arguments
        .single_value("config-file")
        .ok_or_else(|| io::Error::other("--gui requires --config-file"))?;
    if !arguments.flag_present("no-api") {
        return Err(io::Error::other(
            "--gui requires --no-api; use --gui-connect for an API-managed VM",
        ));
    }
    let socket = console_path(config)?;
    if socket.try_exists()? {
        return Err(io::Error::other(
            "console socket already exists; use --gui-connect for a running VM",
        ));
    }
    let mut command = Command::new(std::env::current_exe()?);
    command.args(std::env::args_os().skip(1).filter(|arg| arg != "--gui"));
    let parent = std::process::id();
    // SAFETY: the child uses only async-signal-safe Linux syscalls before exec.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                return Err(io::Error::last_os_error());
            }
            if u32::try_from(libc::getppid()).ok() != Some(parent) {
                return Err(io::Error::other("GUI parent exited during launch"));
            }
            Ok(())
        });
    }
    let mut child = Vmm(command.spawn()?);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !socket.try_exists()? {
        if let Some(status) = child.0.try_wait()? {
            return Err(io::Error::other(format!(
                "VMM exited before opening the console: {status}"
            )));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "VMM did not open its console within 30 seconds",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let socket_guard = utils::unix_socket::SocketPath::capture(&socket)?;
    let bridge = LocalBridge::start(socket)?;
    let result = console::view_with_child(
        ViewerOptions {
            address: bridge.address(),
            password_file: None,
        },
        &mut child.0,
    );
    drop(child);
    drop(bridge);
    drop(socket_guard);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dropping_child_guard_kills_and_reaps_the_process() {
        let child = Vmm(Command::new("sleep").arg("30").spawn().unwrap());
        let pid = child.0.id();
        drop(child);
        // SAFETY: signal zero checks existence without delivering a signal.
        assert_eq!(unsafe { libc::kill(i32::try_from(pid).unwrap(), 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }
}
