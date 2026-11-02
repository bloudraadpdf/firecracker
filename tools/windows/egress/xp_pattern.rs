// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

pub fn storage_pattern(seed: u32, offset: usize) -> u8 {
    if offset < 4 {
        seed.to_le_bytes()[offset]
    } else {
        ((offset as u32).wrapping_mul(37) ^ seed.rotate_right((offset % 32) as u32)) as u8
    }
}
