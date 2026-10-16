// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Validation for filesystem Unix socket paths.

use std::io;
use std::os::unix::ffi::OsStrExt;
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
