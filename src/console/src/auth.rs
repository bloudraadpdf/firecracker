// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io::{self, Read};
use std::path::Path;

use des::cipher::{BlockEncrypt, KeyInit};

use crate::invalid;

#[derive(Clone)]
pub(crate) struct Password([u8; 8]);

impl Password {
    pub fn from_file(path: &Path) -> io::Result<Self> {
        if !std::fs::metadata(path)?.is_file() {
            return Err(invalid("password file must be a regular file"));
        }
        let file = std::fs::File::open(path)?;
        let mut value = String::new();
        file.take(65).read_to_string(&mut value)?;
        if value.len() > 64 {
            return Err(invalid("password file exceeds 64 bytes"));
        }
        Self::new(value.trim_end())
    }
    pub fn new(value: &str) -> io::Result<Self> {
        if value.is_empty() || value.len() > 8 || !value.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(invalid(
                "VNC password must contain 1..8 printable ASCII characters",
            ));
        }
        let mut key = [0; 8];
        for (out, byte) in key.iter_mut().zip(value.bytes()) {
            *out = byte.reverse_bits();
        }
        Ok(Self(key))
    }

    pub(crate) fn response(&self, mut challenge: [u8; 16]) -> [u8; 16] {
        let cipher = des::Des::new(&self.0.into());
        for block in challenge.as_chunks_mut::<8>().0 {
            cipher.encrypt_block(block.into());
        }
        challenge
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_files_allow_trailing_newlines_and_enforce_the_size_limit() {
        let path = std::env::current_dir()
            .unwrap()
            .join(format!("console-password-{}.txt", std::process::id()));
        for (contents, valid) in [
            ("password\r\n".to_owned(), true),
            (format!("password{}", "\n".repeat(56)), true),
            (format!("password{}", "\n".repeat(57)), false),
            (String::new(), false),
            ("123456789".to_owned(), false),
        ] {
            std::fs::write(&path, contents).unwrap();
            let result = Password::from_file(&path);
            std::fs::remove_file(&path).unwrap();
            assert_eq!(result.is_ok(), valid);
            if let Ok(password) = result {
                assert_eq!(
                    password.response([0; 16]),
                    Password::new("password").unwrap().response([0; 16])
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn fifo_password_is_rejected_before_opening() {
        use std::os::unix::ffi::OsStrExt;
        use std::time::{Duration, Instant};
        const CHILD: &str = "FIRECRACKER_PASSWORD_FIFO_TEST";
        if let Some(path) = std::env::var_os(CHILD) {
            assert_eq!(
                Password::from_file(Path::new(&path)).err().unwrap().kind(),
                io::ErrorKind::InvalidData
            );
            return;
        }
        let path = std::env::current_dir()
            .unwrap()
            .join(format!("console-password-{}.fifo", std::process::id()));
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a valid NUL-terminated path in this test's working directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "auth::tests::fifo_password_is_rejected_before_opening",
            ])
            .env(CHILD, &path)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        let _ = child.kill();
        child.wait().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(status.expect("password FIFO blocked startup").success());
    }
}
