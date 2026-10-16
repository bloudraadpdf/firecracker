// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#![no_std]
#![no_main]

use core::{ffi::c_void, fmt::Write, ptr};

type Result<T> = core::result::Result<T, (&'static str, u32)>;
const ATTRIBUTES: u32 = 0x27;
const NAME: [u16; 64] = wide("FirecrackerRuntimeTest");
const GUID: [u16; 64] = wide("{7bc4c67c-16f4-4df6-a24e-a3caa7904f09}");
const PRIVILEGE: [u16; 64] = wide("SeSystemEnvironmentPrivilege");
const GLOBAL_GUID: [u16; 64] = wide("{8be4df61-93ca-11d2-aa0d-00e098032b8c}");
const DERIVED: [(&str, [u16; 64], u8); 2] = [
    ("SetupMode", wide("SetupMode"), 0),
    ("SecureBoot", wide("SecureBoot"), 1),
];
const VALUES: [&[u8]; 3] = [
    include_bytes!("fixtures/create.payload"),
    include_bytes!("fixtures/replace.payload"),
    include_bytes!("fixtures/replace-again.payload"),
];
const UPDATES: [&[u8]; 3] = [
    include_bytes!("fixtures/create.auth"),
    include_bytes!("fixtures/replace.auth"),
    include_bytes!("fixtures/replace-again.auth"),
];

const fn wide(text: &str) -> [u16; 64] {
    assert!(text.is_ascii() && text.len() < 64);
    let mut result = [0; 64];
    let mut index = 0;
    while index < text.len() {
        result[index] = text.as_bytes()[index] as u16;
        index += 1;
    }
    result
}

#[repr(C)]
struct Luid {
    low: u32,
    high: i32,
}
#[repr(C)]
struct Privileges {
    count: u32,
    luid: Luid,
    attributes: u32,
}
#[derive(Clone, Copy)]
enum Phase {
    Cold,
    Full,
    Differential,
    ColdReboot,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFirmwareEnvironmentVariableExW(
        name: *const u16,
        guid: *const u16,
        buffer: *mut u8,
        size: u32,
        attributes: *mut u32,
    ) -> u32;
    fn SetFirmwareEnvironmentVariableExW(
        name: *const u16,
        guid: *const u16,
        buffer: *const u8,
        size: u32,
        attributes: u32,
    ) -> i32;
    fn GetLastError() -> u32;
    fn SetLastError(error: u32);
    fn GetCurrentProcess() -> *mut c_void;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn GetCommandLineW() -> *const u16;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
    fn GetStdHandle(which: u32) -> *mut c_void;
    fn WriteFile(
        file: *mut c_void,
        bytes: *const u8,
        size: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn ExitProcess(code: u32) -> !;
}
#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
    fn LookupPrivilegeValueW(system: *const u16, name: *const u16, luid: *mut Luid) -> i32;
    fn AdjustTokenPrivileges(
        token: *mut c_void,
        disable: i32,
        state: *const Privileges,
        size: u32,
        previous: *mut c_void,
        returned: *mut u32,
    ) -> i32;
}
#[link(name = "shell32")]
unsafe extern "system" {
    fn CommandLineToArgvW(command: *const u16, count: *mut i32) -> *mut *mut u16;
}

struct Console;
impl Write for Console {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let mut written = 0;
        // SAFETY: the UTF8 byte slice and output count remain live for the synchronous write.
        let result = unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                text.as_ptr(),
                text.len() as u32,
                &mut written,
                ptr::null_mut(),
            )
        };
        if result != 0 && written as usize == text.len() {
            Ok(())
        } else {
            Err(core::fmt::Error)
        }
    }
}

fn last_error() -> u32 {
    // SAFETY: GetLastError has no pointer arguments.
    unsafe { GetLastError() }
}

fn check(result: i32, operation: &'static str) -> Result<()> {
    if result != 0 {
        Ok(())
    } else {
        Err((operation, last_error()))
    }
}

fn enable_privilege() -> Result<()> {
    let mut token = ptr::null_mut();
    check(
        // SAFETY: the output token is writable; GetCurrentProcess returns the process pseudo-handle.
        unsafe { OpenProcessToken(GetCurrentProcess(), 0x28, &mut token) },
        "OpenProcessToken",
    )?;
    let result = (|| {
        let mut state = Privileges {
            count: 1,
            luid: Luid { low: 0, high: 0 },
            attributes: 2,
        };
        check(
            // SAFETY: the LUID output is complete/aligned; the constant name is NUL-terminated.
            unsafe { LookupPrivilegeValueW(ptr::null(), PRIVILEGE.as_ptr(), &mut state.luid) },
            "LookupPrivilegeValueW",
        )?;
        // SAFETY: token is open and state is one complete TOKEN_PRIVILEGES record; no previous state is requested.
        unsafe {
            SetLastError(0);
            check(
                AdjustTokenPrivileges(token, 0, &state, 0, ptr::null_mut(), ptr::null_mut()),
                "AdjustTokenPrivileges",
            )?;
            let error = GetLastError();
            if error != 0 {
                return Err(("privilege not assigned", error));
            }
        }
        Ok(())
    })();
    // SAFETY: token was opened successfully and no operation still uses it.
    check(unsafe { CloseHandle(token) }, "CloseHandle")?;
    result
}

fn phase() -> Result<Phase> {
    let mut count = 0;
    // SAFETY: Windows provides a NUL-terminated process command line and writable argc output.
    let args = unsafe { CommandLineToArgvW(GetCommandLineW(), &mut count) };
    if args.is_null() {
        return Err(("CommandLineToArgvW", last_error()));
    }
    let result = if count == 2 {
        // SAFETY: argc is two; argument strings are NUL-terminated. The loop reads no character after NUL.
        unsafe {
            let arg = *args.add(1);
            let mut length = 0;
            while length < 5 && *arg.add(length) != 0 {
                length += 1;
            }
            match core::slice::from_raw_parts(arg, length) {
                [49] => Ok(Phase::Cold),
                [50] => Ok(Phase::Full),
                [51] => Ok(Phase::Differential),
                [99, 111, 108, 100] => Ok(Phase::ColdReboot),
                _ => Err(("usage: uefi-runtime.exe 1|2|3|cold", 0)),
            }
        }
    } else {
        Err(("usage: uefi-runtime.exe 1|2|3|cold", 0))
    };
    // SAFETY: the argument array is the allocation returned by CommandLineToArgvW.
    if !unsafe { LocalFree(args.cast()) }.is_null() {
        return Err(("LocalFree", last_error()));
    }
    result
}

fn expect(expected: Option<&[u8]>) -> Result<()> {
    let mut bytes = [0; 64];
    let mut attributes = 0;
    // SAFETY: names are constant/NUL-terminated, and both output buffers remain writable for the call.
    let count = unsafe {
        GetFirmwareEnvironmentVariableExW(
            NAME.as_ptr(),
            GUID.as_ptr(),
            bytes.as_mut_ptr(),
            64,
            &mut attributes,
        )
    };
    if count == 0 {
        let error = last_error();
        return if expected.is_none() && error == 203 {
            Ok(())
        } else {
            Err(("GetFirmwareEnvironmentVariableExW", error))
        };
    }
    if expected.is_some() && expected == bytes.get(..count as usize) && attributes == ATTRIBUTES {
        Ok(())
    } else {
        Err(("unexpected runtime variable bytes or attributes", 0))
    }
}

fn update(label: &str, payload: &[u8], accepted: bool) -> Result<()> {
    let size = u32::try_from(payload.len()).map_err(|_| ("oversized authentication fixture", 0))?;
    // SAFETY: the constant names and complete public authentication buffer remain live for this synchronous call.
    let result = unsafe {
        SetFirmwareEnvironmentVariableExW(
            NAME.as_ptr(),
            GUID.as_ptr(),
            payload.as_ptr(),
            size,
            ATTRIBUTES,
        )
    };
    if result != 0 {
        if !accepted {
            return Err(("invalid authenticated update was accepted", 0));
        }
    } else {
        let error = last_error();
        writeln!(Console, "UEFI_RUNTIME rejected={label} win32_error={error}")
            .map_err(|_| ("stdout", 0))?;
        if accepted || error == 0 {
            return Err(("SetFirmwareEnvironmentVariableExW", error));
        }
    }
    Ok(())
}

fn expect_derived() -> Result<()> {
    for (_, name, expected) in &DERIVED {
        let mut value = 0;
        let mut attributes = 0;
        // SAFETY: the fixed names are NUL-terminated and both outputs remain writable for the call.
        let size = unsafe {
            GetFirmwareEnvironmentVariableExW(
                name.as_ptr(),
                GLOBAL_GUID.as_ptr(),
                &mut value,
                1,
                &mut attributes,
            )
        };
        if size == 0 {
            return Err(("read derived variable", last_error()));
        }
        if size != 1 || value != *expected || attributes != 6 {
            return Err(("unexpected derived variable bytes or attributes", 0));
        }
    }
    Ok(())
}

fn reject_derived_writes() -> Result<()> {
    expect_derived()?;
    for (label, name, value) in &DERIVED {
        // SAFETY: only the two fixed derived variables receive their unchanged one-byte values.
        let result = unsafe {
            SetFirmwareEnvironmentVariableExW(name.as_ptr(), GLOBAL_GUID.as_ptr(), value, 1, 6)
        };
        let error = if result == 0 { last_error() } else { 0 };
        if result != 0 || error == 0 {
            return Err(("derived variable write was not rejected", error));
        }
        writeln!(Console, "UEFI_RUNTIME rejected={label} win32_error={error}")
            .map_err(|_| ("stdout", 0))?;
        expect_derived()?;
    }
    Ok(())
}

fn run(phase: Phase) -> Result<()> {
    reject_derived_writes()?;
    if matches!(phase, Phase::ColdReboot) {
        expect(Some(VALUES[2]))?;
        update("delete", include_bytes!("fixtures/delete.auth"), true)?;
        expect(None)?;
    } else {
        let phase = phase as usize;
        expect(phase.checked_sub(1).map(|index| VALUES[index]))?;
        update("owner", UPDATES[phase], true)?;
        expect(Some(VALUES[phase]))?;
        for (label, auth) in [
            (
                "tampered",
                include_bytes!("fixtures/tampered.auth").as_slice(),
            ),
            (
                "wrong-owner",
                include_bytes!("fixtures/wrong-owner.auth").as_slice(),
            ),
            ("replay", UPDATES[0]),
        ] {
            update(label, auth, false)?;
            expect(Some(VALUES[phase]))?;
        }
    }
    expect_derived()?;
    writeln!(
        Console,
        "PASS UEFI_RUNTIME phase={}",
        match phase {
            Phase::Cold => "1",
            Phase::Full => "2",
            Phase::Differential => "3",
            Phase::ColdReboot => "cold",
        }
    )
    .map_err(|_| ("stdout", 0))
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // SAFETY: ExitProcess has no pointer arguments and does not return.
    unsafe { ExitProcess(99) }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let result = phase().and_then(|phase| enable_privilege().and_then(|()| run(phase)));
    let code = match result {
        Ok(()) => 0,
        Err((operation, error)) => {
            let _ = writeln!(Console, "FAIL UEFI_RUNTIME {operation} win32_error={error}");
            1
        }
    };
    // SAFETY: ExitProcess has no pointer arguments and does not return.
    unsafe { ExitProcess(code) }
}
