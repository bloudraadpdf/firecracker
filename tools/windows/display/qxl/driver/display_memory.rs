// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

use core::ffi::c_void;
use core::mem::{MaybeUninit, size_of};

const WORD: usize = size_of::<usize>();

unsafe fn copy(dst: *mut u8, src: *const u8, length: usize, backward: bool) {
    let mut remaining = length;
    while remaining != 0 {
        let offset = if backward {
            remaining - 1
        } else {
            length - remaining
        };
        let word = remaining >= WORD
            && if backward {
                (dst.addr() + remaining).is_multiple_of(WORD)
                    && (src.addr() + remaining).is_multiple_of(WORD)
            } else {
                (dst.addr() + offset).is_multiple_of(WORD)
                    && (src.addr() + offset).is_multiple_of(WORD)
            };
        let bytes = if word { WORD } else { 1 };
        let offset = if backward { remaining - bytes } else { offset };
        // SAFETY: callers retain both complete ranges. Direction preserves overlap;
        // word accesses are aligned. MaybeUninit also permits structure padding.
        // Volatile operations prevent lowering the implementation to itself.
        unsafe {
            if word {
                let value = src.add(offset).cast::<MaybeUninit<usize>>().read_volatile();
                dst.add(offset)
                    .cast::<MaybeUninit<usize>>()
                    .write_volatile(value);
            } else {
                let value = src.add(offset).cast::<MaybeUninit<u8>>().read_volatile();
                dst.add(offset)
                    .cast::<MaybeUninit<u8>>()
                    .write_volatile(value);
            }
        }
        remaining -= bytes;
    }
}

/// # Safety
/// Both extents must be valid for length bytes; they must not overlap.
#[cfg_attr(not(test), unsafe(no_mangle))]
pub unsafe extern "C" fn memcpy(
    dst: *mut c_void,
    src: *const c_void,
    length: usize,
) -> *mut c_void {
    // SAFETY: the caller retains the complete disjoint ranges.
    unsafe { copy(dst.cast(), src.cast(), length, false) };
    dst
}

/// # Safety
/// Both extents must be valid for length bytes; overlapping ranges are allowed.
#[cfg_attr(not(test), unsafe(no_mangle))]
pub unsafe extern "C" fn memmove(
    dst: *mut c_void,
    src: *const c_void,
    length: usize,
) -> *mut c_void {
    // SAFETY: the caller retains both ranges; direction preserves overlapping input.
    unsafe { copy(dst.cast(), src.cast(), length, dst.addr() > src.addr()) };
    dst
}

/// # Safety
/// The destination must be writable for length bytes.
#[cfg_attr(not(test), unsafe(no_mangle))]
pub unsafe extern "C" fn memset(dst: *mut c_void, value: i32, length: usize) -> *mut c_void {
    let byte = value as u8;
    let word = usize::from(byte) * (usize::MAX / 255);
    let dst = dst.cast::<u8>();
    let mut offset = 0;
    while offset < length {
        if length - offset >= WORD && (dst.addr() + offset).is_multiple_of(WORD) {
            // SAFETY: this aligned word is inside the caller's writable range.
            unsafe { dst.add(offset).cast::<usize>().write_volatile(word) };
            offset += WORD;
        } else {
            // SAFETY: this byte is inside the caller's writable range.
            unsafe { dst.add(offset).write_volatile(byte) };
            offset += 1;
        }
    }
    dst.cast()
}

/// # Safety
/// Both extents must be readable as initialised bytes for length bytes.
#[cfg_attr(not(test), unsafe(no_mangle))]
pub unsafe extern "C" fn memcmp(left: *const c_void, right: *const c_void, length: usize) -> i32 {
    for index in 0..length {
        // SAFETY: the caller supplies both complete readable extents.
        let (a, b) = unsafe {
            (
                left.cast::<u8>().add(index).read_volatile(),
                right.cast::<u8>().add(index).read_volatile(),
            )
        };
        if a != b {
            return i32::from(a) - i32::from(b);
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_abi_preserves_unaligned_overlap_and_surrounding_bytes() {
        let input = core::array::from_fn::<_, 128, _>(|i| i as u8);
        for source in 0..16 {
            for destination in 0..16 {
                for length in 0..=65 {
                    let mut expected = input;
                    expected.copy_within(source..source + length, destination);
                    let mut actual = input;
                    let base = actual.as_mut_ptr();
                    // SAFETY: both possibly overlapping ranges fit the owned array.
                    let returned = unsafe {
                        memmove(
                            base.add(destination).cast(),
                            base.add(source).cast(),
                            length,
                        )
                    };
                    assert_eq!(returned, base.wrapping_add(destination).cast());
                    assert_eq!(actual, expected);

                    let mut actual = [0xa5; 128];
                    // SAFETY: the two arrays are disjoint and both ranges fit.
                    unsafe {
                        memcpy(
                            actual.as_mut_ptr().add(destination).cast(),
                            input.as_ptr().add(source).cast(),
                            length,
                        );
                    }
                    assert_eq!(
                        &actual[destination..destination + length],
                        &input[source..source + length]
                    );
                    assert!(
                        actual[..destination]
                            .iter()
                            .chain(&actual[destination + length..])
                            .all(|v| *v == 0xa5)
                    );
                }
            }
        }
    }

    #[test]
    fn fill_and_comparison_keep_byte_values_and_first_difference() {
        for start in 0..16 {
            for length in 0..=65 {
                for value in [0, 0x7f, 0x100, -1] {
                    let mut actual = [0xa5; 96];
                    let mut expected = actual;
                    expected[start..start + length].fill(value as u8);
                    // SAFETY: the writable range fits the owned array.
                    unsafe { memset(actual.as_mut_ptr().add(start).cast(), value, length) };
                    assert_eq!(actual, expected);
                }
            }
        }
        // SAFETY: both complete arrays are initialised and retained for each call.
        unsafe {
            assert_eq!(
                memcmp([0u8, 255].as_ptr().cast(), [1u8, 0].as_ptr().cast(), 2),
                -1
            );
            assert_eq!(
                memcmp([255u8].as_ptr().cast(), [0u8].as_ptr().cast(), 1),
                255
            );
            assert_eq!(
                memcmp([1u8, 2].as_ptr().cast(), [1u8, 2].as_ptr().cast(), 2),
                0
            );
        }
    }
}
