// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::os::unix::net::UnixStream;
use std::thread::JoinHandle;
use std::time::Duration;

fn session(version: &[u8; 12]) -> (UnixStream, JoinHandle<io::Result<()>>) {
    let (mut client, mut server) = UnixStream::pair().unwrap();
    for stream in [&client, &server] {
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
    }
    let task = std::thread::spawn(move || {
        handshake(&mut server, Some(&Password::new("password").unwrap()))?;
        assert_eq!(read::<1>(&mut server)?, [1]);
        Ok(())
    });
    assert_eq!(read::<12>(&mut client).unwrap(), *b"RFB 003.008\n");
    client.write_all(version).unwrap();
    if version == b"RFB 003.003\n" {
        assert_eq!(read::<4>(&mut client).unwrap(), [0, 0, 0, 2]);
    } else {
        assert_eq!(read::<2>(&mut client).unwrap(), [1, 2]);
    }
    (client, task)
}

#[test]
fn vnc_des_matches_independent_openssl_vector() {
    // OpenSSL DES-ECB, key 0e86ceceeef64e26, no padding; VNC reverses key-byte bits.
    let challenge = std::array::from_fn(|index| u8::try_from(index).unwrap());
    assert_eq!(
        Password::new("password").unwrap().response(challenge),
        [
            0xb8, 0x66, 0x92, 0x41, 0x25, 0xc8, 0xee, 0xbb, 0x9d, 0xeb, 0xc1, 0xdb, 0x61, 0xc5,
            0x38, 0xe2,
        ]
    );
}

#[test]
fn authenticated_versions_preserve_client_init() {
    for version in [b"RFB 003.003\n", b"RFB 003.007\n", b"RFB 003.008\n"] {
        let (mut client, task) = session(version);
        if version != b"RFB 003.003\n" {
            client.write_all(&[2]).unwrap();
        }
        let challenge = read::<16>(&mut client).unwrap();
        let response = Password::new("password").unwrap().response(challenge);
        client.write_all(&response).unwrap();
        client.write_all(&[1]).unwrap();
        assert_eq!(read::<4>(&mut client).unwrap(), [0; 4]);
        task.join().unwrap().unwrap();
    }
}

#[test]
fn failed_authentication_uses_version_specific_result() {
    for version in [b"RFB 003.003\n", b"RFB 003.007\n", b"RFB 003.008\n"] {
        let (mut client, task) = session(version);
        if version != b"RFB 003.003\n" {
            client.write_all(&[2]).unwrap();
        }
        let challenge = read::<16>(&mut client).unwrap();
        let mut response = Password::new("password").unwrap().response(challenge);
        response[0] ^= 1;
        client.write_all(&response).unwrap();
        assert_eq!(read::<4>(&mut client).unwrap(), [0, 0, 0, 1]);
        if version == b"RFB 003.008\n" {
            assert_eq!(u32::from_be_bytes(read::<4>(&mut client).unwrap()), 21);
            assert_eq!(read::<21>(&mut client).unwrap(), *b"Authentication failed");
        }
        task.join().unwrap().unwrap_err();
        assert_eq!(client.read(&mut [0]).unwrap(), 0);
    }
}

#[test]
fn password_sessions_reject_none_security_selection() {
    for version in [b"RFB 003.007\n", b"RFB 003.008\n"] {
        let (mut client, task) = session(version);
        client.write_all(&[1]).unwrap();
        task.join().unwrap().unwrap_err();
        assert_eq!(client.read(&mut [0]).unwrap(), 0);
    }
}

#[test]
fn password_validation_does_not_silently_truncate() {
    for value in ["", "123456789", "abc\n", "ab\0cd", "caf\u{e9}"] {
        Password::new(value).err().unwrap();
    }
    Password::new("a").unwrap();
    Password::new("12345678").unwrap();
}

#[test]
fn failed_authentication_does_not_release_guest_input() {
    use crate::{Backend, SessionGuard, authenticate_console};
    use std::os::unix::net::UnixListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let path = std::path::PathBuf::from(format!("rfb-auth-test-{}.sock", std::process::id()));
    let backend = UnixListener::bind(&path).unwrap();
    backend.set_nonblocking(true).unwrap();
    let (mut client, mut server) = UnixStream::pair().unwrap();
    for stream in [&client, &server] {
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
    }
    let busy = Arc::new(AtomicBool::new(true));
    let mut guard = SessionGuard {
        input_active: false,
        backend: Backend(path.clone()),
        busy: busy.clone(),
    };
    let task = std::thread::spawn(move || {
        authenticate_console(
            &mut server,
            &mut guard,
            Some(&Password::new("password").unwrap()),
        )
        .unwrap_err();
        assert!(!guard.input_active);
    });
    read::<12>(&mut client).unwrap();
    client.write_all(b"RFB 003.008\n").unwrap();
    assert_eq!(read::<2>(&mut client).unwrap(), [1, 2]);
    client.write_all(&[2]).unwrap();
    let challenge = read::<16>(&mut client).unwrap();
    let mut response = Password::new("password").unwrap().response(challenge);
    response[0] ^= 1;
    client.write_all(&response).unwrap();
    task.join().unwrap();
    assert!(!busy.load(Ordering::Acquire));
    assert_eq!(
        backend.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    std::fs::remove_file(path).unwrap();
}
