// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

use super::Error;

pub(super) struct Writer<'a> {
    bytes: &'a mut [u8],
    written: usize,
    word: u32,
    used: u32,
}

impl<'a> Writer<'a> {
    pub(super) fn new(bytes: &'a mut [u8]) -> Self {
        Self {
            bytes,
            written: 0,
            word: 0,
            used: 0,
        }
    }

    pub(super) fn put(&mut self, value: u32, mut bits: u32) -> Result<(), Error> {
        while bits != 0 {
            let count = bits.min(32 - self.used);
            let part = value >> (bits - count);
            self.word |= if count == 32 {
                part
            } else {
                (part & ((1 << count) - 1)) << (32 - self.used - count)
            };
            self.used += count;
            bits -= count;
            if self.used == 32 {
                let target = self
                    .bytes
                    .get_mut(self.written..self.written + 4)
                    .ok_or(Error::BufferTooSmall)?;
                target.copy_from_slice(&self.word.to_le_bytes());
                self.written += 4;
                self.word = 0;
                self.used = 0;
            }
        }
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<usize, Error> {
        if self.used != 0 {
            self.put(0, 32 - self.used)?;
        }
        self.put(0, 32)?;
        Ok(self.written)
    }
}

pub(super) struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < 24 || !bytes.len().is_multiple_of(4) {
            return Err(Error::InvalidStream);
        }
        Ok(Self { bytes, position: 0 })
    }

    pub(super) fn get(&mut self, mut bits: u32) -> Result<u32, Error> {
        let mut value = 0u32;
        while bits != 0 {
            let offset = self.position / 32 * 4;
            let word = self
                .bytes
                .get(offset..offset + 4)
                .ok_or(Error::InvalidStream)?;
            let word = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
            let used = self.position as u32 % 32;
            let count = bits.min(32 - used);
            let part = word << used >> (32 - count);
            value = if count == 32 {
                part
            } else {
                (value << count) | part
            };
            self.position += count as usize;
            bits -= count;
        }
        Ok(value)
    }
}
