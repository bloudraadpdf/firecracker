// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! XDDM cache hashes from murmur_hash2a.h, based on Austin Appleby's MurmurHash2A.

const MULTIPLIER: u32 = 0x5bd1e995;

fn mix(hash: u32, word: u32) -> u32 {
    let word = word.wrapping_mul(MULTIPLIER);
    let word = (word ^ (word >> 24)).wrapping_mul(MULTIPLIER);
    hash.wrapping_mul(MULTIPLIER) ^ word
}

fn finish(hash: u32, length: u32) -> u32 {
    let hash = mix(hash, length);
    let hash = (hash ^ (hash >> 13)).wrapping_mul(MULTIPLIER);
    hash ^ (hash >> 15)
}

pub fn murmur2a(data: &[u8], seed: u32) -> Option<u32> {
    let length = u32::try_from(data.len()).ok()?;
    let (words, tail) = data.as_chunks::<4>();
    let mut hash = seed;
    for word in words {
        hash = mix(hash, u32::from_le_bytes(*word));
    }
    let mut tail_word = 0;
    for (index, &byte) in tail.iter().enumerate() {
        tail_word |= u32::from(byte) << (index * 8);
    }
    Some(finish(mix(hash, tail_word), length))
}

/// Preserve the XDDM Jump3 packing, which excludes each pixel's high byte.
pub fn murmur2a_jump3(pixels: &[u32], seed: u32) -> Option<u32> {
    let length = u32::try_from(pixels.len()).ok()?.checked_mul(4)?;
    let (groups, tail) = pixels.as_chunks::<4>();
    let mut hash = seed;
    for &[first, second, third, fourth] in groups {
        hash = mix(hash, (first << 8) | (second & 0xff));
        hash = mix(hash, ((second << 8) & 0xffff0000) | ((third >> 8) & 0xffff));
        hash = mix(hash, (fourth << 8) | (third & 0xff));
    }
    for &pixel in tail {
        hash = mix(hash, pixel << 8);
    }
    Some(finish(hash.wrapping_mul(MULTIPLIER), length))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_smhasher_murmur2a_verification() {
        // Austin Appleby's SMHasher verification value for MurmurHash2A.
        let mut key = [0; 256];
        let mut hashes = [0; 1024];
        for index in 0u16..256 {
            let offset = usize::from(index);
            key[offset] = u8::try_from(index).unwrap();
            hashes[offset * 4..offset * 4 + 4].copy_from_slice(
                &murmur2a(&key[..offset], u32::from(256 - index))
                    .unwrap()
                    .to_le_bytes(),
            );
        }
        assert_eq!(murmur2a(&hashes, 0), Some(0x7fbd4396));
    }

    #[test]
    fn jump3_ignores_alpha_for_full_groups_and_every_tail_length() {
        let pixels = [
            0x00112233, 0x00445566, 0x00778899, 0x00aabbcc, 0x00ddeeff, 0x00332211, 0x00665544,
        ];
        let opaque = pixels.map(|pixel| pixel | 0xff000000);
        for count in 0..=pixels.len() {
            for seed in [0, 1, u32::MAX] {
                assert_eq!(
                    murmur2a_jump3(&pixels[..count], seed),
                    murmur2a_jump3(&opaque[..count], seed)
                );
            }
        }
        let mut different = pixels;
        different[3] ^= 0x00010000;
        assert_ne!(murmur2a_jump3(&pixels, 0), murmur2a_jump3(&different, 0));
    }

    #[test]
    fn unaligned_bytes_have_the_same_hash() {
        #[repr(C, align(4))]
        struct Aligned([u8; 8]);
        let input = Aligned([0xff, 0, 1, 2, 3, 4, 5, 6]);
        let bytes = &input.0[1..];
        assert_eq!(bytes.as_ptr().addr() % 4, 1);
        assert_eq!(
            murmur2a(bytes, 17),
            murmur2a(&[0, 1, 2, 3, 4, 5, 6], 17)
        );
    }
}
