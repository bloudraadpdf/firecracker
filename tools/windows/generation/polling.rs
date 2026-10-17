// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::counter::VmGenCounter;

fn accept(
    first: VmGenCounter,
    second: VmGenCounter,
    iteration: u8,
    previous: &[u8],
) -> Result<[u8; 32], &'static [u8]> {
    context(iteration, previous)?;
    if first != second {
        return Err(b"generation changed within the running stage");
    }
    let value = first.hex();
    if value == [b'0'; 32] || value.eq_ignore_ascii_case(previous) {
        return Err(b"generation is zero or unchanged after restore");
    }
    Ok(value)
}

fn context(iteration: u8, previous: &[u8]) -> Result<(), &'static [u8]> {
    if !(1..=3).contains(&iteration)
        || (iteration == 1 && !previous.is_empty())
        || (iteration > 1
            && (previous.len() != 32
                || !previous.iter().all(u8::is_ascii_hexdigit)
                || previous == [b'0'; 32]))
    {
        return Err(b"invalid generation stage context");
    }
    Ok(())
}

fn enabled_value(flag: &[u8], hash: &[u8]) -> Result<bool, &'static [u8]> {
    match flag {
        b"" | b"0" if hash.is_empty() => Ok(false),
        b"" | b"0" => Err(b"reader SHA256 requires generation checks"),
        b"1" if hash.len() == 64 && hash.iter().all(u8::is_ascii_hexdigit) => Ok(true),
        b"1" => Err(b"generation checks require the staged reader SHA256"),
        _ => Err(b"WINDOWS_GENERATION_CHECKS must be 0 or 1"),
    }
}

#[cfg(not(test))]
pub(crate) mod windows {
    use core::ffi::CStr;

    use super::super::counter;
    use super::*;

    pub(crate) enum Failure {
        Message(&'static [u8]),
        Query(counter::QueryError),
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetEnvironmentVariableA(name: *const u8, buffer: *mut u8, size: u32) -> u32;
    }

    pub(crate) fn variable(name: &CStr, buffer: &mut [u8]) -> Result<usize, Failure> {
        // SAFETY: name is terminated; buffer is writable for its declared capacity.
        let length = unsafe {
            GetEnvironmentVariableA(
                name.as_ptr().cast(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
            )
        } as usize;
        if length >= buffer.len() || !buffer[..length].is_ascii() {
            return Err(Failure::Message(b"invalid generation environment value"));
        }
        Ok(length)
    }

    pub(crate) fn enabled() -> Result<bool, Failure> {
        let mut flag = [0; 2];
        let flag_length = variable(c"WINDOWS_GENERATION_CHECKS", &mut flag)?;
        let mut hash = [0; 65];
        let hash_length = variable(c"WINDOWS_GENERATION_PROBE_SHA256", &mut hash)?;
        enabled_value(&flag[..flag_length], &hash[..hash_length]).map_err(Failure::Message)
    }

    pub(crate) fn read(
        iteration: u8,
        previous: &[u8],
        output: fn(&[u8]),
    ) -> Result<[u8; 32], Failure> {
        context(iteration, previous).map_err(Failure::Message)?;
        let first = counter::query().map_err(Failure::Query)?;
        record(first, iteration, b'1', output);
        let second = counter::query().map_err(Failure::Query)?;
        record(second, iteration, b'2', output);
        let value = accept(first, second, iteration, previous).map_err(Failure::Message)?;
        output(b"VMGENID_STAGE iteration=");
        output(&[b'0' + iteration]);
        output(b" value=");
        output(&value);
        output(b"\r\nVMGENID=");
        output(&value);
        output(b"\r\n");
        Ok(value)
    }

    fn record(counter: VmGenCounter, iteration: u8, read: u8, output: fn(&[u8])) {
        output(b"VMGENID_READ iteration=");
        output(&[b'0' + iteration]);
        output(b" read=");
        output(&[read]);
        output(b" value=");
        output(&counter.hex());
        output(b"\r\n");
    }

    pub(crate) fn report(error: Failure, output: fn(&[u8]), number: fn(u32)) {
        output(b"FAIL VMGENID ");
        match error {
            Failure::Message(message) => output(message),
            Failure::Query(counter::QueryError::Win32(stage, code)) => {
                output(stage);
                output(b" win32=");
                number(code);
            }
            Failure::Query(counter::QueryError::OutputSize(size)) => {
                output(b"output_bytes=");
                number(size);
            }
        }
        output(b"\r\n");
    }
}

#[cfg(not(test))]
pub(crate) use windows::{Failure, enabled, read, report};

#[cfg(test)]
mod tests {
    use super::*;

    fn counter(value: u64) -> VmGenCounter {
        VmGenCounter {
            generation_count: value,
            generation_count_high: value,
        }
    }

    #[test]
    fn reader_hash_is_required_only_with_explicit_checks() {
        assert_eq!(enabled_value(b"", b""), Ok(false));
        assert_eq!(enabled_value(b"0", b""), Ok(false));
        assert_eq!(enabled_value(b"1", &[b'a'; 64]), Ok(true));
        for (flag, hash) in [
            (b"1".as_slice(), b"".as_slice()),
            (b"", b"unexpected"),
            (b"1", &[b'x'; 64]),
            (b"true", &[b'a'; 64]),
        ] {
            assert!(enabled_value(flag, hash).is_err());
        }
    }

    #[test]
    fn equal_running_reads_and_changed_restores() {
        let first = accept(counter(1), counter(1), 1, b"").unwrap();
        let second = accept(counter(2), counter(2), 2, &first).unwrap();
        assert!(accept(counter(3), counter(3), 3, &second).is_ok());
    }

    #[test]
    fn rejects_zero_unstable_stale_and_invalid_context() {
        for (first, second, stage, previous) in [
            (counter(0), counter(0), 1, b"".as_slice()),
            (counter(1), counter(2), 1, b"".as_slice()),
            (
                counter(1),
                counter(1),
                2,
                b"00000000000000010000000000000001".as_slice(),
            ),
            (counter(1), counter(1), 1, b"unexpected".as_slice()),
            (counter(1), counter(1), 2, b"".as_slice()),
            (counter(1), counter(1), 0, b"".as_slice()),
            (
                counter(1),
                counter(1),
                4,
                b"00000000000000020000000000000002".as_slice(),
            ),
        ] {
            assert!(accept(first, second, stage, previous).is_err());
        }
    }
}
