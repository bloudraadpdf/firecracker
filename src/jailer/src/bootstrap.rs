// Copyright 2026 Bloudraad Ltd
// SPDX-License-Identifier: Apache-2.0

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixDatagram;
use std::path::Path;

use vmm_sys_util::sock_ctrl_msg::ScmSocket;

use crate::JailerError;

pub fn channel(fd: i32) -> Result<UnixDatagram, JailerError> {
    utils::inherited::socket(fd, libc::SOCK_DGRAM, false).map_err(JailerError::Launch)?;
    // SAFETY: main rejects duplicate launch descriptors; Env consumes this role once.
    let fd = unsafe { utils::inherited::take(fd) }.map_err(JailerError::Launch)?;
    Ok(UnixDatagram::from(fd))
}

pub fn join(fd: i32, root: &Path) -> Result<File, JailerError> {
    // SAFETY: main rejects duplicate launch descriptors; this function consumes the namespace role once.
    let namespace = File::from(unsafe { utils::inherited::take(fd) }.map_err(JailerError::Launch)?);
    // NS_GET_NSTYPE has no pointer argument and returns the namespace clone flag.
    // SAFETY: the namespace descriptor is live.
    let kind = unsafe { libc::ioctl(namespace.as_raw_fd(), 0xb703) };
    if kind != libc::CLONE_NEWNS {
        return Err(JailerError::Launch(io::ErrorKind::InvalidInput.into()));
    }
    let expected = std::fs::metadata(root).map_err(JailerError::Launch)?;
    // SAFETY: jailer is single-threaded; the validated descriptor names a mount namespace.
    if unsafe { libc::setns(namespace.as_raw_fd(), libc::CLONE_NEWNS) } < 0 {
        return Err(JailerError::Launch(io::Error::last_os_error()));
    }
    let actual = std::fs::metadata("/").map_err(JailerError::Launch)?;
    if (actual.dev(), actual.ino()) != (expected.dev(), expected.ino()) {
        return Err(JailerError::Launch(io::ErrorKind::PermissionDenied.into()));
    }
    Ok(namespace)
}

pub fn clone_vmm() -> Result<(i32, Option<OwnedFd>), JailerError> {
    let mut pidfd = -1;
    // SAFETY: clone_args is an integer-only kernel ABI structure; zero disables unused options.
    let mut args: libc::clone_args = unsafe { std::mem::zeroed() };
    args.flags = u64::from((libc::CLONE_NEWPID | libc::CLONE_PIDFD).cast_unsigned());
    args.pidfd = (&raw mut pidfd) as u64;
    args.exit_signal = u64::from(libc::SIGCHLD.cast_unsigned());
    // SAFETY: jailer is single-threaded. No CLONE_VM is used, so child stack and Rust ownership
    // are copied; only the parent receives the newly allocated pidfd.
    let pid = unsafe { libc::syscall(libc::SYS_clone3, &args, size_of::<libc::clone_args>()) };
    if pid < 0 {
        return Err(JailerError::Clone(io::Error::last_os_error()));
    }
    let descriptor = if pid == 0 {
        None
    } else {
        // SAFETY: clone3 succeeded in the parent and wrote a new owned pidfd.
        Some(unsafe { OwnedFd::from_raw_fd(pidfd) })
    };
    Ok((
        i32::try_from(pid).map_err(|_| JailerError::Launch(io::ErrorKind::InvalidData.into()))?,
        descriptor,
    ))
}

pub fn executable(fd: i32) -> Result<File, JailerError> {
    // SAFETY: the launch protocol transfers this unique descriptor to the jailer.
    let file = File::from(unsafe { utils::inherited::take(fd) }.map_err(JailerError::Launch)?);
    let seals = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    // SAFETY: F_GET_SEALS reads flags from the live descriptor.
    let actual = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GET_SEALS) };
    if actual < 0 || actual & seals != seals {
        return Err(JailerError::Launch(io::ErrorKind::InvalidInput.into()));
    }
    Ok(file)
}

pub fn publish(
    channel: &UnixDatagram,
    pid: i32,
    pidfd: &OwnedFd,
    namespace: &File,
    executable: &File,
) -> Result<(), JailerError> {
    let bytes = pid.to_le_bytes();
    let sent = channel
        .send_with_fds(
            &[bytes.as_slice()],
            &[
                pidfd.as_raw_fd(),
                namespace.as_raw_fd(),
                executable.as_raw_fd(),
            ],
        )
        .map_err(|error| JailerError::Launch(io::Error::from_raw_os_error(error.errno())))?;
    if sent != bytes.len() {
        return Err(JailerError::Launch(io::ErrorKind::WriteZero.into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::fd::{AsFd, IntoRawFd};
    use std::process::{Command, Stdio};

    #[test]
    fn retained_executable_requires_all_seals_and_closes_on_exec() {
        for seals in [
            0,
            libc::F_SEAL_WRITE,
            libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL,
        ] {
            // SAFETY: name is NUL-terminated; flags request a private sealable executable.
            let fd = unsafe {
                libc::memfd_create(
                    c"retained-exec-test".as_ptr(),
                    libc::MFD_ALLOW_SEALING | libc::MFD_EXEC,
                )
            };
            assert!(fd >= 0);
            // SAFETY: fd is live; F_ADD_SEALS takes the integer seal mask.
            assert_eq!(unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) }, 0);
            let result = executable(fd);
            if seals & libc::F_SEAL_SEAL == 0 {
                assert!(result.is_err());
            } else {
                let file = result.unwrap();
                assert_ne!(
                    // SAFETY: file owns the descriptor; F_GETFD only reads descriptor flags.
                    unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
                    0
                );
            }
        }
        let regular = File::open("/dev/null").unwrap();
        executable(regular.into_raw_fd()).unwrap_err();
    }

    #[test]
    #[ignore = "requires CAP_SYS_ADMIN; launches isolated namespace processes"]
    fn retained_namespace_outlives_creator_and_keeps_read_only_mount() {
        let base = std::env::current_dir()
            .unwrap()
            .join(format!(".namespace-test-{}", std::process::id()));
        std::fs::create_dir(&base).unwrap();
        let root = base.join("root");
        let source = base.join("source");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("restore")).unwrap();
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("mem"), b"immutable").unwrap();
        let command = || {
            let mut cmd = Command::new(std::env::current_exe().unwrap());
            cmd.args([
                "--exact",
                "bootstrap::tests::namespace_child",
                "--ignored",
                "--nocapture",
            ])
            .env("NAMESPACE_TEST_ROOT", &root)
            .env("NAMESPACE_TEST_SOURCE", &source);
            cmd
        };
        let mut creator = command()
            .env("NAMESPACE_TEST_MODE", "create")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(creator.stdout.take().unwrap());
        let namespace = loop {
            let mut line = String::new();
            assert_ne!(output.read_line(&mut line).unwrap(), 0);
            if let Some(tid) = line.split("NAMESPACE_READY ").nth(1) {
                break File::open(format!("/proc/{}/ns/mnt", tid.trim())).unwrap();
            }
        };
        creator.stdin.take().unwrap().write_all(b"x").unwrap();
        assert!(creator.wait().unwrap().success());
        let initial = namespace.metadata().unwrap().ino();
        for _ in 0..2 {
            let scratch = root.join("scratch");
            std::fs::write(&scratch, b"fresh").unwrap();
            let mut child = command()
                .env("NAMESPACE_TEST_MODE", "join")
                .stdin(Stdio::from(namespace.try_clone().unwrap()))
                .spawn()
                .unwrap();
            assert!(child.wait().unwrap().success());
            std::fs::remove_file(scratch).unwrap();
            assert_eq!(namespace.metadata().unwrap().ino(), initial);
        }
        let host = File::open("/proc/self/ns/mnt").unwrap();
        assert!(
            command()
                .env("NAMESPACE_TEST_MODE", "reject")
                .stdin(Stdio::from(host))
                .status()
                .unwrap()
                .success()
        );
        drop(namespace);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[ignore = "child entry for retained namespace test"]
    fn namespace_child() {
        let root = std::env::var("NAMESPACE_TEST_ROOT").unwrap();
        let source = std::env::var("NAMESPACE_TEST_SOURCE").unwrap();
        let mode = std::env::var("NAMESPACE_TEST_MODE").unwrap();
        if mode == "create" {
            let parser = utils::arg_parser::ArgParser::new()
                .arg(utils::arg_parser::Argument::new("bind-ro").allow_multiple(true));
            let mut arguments = parser.arguments().clone();
            arguments
                .parse(&[
                    "jailer".to_owned(),
                    "--bind-ro".to_owned(),
                    format!("restore={source}"),
                ])
                .unwrap();
            let binds = crate::bind_mount::BindMount::from_arguments(&arguments).unwrap();
            let _namespace = crate::chroot::chroot(Path::new(&root), &binds).unwrap();
            // SAFETY: gettid takes no arguments.
            println!("NAMESPACE_READY {}", unsafe {
                libc::syscall(libc::SYS_gettid)
            });
            std::io::stdin().read_exact(&mut [0]).unwrap();
            return;
        }
        // The test harness uses threads; production jailer is single-threaded.
        // SAFETY: unshare separates this test thread's filesystem context before setns.
        assert_eq!(unsafe { libc::unshare(libc::CLONE_FS) }, 0);
        let fd = std::io::stdin()
            .as_fd()
            .try_clone_to_owned()
            .unwrap()
            .into_raw_fd();
        let joined = join(fd, Path::new(&root));
        if mode == "reject" {
            assert!(joined.is_err());
            return;
        }
        let _namespace = joined.unwrap();
        assert_eq!(std::env::current_dir().unwrap(), Path::new("/"));
        assert_eq!(std::fs::read("/restore/mem").unwrap(), b"immutable");
        assert_eq!(std::fs::read("/scratch").unwrap(), b"fresh");
        assert_eq!(
            std::fs::write("/restore/denied", b"x")
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EROFS)
        );
        assert!(!Path::new(&source).exists());
    }
}
