// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use crate::invalid;
use des::cipher::{BlockEncrypt, KeyInit};
use std::io;

#[derive(Clone)]
pub(crate) struct Password([u8; 8]);

impl Password {
    pub fn new(value: &str) -> io::Result<Self> {
        if value.is_empty() || value.len() > 8 || !value.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(invalid(
                "VNC password must contain 1..8 printable ASCII characters",
            ));
        }
        let mut key = [0; 8];
        for (out, byte) in key.iter_mut().zip(value.bytes()) {
            *out = byte.reverse_bits();
        }
        Ok(Self(key))
    }

    pub(crate) fn response(&self, mut challenge: [u8; 16]) -> [u8; 16] {
        let cipher = des::Des::new(&self.0.into());
        for block in challenge.as_chunks_mut::<8>().0 {
            cipher.encrypt_block(block.into());
        }
        challenge
    }
}
