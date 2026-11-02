// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::env;
use std::ffi::CStr;
use std::fs::File;
use std::path::Path;
use std::ptr::null;

use vmm_sys_util::syscall::SyscallReturnCode;

use super::{JailerError, to_cstring};
use crate::bind_mount::BindMount;

const ROOT_DIR: &CStr = c"/";
const CURRENT_DIR: &CStr = c".";

// This uses switching to a new mount namespace + pivot_root(), together with the regular chroot,
// to provide a hardened jail (at least compared to only relying on chroot).
pub fn chroot(path: &Path, binds: &[BindMount]) -> Result<File, JailerError> {
    // We unshare into a new mount namespace.
    // SAFETY: The call is safe because we're invoking a C library
    // function with valid parameters.
    SyscallReturnCode(unsafe { libc::unshare(libc::CLONE_NEWNS) })
        .into_empty_result()
        .map_err(JailerError::UnshareNewNs)?;

    let namespace = File::open("/proc/self/ns/mnt").map_err(JailerError::Launch)?;

    // Bind mounts must neither propagate to nor receive mounts from the host.
    // SAFETY: Safe because we provide valid parameters.
    SyscallReturnCode(unsafe {
        libc::mount(
            null(),
            ROOT_DIR.as_ptr(),
            null(),
            (if binds.is_empty() {
                libc::MS_SLAVE
            } else {
                libc::MS_PRIVATE
            }) | libc::MS_REC,
            null(),
        )
    })
    .into_empty_result()
    .map_err(JailerError::MountPropagation)?;

    for bind in binds {
        bind.mount(path)?;
    }

    // We need a CString for the following mount call.
    let chroot_dir = to_cstring(path)?;

    // Bind mount the jail root directory over itself, so we can go around a restriction
    // imposed by pivot_root, which states that the new root and the old root should not
    // be on the same filesystem.
    // SAFETY: Safe because we provide valid parameters.
    SyscallReturnCode(unsafe {
        libc::mount(
            chroot_dir.as_ptr(),
            chroot_dir.as_ptr(),
            null(),
            libc::MS_BIND | libc::MS_REC,
            null(),
        )
    })
    .into_empty_result()
    .map_err(JailerError::MountBind)?;

    // Change current dir to the chroot dir, so we only need to handle relative paths from now on.
    env::set_current_dir(path).map_err(JailerError::SetCurrentDir)?;

    // We are now ready to call pivot_root. We have to use sys_call because there is no libc
    // wrapper for pivot_root.
    // SAFETY: Safe because we provide valid parameters.
    SyscallReturnCode(unsafe {
        libc::syscall(
            libc::SYS_pivot_root,
            CURRENT_DIR.as_ptr(),
            CURRENT_DIR.as_ptr(),
        )
    })
    .into_empty_result()
    .map_err(JailerError::PivotRoot)?;

    // Umount the old_root, thus isolating the process from everything outside the jail root folder.
    // SAFETY: Safe because we provide valid parameters.
    SyscallReturnCode(unsafe { libc::umount2(CURRENT_DIR.as_ptr(), libc::MNT_DETACH) })
        .into_empty_result()
        .map_err(JailerError::UmountOldRoot)?;

    Ok(namespace)
}
