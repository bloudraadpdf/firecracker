// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::ffi::CString;
use std::fs;
use std::path::{Component, Path};
use std::ptr::null;

use utils::arg_parser::Arguments;
use vmm_sys_util::syscall::SyscallReturnCode;

use crate::{JailerError, to_cstring};

#[derive(Debug)]
pub struct BindMount {
    source: CString,
    destination: String,
    read_only: bool,
}

impl BindMount {
    pub fn from_arguments(arguments: &Arguments) -> Result<Vec<Self>, JailerError> {
        let mut mounts: Vec<Self> = Vec::new();
        for (argument, read_only) in [("bind-ro", true), ("bind-rw", false)] {
            for value in arguments.multiple_values(argument).unwrap_or_default() {
                let mount = Self::parse(value, read_only)?;
                if mounts
                    .iter()
                    .any(|existing| existing.destination == mount.destination)
                {
                    return Err(JailerError::BindMountArgument(value.clone()));
                }
                mounts.push(mount);
            }
        }
        Ok(mounts)
    }

    fn parse(value: &str, read_only: bool) -> Result<Self, JailerError> {
        let invalid = || JailerError::BindMountArgument(value.to_owned());
        let (destination, source) = value.split_once('=').ok_or_else(invalid)?;
        if !matches!(
            Path::new(destination).components().next(),
            Some(Component::Normal(_))
        ) || destination.contains('/')
            || !Path::new(source).is_absolute()
        {
            return Err(invalid());
        }
        Ok(Self {
            source: to_cstring(Path::new(source))?,
            destination: destination.to_owned(),
            read_only,
        })
    }

    pub fn mount(&self, root: &Path) -> Result<(), JailerError> {
        let target = root.join(&self.destination);
        let metadata = fs::symlink_metadata(&target)
            .map_err(|error| JailerError::BindMount(target.clone(), error))?;
        if !metadata.is_dir() {
            return Err(JailerError::NotADirectory(target));
        }
        let target_c = to_cstring(&target)?;
        let mount = |source: *const libc::c_char, flags| {
            // SAFETY: Both paths are live C strings; this runs in the jailer's private namespace.
            SyscallReturnCode(unsafe {
                libc::mount(source, target_c.as_ptr(), null(), flags, null())
            })
            .into_empty_result()
            .map_err(|error| JailerError::BindMount(target.clone(), error))
        };
        mount(self.source.as_ptr(), libc::MS_BIND)?;
        mount(
            null(),
            libc::MS_BIND
                | libc::MS_REMOUNT
                | libc::MS_NOSUID
                | libc::MS_NODEV
                | libc::MS_NOEXEC
                | if self.read_only { libc::MS_RDONLY } else { 0 },
        )
    }
}

#[cfg(test)]
#[path = "bind_mount_tests.rs"]
mod tests;
