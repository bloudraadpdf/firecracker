// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use super::BindMount;
use crate::chroot::chroot;

#[test]
fn rejects_invalid_bind_paths_and_duplicate_destinations() {
    for value in [
        "",
        "restore",
        "=/source",
        "../restore=/source",
        "..=/source",
        ".=/source",
        "/restore=/source",
        "restore/=/source",
        "restore=relative",
        "restore=",
    ] {
        assert!(BindMount::parse(value, true).is_err(), "{value}");
    }
    let mount = BindMount::parse("restore=/source with spaces=a:b", true).unwrap();
    assert_eq!(mount.source.to_bytes(), b"/source with spaces=a:b");
    let parser = crate::build_arg_parser();
    let mut arguments = parser.arguments().clone();
    arguments
        .parse(
            &[
                "jailer",
                "--id",
                "test",
                "--exec-file",
                "/bin/true",
                "--uid",
                "0",
                "--gid",
                "0",
                "--bind-ro",
                "restore=/source",
                "--bind-rw",
                "restore=/other",
            ]
            .map(str::to_owned),
        )
        .unwrap();
    BindMount::from_arguments(&arguments).unwrap_err();
}

#[test]
#[ignore = "requires CAP_SYS_ADMIN; runs isolated child processes"]
fn private_bind_mounts_preserve_restrictions_and_host_unlink() {
    let root = std::env::current_dir()
        .unwrap()
        .join(format!(".bind-test-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let source = root.join("source=a:b");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("snapshot"), b"snapshot").unwrap();
    let mut children = Vec::new();
    for index in 0..2 {
        let jail = root.join(index.to_string());
        fs::create_dir(&jail).unwrap();
        for name in ["restore", "warm"] {
            fs::create_dir(jail.join(name)).unwrap();
        }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "bind_mount::tests::bound_child",
                "--ignored",
                "--nocapture",
            ])
            .env("BIND_TEST_JAIL", &jail)
            .env("BIND_TEST_SOURCE", &source)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            assert!(
                output.read_line(&mut line).unwrap() > 0,
                "child exited before mounting: {line}"
            );
            if line.contains("MOUNTS_READY") {
                break;
            }
            line.clear();
        }
        let tid: u32 = line
            .split_once("MOUNTS_READY ")
            .unwrap()
            .1
            .trim()
            .parse()
            .unwrap();
        let namespace = fs::read_link(format!("/proc/{tid}/ns/mnt")).unwrap();
        assert_ne!(namespace, fs::read_link("/proc/self/ns/mnt").unwrap());
        assert_eq!(fs::read_dir(jail.join("restore")).unwrap().count(), 0);
        children.push((child, namespace, output));
    }
    assert_ne!(children[0].1, children[1].1);
    fs::remove_file(source.join("snapshot")).unwrap();
    for (mut child, _, mut output) in children {
        child.stdin.take().unwrap().write_all(b"x").unwrap();
        let mut report = String::new();
        output.read_to_string(&mut report).unwrap();
        assert!(child.wait().unwrap().success(), "{report}");
    }
    assert_eq!(fs::read(source.join("captured")).unwrap(), b"captured");
    let target = root.join("0/restore");
    fs::remove_dir(&target).unwrap();
    std::os::unix::fs::symlink(&source, &target).unwrap();
    let bind = BindMount::parse(&format!("restore={}", source.display()), true).unwrap();
    assert!(matches!(
        bind.mount(&root.join("0")),
        Err(crate::JailerError::NotADirectory(_))
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "child entry for private_bind_mounts_preserve_restrictions_and_host_unlink"]
fn bound_child() {
    let jail = std::env::var_os("BIND_TEST_JAIL").unwrap();
    let source = std::env::var("BIND_TEST_SOURCE").unwrap();
    let binds = [
        BindMount::parse(&format!("restore={source}"), true).unwrap(),
        BindMount::parse(&format!("warm={source}"), false).unwrap(),
    ];
    chroot(Path::new(&jail), &binds).unwrap();
    assert_eq!(fs::read("/restore/snapshot").unwrap(), b"snapshot");
    assert_eq!(
        fs::write("/restore/denied", b"x")
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EROFS)
    );
    fs::write("/warm/captured", b"captured").unwrap();
    for path in [c"/restore", c"/warm"] {
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        assert_eq!(
            // SAFETY: path is a valid C string and stat is writable for the syscall.
            unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) },
            0
        );
        // SAFETY: statvfs succeeded and initialised stat.
        let flags = unsafe { stat.assume_init() }.f_flag;
        let restricted = libc::ST_NOSUID | libc::ST_NODEV | libc::ST_NOEXEC;
        assert_eq!(flags & restricted, restricted);
    }
    assert!(!Path::new("/old_root").exists());
    // SAFETY: gettid takes no parameters and only reads the calling thread's identity.
    let tid = unsafe { libc::gettid() };
    println!("MOUNTS_READY {tid}");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_exact(&mut [0]).unwrap();
    assert!(!Path::new("/restore/snapshot").exists());
}
