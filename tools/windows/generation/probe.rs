// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(not(test))]
#[path = "../egress/win32.rs"]
mod win32;

// CTL_CODE(FILE_DEVICE_ACPI, 1, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS).
const IOCTL_VMGENCOUNTER_READ: u32 = (0x32 << 16) | (3 << 14) | (1 << 2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
struct VmGenCounter {
    generation_count: u64,
    generation_count_high: u64,
}

const _: () = assert!(size_of::<VmGenCounter>() == 16);
const _: () = assert!(core::mem::offset_of!(VmGenCounter, generation_count_high) == 8);

#[derive(Debug, PartialEq, Eq)]
enum Request {
    Poll,
    Wait(VmGenCounter, u32),
}

fn parse(args: &[&[u16]]) -> Option<Request> {
    if args.is_empty() {
        return Some(Request::Poll);
    }
    let [wait, id, timeout, milliseconds] = args else {
        return None;
    };
    if !wait.iter().copied().eq(b"--wait".map(u16::from))
        || !timeout.iter().copied().eq(b"--timeout-ms".map(u16::from))
        || id.len() != 32
        || milliseconds.is_empty()
        || milliseconds.len() > 6
    {
        return None;
    }
    let mut fields = [0u64; 2];
    for (field, digits) in fields.iter_mut().zip(id.as_chunks::<16>().0) {
        for &digit in digits {
            let value = match digit {
                48..=57 => digit - 48,
                65..=70 => digit - 65 + 10,
                97..=102 => digit - 97 + 10,
                _ => return None,
            };
            *field = (*field << 4) | u64::from(value);
        }
    }
    let mut ms = 0u32;
    for &digit in *milliseconds {
        if !(48..=57).contains(&digit) {
            return None;
        }
        ms = ms.checked_mul(10)?.checked_add(u32::from(digit - 48))?;
    }
    if !(1..=600_000).contains(&ms) || fields == [0, 0] {
        return None;
    }
    Some(Request::Wait(
        VmGenCounter {
            generation_count: fields[1],
            generation_count_high: fields[0],
        },
        ms,
    ))
}

#[cfg(not(test))]
mod windows {
    use super::*;
    use crate::win32::{ExitProcess, GetLastError, GetStdHandle, WriteFile};
    use core::ffi::c_void;
    use core::ptr;

    const ERROR_IO_PENDING: u32 = 997;
    const ERROR_IO_INCOMPLETE: u32 = 996;
    const WAIT_TIMEOUT: u32 = 258;

    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: usize,
    }

    const _: () = assert!(size_of::<Overlapped>() == if size_of::<usize>() == 4 { 20 } else { 32 });
    const _: () = assert!(core::mem::offset_of!(Overlapped, event) == 2 * size_of::<usize>() + 8);

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const c_void,
            disposition: u32,
            attributes: u32,
            template: usize,
        ) -> usize;
        fn DeviceIoControl(
            handle: usize,
            code: u32,
            input: *const c_void,
            input_len: u32,
            output: *mut c_void,
            output_len: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn CloseHandle(handle: usize) -> i32;
        fn CreateEventW(
            security: *const c_void,
            manual: i32,
            initial: i32,
            name: *const u16,
        ) -> usize;
        fn WaitForSingleObject(handle: usize, milliseconds: u32) -> u32;
        fn GetOverlappedResult(
            handle: usize,
            overlapped: *mut Overlapped,
            returned: *mut u32,
            wait: i32,
        ) -> i32;
        fn CancelIo(handle: usize) -> i32;
        fn GetCommandLineW() -> *const u16;
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn CommandLineToArgvW(command: *const u16, count: *mut i32) -> *mut *mut u16;
    }

    struct Handle(usize);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: this scope owns the valid handle and all I/O has finished.
            unsafe { CloseHandle(self.0) };
        }
    }

    enum Failure {
        Win32(&'static [u8], u32),
        OutputSize(u32),
        Message(&'static [u8]),
    }

    fn request() -> Result<Request, Failure> {
        let mut count = 0;
        // SAFETY: Windows supplies a terminated command line; count is writable.
        let args = unsafe { CommandLineToArgvW(GetCommandLineW(), &mut count) };
        if args.is_null() {
            // SAFETY: capture the error before any other Win32 call.
            return Err(Failure::Win32(b"CommandLineToArgvW", unsafe {
                GetLastError()
            }));
        }
        let result = match count {
            1 => Some(Request::Poll),
            5 => {
                let mut tokens: [&[u16]; 4] = [&[]; 4];
                for (index, token) in tokens.iter_mut().enumerate() {
                    // SAFETY: argc is five; each argument is terminated. Inspect at most
                    // 33 characters, stopping at NUL before reading another character.
                    unsafe {
                        let arg = *args.add(index + 1);
                        let mut length = 0;
                        while length < 33 && *arg.add(length) != 0 {
                            length += 1;
                        }
                        *token = core::slice::from_raw_parts(arg, length);
                    }
                }
                parse(&tokens)
            }
            _ => None,
        };
        // SAFETY: this is the single allocation returned by CommandLineToArgvW.
        if !unsafe { LocalFree(args.cast()) }.is_null() {
            // SAFETY: capture the error before any other Win32 call.
            return Err(Failure::Win32(b"LocalFree", unsafe { GetLastError() }));
        }
        result.ok_or(Failure::Message(
            b"usage: vmgenid.exe [--wait CURRENT32HEX --timeout-ms 1..600000]",
        ))
    }

    fn open(attributes: u32) -> Result<Handle, Failure> {
        let name = (*b"\\\\.\\VmGenerationCounter\0").map(u16::from);
        // SAFETY: name is terminated UTF-16 and remains live for the synchronous open.
        let raw =
            unsafe { CreateFileW(name.as_ptr(), 0xc0000000, 3, ptr::null(), 3, attributes, 0) };
        if raw == usize::MAX {
            // SAFETY: GetLastError has no arguments; capture before any cleanup can alter it.
            return Err(Failure::Win32(b"CreateFileW", unsafe { GetLastError() }));
        }
        Ok(Handle(raw))
    }

    fn query() -> Result<VmGenCounter, Failure> {
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
            return Err(Failure::Win32(b"DeviceIoControl", unsafe {
                GetLastError()
            }));
        }
        if returned != 16 {
            return Err(Failure::OutputSize(returned));
        }
        Ok(counter)
    }

    // Do not return, drop handles or release request storage until completion.
    fn cancel(handle: &Handle, overlapped: &mut Overlapped) {
        // SAFETY: this thread issued the only request on the live overlapped handle.
        let ok = unsafe { CancelIo(handle.0) };
        if ok == 0 {
            // SAFETY: capture the error before waiting or writing output.
            let error = unsafe { GetLastError() };
            output(b"FAIL VMGENID CancelIo win32=0x");
            hex(u64::from(error));
            output(b"\r\n");
        }
        let mut returned = 0;
        // SAFETY: event, handle and request storage remain live through cancellation.
        let finished = unsafe { WaitForSingleObject(overlapped.event, 5_000) } == 0;
        // SAFETY: use the original handle/object, a writable count, and never wait unboundedly.
        let complete = finished
            && unsafe {
                GetOverlappedResult(handle.0, overlapped, &mut returned, 0) != 0
                    || GetLastError() != ERROR_IO_INCOMPLETE
            };
        if !complete {
            output(b"FAIL VMGENID cancellation did not complete within 5000 ms\r\n");
            // SAFETY: terminate without unwinding; pending I/O storage stays live until
            // process teardown. Returning here would release buffers still used by Windows.
            unsafe { ExitProcess(1) }
        }
    }

    fn wait(current: VmGenCounter, milliseconds: u32) -> Result<VmGenCounter, Failure> {
        let handle = open(0x40000000)?; // FILE_FLAG_OVERLAPPED
        // SAFETY: no name/security descriptor; create an initially unsignalled manual-reset event.
        let raw = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if raw == 0 {
            // SAFETY: capture before the device handle is dropped.
            return Err(Failure::Win32(b"CreateEventW", unsafe { GetLastError() }));
        }
        let event = Handle(raw);
        let mut overlapped = Overlapped {
            internal: 0,
            internal_high: 0,
            offset: 0,
            offset_high: 0,
            event: event.0,
        };
        let mut counter = VmGenCounter {
            generation_count: 0,
            generation_count_high: 0,
        };
        let mut returned = 0;
        // SAFETY: distinct input/output and OVERLAPPED storage remain live until completion
        // or a drained cancellation. The event belongs to this one request.
        let ok = unsafe {
            DeviceIoControl(
                handle.0,
                IOCTL_VMGENCOUNTER_READ,
                (&raw const current).cast(),
                16,
                (&raw mut counter).cast(),
                16,
                &mut returned,
                (&raw mut overlapped).cast(),
            )
        };
        if ok != 0 {
            return Err(Failure::Message(
                b"wait completed immediately; not prearmed",
            ));
        }
        // SAFETY: capture the submission status before any other API call.
        let error = unsafe { GetLastError() };
        if error != ERROR_IO_PENDING {
            return Err(Failure::Win32(b"DeviceIoControl", error));
        }
        print_counter(b"ARMED VMGENID=", current);
        // SAFETY: this event remains live for the bounded wait.
        match unsafe { WaitForSingleObject(event.0, milliseconds) } {
            0 => {}
            status => {
                let failure = if status == WAIT_TIMEOUT {
                    Failure::Message(b"notification timed out")
                } else if status == u32::MAX {
                    // SAFETY: capture a failed wait before cancellation can alter the error.
                    Failure::Win32(b"WaitForSingleObject", unsafe { GetLastError() })
                } else {
                    Failure::Message(b"unexpected event wait result")
                };
                cancel(&handle, &mut overlapped);
                return Err(failure);
            }
        }
        // SAFETY: original request/handle and writable byte count; event was signalled.
        if unsafe { GetOverlappedResult(handle.0, &mut overlapped, &mut returned, 0) } == 0 {
            // SAFETY: capture before cleanup. An incomplete request still needs draining.
            let error = unsafe { GetLastError() };
            if error == ERROR_IO_INCOMPLETE {
                cancel(&handle, &mut overlapped);
            }
            return Err(Failure::Win32(b"GetOverlappedResult", error));
        }
        if returned != 16 {
            return Err(Failure::OutputSize(returned));
        }
        if counter == current {
            return Err(Failure::Message(
                b"notification returned unchanged identity",
            ));
        }
        Ok(counter)
    }

    fn print_counter(prefix: &[u8], counter: VmGenCounter) {
        output(prefix);
        hex(counter.generation_count_high);
        hex(counter.generation_count);
        output(b"\r\n");
    }

    fn output(mut bytes: &[u8]) {
        // SAFETY: GetStdHandle takes a standard handle identifier and no pointers.
        let stdout = unsafe { GetStdHandle(-11i32 as u32) };
        while !bytes.is_empty() {
            let mut written = 0;
            // SAFETY: bytes remains readable and written is writable during synchronous I/O.
            let ok = unsafe {
                WriteFile(
                    stdout,
                    bytes.as_ptr(),
                    u32::try_from(bytes.len()).unwrap(),
                    &mut written,
                    ptr::null_mut(),
                )
            };
            if ok == 0 || written == 0 || written as usize > bytes.len() {
                // SAFETY: ExitProcess never returns. Pending request storage remains
                // live until process teardown; no stack unwinding occurs.
                unsafe { ExitProcess(2) }
            }
            bytes = &bytes[written as usize..];
        }
    }

    fn hex(mut value: u64) {
        let mut digits = [b'0'; 16];
        for digit in digits.iter_mut().rev() {
            *digit = b"0123456789abcdef"[(value & 15) as usize];
            value >>= 4;
        }
        output(&digits);
    }

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
        // SAFETY: ExitProcess takes no pointers and never returns.
        unsafe { ExitProcess(99) }
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn mainCRTStartup() -> ! {
        let result = request().and_then(|request| match request {
            Request::Poll => query(),
            Request::Wait(current, milliseconds) => wait(current, milliseconds),
        });
        let code = match result {
            Ok(counter) => {
                print_counter(b"VMGENID=", counter);
                0
            }
            Err(Failure::Win32(stage, error)) => {
                output(b"FAIL VMGENID ");
                output(stage);
                output(b" win32=0x");
                hex(u64::from(error));
                output(b"\r\n");
                if error == 5 {
                    output(b"Run as Administrator or SYSTEM.\r\n");
                }
                1
            }
            Err(Failure::Message(message)) => {
                output(b"FAIL VMGENID ");
                output(message);
                output(b"\r\n");
                1
            }
            Err(Failure::OutputSize(size)) => {
                output(b"FAIL VMGENID output_bytes=0x");
                hex(u64::from(size));
                output(b" expected=0x10\r\n");
                1
            }
        };
        // SAFETY: ExitProcess never returns; submitted I/O has completed or been drained.
        unsafe { ExitProcess(code) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(args: &[&str]) -> Option<Request> {
        let wide: Vec<Vec<u16>> = args
            .iter()
            .map(|arg| arg.encode_utf16().collect())
            .collect();
        parse(&wide.iter().map(Vec::as_slice).collect::<Vec<_>>())
    }

    #[test]
    fn polling_has_no_options() {
        assert_eq!(IOCTL_VMGENCOUNTER_READ, 0x0032c004);
        assert_eq!(args(&[]), Some(Request::Poll));
        assert_eq!(args(&["--wait"]), None);
        assert_eq!(args(&["extra"]), None);
    }

    #[test]
    fn wait_uses_printed_high_then_low_fields() {
        assert_eq!(
            args(&[
                "--wait",
                "0123456789abcDEFfedcba9876543210",
                "--timeout-ms",
                "600000"
            ]),
            Some(Request::Wait(
                VmGenCounter {
                    generation_count: 0xfedcba9876543210,
                    generation_count_high: 0x0123456789abcdef
                },
                600000
            ))
        );
        assert!(matches!(
            args(&[
                "--wait",
                "00000000000000000000000000000001",
                "--timeout-ms",
                "1"
            ]),
            Some(Request::Wait(_, 1))
        ));
    }

    #[test]
    fn rejects_invalid_identity_and_timeout() {
        for id in [
            "",
            "00000000000000000000000000000000",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "g123456789abcdef0123456789abcdef",
        ] {
            assert_eq!(args(&["--wait", id, "--timeout-ms", "1000"]), None);
        }
        for timeout in [
            "",
            "0",
            "600001",
            "4294967296",
            "0000001",
            "-1",
            "+1",
            "1ms",
            " 1",
            "1 ",
            "\u{ff11}",
        ] {
            assert_eq!(
                args(&[
                    "--wait",
                    "0123456789abcdef0123456789abcdef",
                    "--timeout-ms",
                    timeout
                ]),
                None
            );
        }
    }

    #[test]
    fn rejects_wrong_tokens_order_and_extra_arguments() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(args(&["--WAIT", id, "--timeout-ms", "1000"]), None);
        assert_eq!(args(&["--wait", id, "--timeout", "1000"]), None);
        assert_eq!(args(&["--timeout-ms", "1000", "--wait", id]), None);
        assert_eq!(args(&["--wait", id, "--timeout-ms", "1000", "extra"]), None);
    }
}
