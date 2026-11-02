// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::protocol::HEADER_SIZE;

pub(super) fn request(kind: u32, words: &[u32]) -> Vec<u8> {
    let mut bytes = vec![0; HEADER_SIZE];
    bytes[..4].copy_from_slice(&kind.to_le_bytes());
    bytes.extend(words.iter().flat_map(|word| word.to_le_bytes()));
    bytes
}
