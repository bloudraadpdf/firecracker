// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

type Result<T> = core::result::Result<T, &'static str>;
const MAX_RESPONSE: usize = 1024;

struct Cursor<'a>(&'a [u8]);
impl<'a> Cursor<'a> {
    fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        let (value, rest) = self
            .0
            .split_at_checked(size)
            .ok_or("truncated TPM response")?;
        self.0 = rest;
        Ok(value)
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn sized(&mut self) -> Result<&'a [u8]> {
        let size = usize::from(self.u16()?);
        self.take(size)
    }
    fn end(self) -> Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err("trailing TPM response bytes")
        }
    }
}

fn response(bytes: &[u8]) -> Result<Cursor<'_>> {
    if !(10..=MAX_RESPONSE).contains(&bytes.len()) {
        return Err("invalid TPM response length");
    }
    let mut cursor = Cursor(bytes);
    if cursor.u16()? != 0x8001 || cursor.u32()? as usize != bytes.len() || cursor.u32()? != 0 {
        return Err("invalid TPM response header or command failure");
    }
    Ok(cursor)
}

fn random(bytes: &[u8]) -> Result<()> {
    let mut cursor = response(bytes)?;
    if !(1..=16).contains(&cursor.sized()?.len()) {
        return Err("invalid GetRandom result length");
    }
    cursor.end()
}

fn pcr7(bytes: &[u8]) -> Result<[u8; 32]> {
    let mut cursor = response(bytes)?;
    let _update_counter = cursor.u32()?;
    if cursor.u32()? != 1
        || cursor.u16()? != 0x000b
        || cursor.take(4)? != [3, 0x80, 0, 0]
        || cursor.u32()? != 1
    {
        return Err("PCR_Read did not return only SHA256 PCR7");
    }
    let digest = cursor
        .sized()?
        .try_into()
        .map_err(|_| "invalid PCR7 digest length")?;
    cursor.end()?;
    Ok(digest)
}

fn first_handle(bytes: &[u8]) -> Result<u32> {
    let mut cursor = response(bytes)?;
    if cursor.take(1)?[0] > 1 || cursor.u32()? != 1 || cursor.u32()? != 1 {
        return Err("no single persistent handle returned");
    }
    let handle = cursor.u32()?;
    if handle >> 24 != 0x81 {
        return Err("handle is not persistent");
    }
    cursor.end()?;
    Ok(handle)
}

fn public(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    let mut cursor = response(bytes)?;
    let public = cursor.sized()?;
    let name = cursor.sized()?;
    let qualified = cursor.sized()?;
    cursor.end()?;
    let mut fields = Cursor(public);
    if !matches!(fields.u16()?, 1 | 0x23) {
        return Err("persistent object is not an RSA or ECC key");
    }
    let algorithm = fields.u16()?;
    let digest_size = match algorithm {
        4 => 20,
        0x0b | 0x12 => 32,
        0x0c => 48,
        0x0d => 64,
        _ => return Err("unsupported persistent key Name algorithm"),
    };
    fields.u32()?;
    let policy = fields.sized()?;
    if !policy.is_empty() && policy.len() != digest_size {
        return Err("invalid public auth policy length");
    }
    if fields.0.is_empty()
        || [name, qualified]
            .iter()
            .any(|value| value.len() != digest_size + 2 || value[..2] != algorithm.to_be_bytes())
    {
        return Err("invalid public key or Name structure");
    }
    Ok((public, name))
}

#[cfg(not(test))]
mod windows {
    use super::*;
    use core::{
        ffi::c_void,
        fmt::{self, Write},
    };

    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn WriteFile(
            handle: *mut c_void,
            data: *const u8,
            size: u32,
            written: *mut u32,
            overlap: *mut c_void,
        ) -> i32;
        fn ExitProcess(code: u32) -> !;
    }
    #[link(name = "tbs", kind = "raw-dylib")]
    unsafe extern "system" {
        fn Tbsi_Context_Create(params: *const u32, context: *mut *mut c_void) -> u32;
        fn Tbsip_Submit_Command(
            context: *mut c_void,
            locality: u32,
            priority: u32,
            command: *const u8,
            command_size: u32,
            response: *mut u8,
            response_size: *mut u32,
        ) -> u32;
        fn Tbsip_Context_Close(context: *mut c_void) -> u32;
    }
    struct Console;
    impl Write for Console {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let mut count = 0;
            let result = unsafe {
                WriteFile(
                    GetStdHandle(-11i32 as u32),
                    s.as_ptr(),
                    s.len() as u32,
                    &mut count,
                    core::ptr::null_mut(),
                )
            };
            if result == 0 || count as usize != s.len() {
                Err(fmt::Error)
            } else {
                Ok(())
            }
        }
    }
    struct Hex<'a>(&'a [u8]);
    impl fmt::Display for Hex<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            for byte in self.0 {
                write!(f, "{byte:02x}")?;
            }
            Ok(())
        }
    }
    fn submit<'a>(
        context: *mut c_void,
        code: u32,
        body: &[u8],
        buffer: &'a mut [u8; MAX_RESPONSE],
    ) -> Result<&'a [u8]> {
        let mut command = [0u8; 64];
        let size = 10 + body.len();
        if size > command.len() {
            return Err("command exceeds probe bound");
        }
        command[..2].copy_from_slice(&0x8001u16.to_be_bytes());
        command[2..6].copy_from_slice(&(size as u32).to_be_bytes());
        command[6..10].copy_from_slice(&code.to_be_bytes());
        command[10..size].copy_from_slice(body);
        let mut count = buffer.len() as u32;
        let status = unsafe {
            Tbsip_Submit_Command(
                context,
                0,
                200,
                command.as_ptr(),
                size as u32,
                buffer.as_mut_ptr(),
                &mut count,
            )
        };
        writeln!(
            Console,
            "TBS command={code:#010x} status={status:#010x} bytes={count}"
        )
        .map_err(|_| "console write failed")?;
        if status != 0 {
            return Err("TBS command blocked or failed");
        }
        buffer
            .get(..count as usize)
            .ok_or("TBS response exceeds buffer")
    }
    fn read_state(
        context: *mut c_void,
        buffer: &mut [u8; MAX_RESPONSE],
    ) -> Result<([u8; 32], u32, &[u8], &[u8])> {
        random(submit(context, 0x17b, &[0, 16], buffer)?)?;
        let digest = pcr7(submit(
            context,
            0x17e,
            &[0, 0, 0, 1, 0, 0x0b, 3, 0x80, 0, 0],
            buffer,
        )?)?;
        let handle = first_handle(submit(
            context,
            0x17a,
            &[0, 0, 0, 1, 0x81, 0, 0, 0, 0, 0, 0, 1],
            buffer,
        )?)?;
        let bytes = submit(context, 0x173, &handle.to_be_bytes(), buffer)?;
        let (key, name) = public(bytes)?;
        Ok((digest, handle, key, name))
    }
    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
        unsafe { ExitProcess(99) }
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn mainCRTStartup() -> ! {
        let mut context = core::ptr::null_mut();
        let status = unsafe { Tbsi_Context_Create([2u32, 4].as_ptr(), &mut context) };
        let _ = writeln!(Console, "TBS context status={status:#010x}");
        if status != 0 {
            unsafe { ExitProcess(1) }
        }
        let mut buffer = [0u8; MAX_RESPONSE];
        let state = read_state(context, &mut buffer);
        let close = unsafe { Tbsip_Context_Close(context) };
        let _ = writeln!(Console, "TBS close status={close:#010x}");
        if close != 0 {
            unsafe { ExitProcess(2) }
        }
        match state {
            Ok((digest, handle, key, name)) => {
                if writeln!(Console, "TPM_STATE PCR7_SHA256={}\nTPM_STATE HANDLE={handle:08x}\nTPM_STATE PUBLIC={}\nTPM_STATE NAME={}\nPASS Windows TBS TPM2_GetRandom\nPASS Windows TBS TPM2_PCR_Read TPM2_ReadPublic", Hex(&digest), Hex(key), Hex(name)).is_err() {
                    unsafe { ExitProcess(4) }
                }
                unsafe { ExitProcess(0) }
            }
            Err(error) => {
                let _ = writeln!(Console, "FAIL {error}");
                unsafe { ExitProcess(3) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reply(body: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0x80, 1];
        bytes.extend(((10 + body.len()) as u32).to_be_bytes());
        bytes.extend([0; 4]);
        bytes.extend(body);
        bytes
    }
    fn pcr_reply() -> Vec<u8> {
        let mut body = vec![
            0, 0, 0, 42, 0, 0, 0, 1, 0, 0x0b, 3, 0x80, 0, 0, 0, 0, 0, 1, 0, 32,
        ];
        body.extend([0xa5; 32]);
        reply(&body)
    }
    fn key_reply() -> Vec<u8> {
        let mut body = vec![
            1, 22, 0, 1, 0, 0x0b, 0, 0, 0, 0, 0, 0, 0, 0x10, 0, 0x10, 8, 0, 0, 0, 0, 0, 1, 0,
        ];
        body.extend([0xa5; 256]);
        for _ in 0..2 {
            body.extend([0, 34, 0, 0x0b]);
            body.extend([0x5a; 32]);
        }
        reply(&body)
    }
    #[test]
    fn headers_reject_tpm_errors_wrong_tags_sizes_and_oversized_frames() {
        let good = reply(&[]);
        for (offset, value) in [(0, 0), (1, 2), (5, 11), (9, 1)] {
            let mut bad = good.clone();
            bad[offset] = value;
            assert!(response(&bad).is_err());
        }
        assert!(response(&good[..9]).is_err());
        assert!(response(&vec![0; MAX_RESPONSE + 1]).is_err());
        response(&good).unwrap().end().unwrap();
    }
    #[test]
    fn pcr7_checks_selection_and_digest_not_update_counter() {
        let good = pcr_reply();
        assert_eq!(pcr7(&good).unwrap(), [0xa5; 32]);
        let mut counter = good.clone();
        counter[10..14].fill(255);
        assert_eq!(pcr7(&counter).unwrap(), pcr7(&good).unwrap());
        for offset in [17, 19, 20, 21, 22, 27, 29] {
            let mut bad = good.clone();
            bad[offset] ^= 1;
            assert!(pcr7(&bad).is_err(), "offset {offset}");
        }
        for length in 0..good.len() {
            assert!(pcr7(&good[..length]).is_err());
        }
        let mut body = good[10..].to_vec();
        body.push(0);
        assert!(pcr7(&reply(&body)).is_err());
    }
    #[test]
    fn capability_requires_one_persistent_handle_and_accepts_more_data() {
        let body = [0, 0, 0, 0, 1, 0, 0, 0, 1, 0x81, 0, 0, 1];
        assert_eq!(first_handle(&reply(&body)).unwrap(), 0x81000001);
        let mut more = body;
        more[0] = 1;
        assert_eq!(first_handle(&reply(&more)).unwrap(), 0x81000001);
        for (offset, value) in [(0, 2), (4, 2), (8, 0), (8, 2), (9, 0x80)] {
            let mut bad = body;
            bad[offset] = value;
            assert!(first_handle(&reply(&bad)).is_err());
        }
        assert!(first_handle(&reply(&body[..12])).is_err());
    }
    #[test]
    fn public_lengths_names_and_object_type_are_checked() {
        let good = key_reply();
        let (key, name) = public(&good).unwrap();
        assert_eq!(key.len(), 278);
        assert_eq!(name.len(), 34);
        for offset in [11, 13, 15, 20, 291, 293, 327, 329] {
            let mut bad = good.clone();
            bad[offset] ^= 1;
            assert!(public(&bad).is_err(), "offset {offset}");
        }
        for length in 0..good.len() {
            assert!(public(&good[..length]).is_err());
        }
        let mut body = good[10..].to_vec();
        body.push(0);
        assert!(public(&reply(&body)).is_err());
    }
    #[test]
    fn random_rejects_empty_oversized_and_trailing_data() {
        random(&reply(&[0, 1, 42])).unwrap();
        for body in [vec![0, 0], vec![0, 2, 42], vec![0, 1, 42, 43], vec![0; 19]] {
            assert!(random(&reply(&body)).is_err());
        }
    }
}
