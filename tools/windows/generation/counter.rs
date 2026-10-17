// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

// CTL_CODE(FILE_DEVICE_ACPI, 1, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS).
pub(crate) const IOCTL_VMGENCOUNTER_READ: u32 = (0x32 << 16) | (3 << 14) | (1 << 2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct VmGenCounter {
    pub(crate) generation_count: u64,
    pub(crate) generation_count_high: u64,
}

const _: () = assert!(size_of::<VmGenCounter>() == 16);
const _: () = assert!(core::mem::offset_of!(VmGenCounter, generation_count_high) == 8);

impl VmGenCounter {
    pub(crate) fn hex(self) -> [u8; 32] {
        let mut bytes = [b'0'; 32];
        for (value, field) in [self.generation_count_high, self.generation_count]
            .into_iter()
            .zip(bytes.as_chunks_mut::<16>().0)
        {
            let mut value = value;
            for byte in field.iter_mut().rev() {
                *byte = b"0123456789abcdef"[(value & 15) as usize];
                value >>= 4;
            }
        }
        bytes
    }
}

#[cfg(not(test))]
pub(crate) mod windows {
    use core::ptr;

    use super::*;
    use crate::device::{CloseHandle, CreateFileW, DeviceIoControl};

    #[derive(Debug)]
    pub(crate) enum QueryError {
        Win32(&'static [u8], u32),
        OutputSize(u32),
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLastError() -> u32;
    }

    pub(crate) struct Handle(pub(crate) usize);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: this scope owns the valid handle and all I/O has finished.
            unsafe { CloseHandle(self.0) };
        }
    }

    pub(crate) fn open(attributes: u32) -> Result<Handle, QueryError> {
        let name = (*b"\\\\.\\VmGenerationCounter\0").map(u16::from);
        // SAFETY: name is terminated UTF-16 and remains live for the synchronous open.
        let raw =
            unsafe { CreateFileW(name.as_ptr(), 0xc0000000, 3, ptr::null(), 3, attributes, 0) };
        if raw == usize::MAX {
            // SAFETY: GetLastError has no arguments; capture before any cleanup can alter it.
            return Err(QueryError::Win32(b"CreateFileW", unsafe { GetLastError() }));
        }
        Ok(Handle(raw))
    }

    pub(crate) fn query() -> Result<VmGenCounter, QueryError> {
        let handle = open(0)?;
        let mut counter = VmGenCounter {
            generation_count: 0,
            generation_count_high: 0,
        };
        let mut returned = 0;
        // SAFETY: handle is live; both output buffers are writable for their declared sizes.
        // No input selects polling mode; this synchronous call has no OVERLAPPED object.
        let ok = unsafe {
            DeviceIoControl(
                handle.0,
                IOCTL_VMGENCOUNTER_READ,
                ptr::null(),
                0,
                (&raw mut counter).cast(),
                16,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            // SAFETY: GetLastError has no arguments; capture before Handle is dropped.
            return Err(QueryError::Win32(b"DeviceIoControl", unsafe {
                GetLastError()
            }));
        }
        if returned != 16 {
            return Err(QueryError::OutputSize(returned));
        }
        Ok(counter)
    }
}

#[cfg(not(test))]
pub(crate) use windows::{QueryError, query};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_and_wire_order() {
        assert_eq!(IOCTL_VMGENCOUNTER_READ, 0x0032c004);
        assert_eq!(size_of::<VmGenCounter>(), 16);
        assert_eq!(
            core::mem::offset_of!(VmGenCounter, generation_count_high),
            8
        );
        assert_eq!(
            VmGenCounter {
                generation_count: 0x0123456789abcdef,
                generation_count_high: 0xfedcba9876543210
            }
            .hex(),
            *b"fedcba98765432100123456789abcdef"
        );
    }
}
