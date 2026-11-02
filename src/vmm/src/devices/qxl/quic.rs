// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Bounded SPICE QUIC wire decoder. No guest-driver implementation is linked.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Truncated,
    Invalid,
}

#[derive(Debug)]
struct Bits<'a> {
    input: &'a [u8],
    position: usize,
}

impl Bits<'_> {
    fn take(&mut self, mut count: u32) -> Result<u32, Error> {
        let mut value = 0;
        while count != 0 {
            let base = (self.position / 32) * 4;
            let word = self.input.get(base..base + 4).ok_or(Error::Truncated)?;
            let word = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
            let offset = u32::try_from(self.position % 32).map_err(|_| Error::Invalid)?;
            let part = count.min(32 - offset);
            let mask = u32::MAX >> (32 - part);
            let next = (word >> (32 - offset - part)) & mask;
            value = if part == 32 {
                next
            } else {
                (value << part) | next
            };
            self.position += part as usize;
            count -= part;
        }
        Ok(value)
    }

    fn symbol(&mut self, parameter: u32, depth: u32) -> Result<u8, Error> {
        let escape = (26 - depth).min((1 << (depth - parameter)) - 1);
        let mut quotient = 0;
        while quotient < escape {
            if self.take(1)? != 0 {
                let value = (quotient << parameter) | self.take(parameter)?;
                return u8::try_from(value).map_err(|_| Error::Invalid);
            }
            quotient += 1;
        }
        let base = escape << parameter;
        let remaining = (1u32 << depth) - base;
        let suffix = 32 - (remaining - 1).leading_zeros();
        let value = base + self.take(suffix)?;
        if value >= 1 << depth {
            return Err(Error::Invalid);
        }
        u8::try_from(value).map_err(|_| Error::Invalid)
    }
}

#[derive(Clone, Copy, Debug)]
struct Estimate {
    costs: [u32; 8],
    parameter: u32,
}

impl Estimate {
    fn observe(&mut self, residual: u8, depth: u32, threshold: u32) {
        let mut best = depth - 1;
        for parameter in 0..depth {
            let escape = (26 - depth).min((1 << (depth - parameter)) - 1);
            let base = escape << parameter;
            let value = u32::from(residual);
            let length = if value < base {
                (value >> parameter) + parameter + 1
            } else {
                escape + 32 - (((1u32 << depth) - base) - 1).leading_zeros()
            };
            self.costs[parameter as usize] += length;
        }
        for parameter in (0..depth - 1).rev() {
            if self.costs[parameter as usize] < self.costs[best as usize] {
                best = parameter;
            }
        }
        self.parameter = best;
        if self.costs[best as usize] > threshold {
            for cost in &mut self.costs {
                *cost >>= 1;
            }
        }
    }
}

#[derive(Debug)]
struct Schedule {
    skip: usize,
    window: usize,
    until_window: usize,
    random: usize,
    run_state: usize,
}

impl Schedule {
    fn new() -> Self {
        Self {
            skip: 0,
            window: 0,
            until_window: 2048,
            random: 0,
            run_state: 0,
        }
    }

    fn run(&mut self, bits: &mut Bits<'_>, limit: usize) -> Result<usize, Error> {
        const EXPONENT: [u32; 32] = [
            0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 9, 10, 11,
            12, 13, 14, 15,
        ];
        let mut length = 0;
        while bits.take(1)? != 0 {
            length += 1usize << EXPONENT[self.run_state];
            if length > limit {
                return Err(Error::Invalid);
            }
            self.run_state = (self.run_state + 1).min(EXPONENT.len() - 1);
        }
        length += bits.take(EXPONENT[self.run_state])? as usize;
        self.run_state = self.run_state.saturating_sub(1);
        if length > limit {
            Err(Error::Invalid)
        } else {
            Ok(length)
        }
    }
}

#[derive(Debug)]
struct Decoder<'a> {
    bits: Bits<'a>,
    width: usize,
    depth: u32,
    pixels: Vec<u32>,
    residuals: [Vec<u8>; 4],
    estimates: [[Estimate; 8]; 4],
}

impl Decoder<'_> {
    fn context(&self, channel: usize, x: usize) -> usize {
        let value = u32::from(self.residuals[channel][x]) + 1;
        (31 - value.leading_zeros()).min(self.depth - 1) as usize
    }

    fn component(&self, row: usize, x: usize, channel: usize) -> u32 {
        (self.pixels[row * self.width + x] >> [16, 8, 0, 24][channel]) & 255
    }

    fn put(&mut self, row: usize, x: usize, channel: usize, value: u32) {
        let shift = [16, 8, 0, 24][channel];
        let pixel = &mut self.pixels[row * self.width + x];
        *pixel = (*pixel & !(255 << shift)) | (value << shift);
    }

    fn row(&mut self, y: usize, channels: &[usize], state: &mut Schedule) -> Result<(), Error> {
        const THRESHOLD: [u32; 7] = [110, 550, 900, 800, 550, 400, 350];
        for &channel in channels {
            self.residuals[channel][0] = self.residuals[channel][1];
        }
        let mut x = 0;
        while x < self.width {
            if state.window < 6 && state.until_window == 0 {
                state.window += 1;
                state.until_window = 2048;
            }
            let end = if state.window < 6 {
                self.width.min(x + state.until_window)
            } else {
                self.width
            };
            let start = x;
            let mut last_run = 0;
            while x < end {
                if y != 0
                    && x > 2
                    && x != last_run
                    && channels.iter().all(|&c| {
                        self.component(y - 1, x - 1, c) == self.component(y - 1, x, c)
                            && self.component(y, x - 2, c) == self.component(y, x - 1, c)
                    })
                {
                    last_run = x;
                    let length = state.run(&mut self.bits, end - x)?;
                    for dest in x..x + length {
                        for &channel in channels {
                            self.put(y, dest, channel, self.component(y, x - 1, channel));
                        }
                    }
                    x += length;
                    if x == end {
                        break;
                    }
                    if length != 0 {
                        continue;
                    }
                }
                for &channel in channels {
                    let bucket = self.context(channel, x);
                    let residual = self
                        .bits
                        .symbol(self.estimates[channel][bucket].parameter, self.depth)?;
                    let predicted = match (x, y) {
                        (0, 0) => 0,
                        (0, _) => self.component(y - 1, 0, channel),
                        (_, 0) => self.component(0, x - 1, channel),
                        _ => {
                            (self.component(y, x - 1, channel) + self.component(y - 1, x, channel))
                                / 2
                        }
                    };
                    let difference = if residual & 1 == 0 {
                        u32::from(residual) / 2
                    } else {
                        (1 << self.depth) - 1 - u32::from(residual) / 2
                    };
                    self.put(
                        y,
                        x,
                        channel,
                        (predicted + difference) & ((1 << self.depth) - 1),
                    );
                    self.residuals[channel][x + 1] = residual;
                    if state.skip == 0 {
                        self.estimates[channel][bucket].observe(
                            residual,
                            self.depth,
                            THRESHOLD[state.window],
                        );
                    }
                }
                if state.skip == 0 {
                    state.skip = usize::from(SAMPLE[state.random]) & ((1 << state.window) - 1);
                    state.random = (state.random + 1) & 255;
                } else {
                    state.skip -= 1;
                }
                x += 1;
            }
            if state.window < 6 {
                state.until_window -= end - start;
            }
        }
        Ok(())
    }
}

/// Decode canonical ARGB pixels; unused X bytes are opaque. Geometry and all
/// loops are bounded before allocation or entropy decoding.
pub fn decode(input: &[u8], width: u32, height: u32) -> Result<Vec<u32>, Error> {
    if input.len() < 24 || input.len() > 32 << 20 || !input.len().is_multiple_of(4) {
        return Err(Error::Truncated);
    }
    let word = |offset| {
        let b: &[u8] = &input[offset..offset + 4];
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    };
    let kind = word(8);
    let count = u64::from(width) * u64::from(height);
    if &input[..4] != b"QUIC"
        || word(4) != 0
        || !(1..=5).contains(&kind)
        || word(12) != width
        || word(16) != height
        || width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || count > (32 << 20) / 4
    {
        return Err(Error::Invalid);
    }
    let depth = if kind == 2 { 5 } else { 8 };
    let mut decoder = Decoder {
        bits: Bits {
            input,
            position: 160,
        },
        width: width as usize,
        depth,
        pixels: vec![0; usize::try_from(count).map_err(|_| Error::Invalid)?],
        residuals: core::array::from_fn(|_| vec![0; width as usize + 1]),
        estimates: [[Estimate {
            costs: [0; 8],
            parameter: depth - 1,
        }; 8]; 4],
    };
    let mut color = Schedule::new();
    let mut alpha = Schedule::new();
    for y in 0..height as usize {
        decoder.row(y, if kind == 1 { &[0] } else { &[0, 1, 2] }, &mut color)?;
        if kind == 5 {
            decoder.row(y, &[3], &mut alpha)?;
        }
    }
    for pixel in &mut decoder.pixels {
        if kind == 1 {
            let gray = (*pixel >> 16) & 255;
            *pixel = gray * 0x010101;
        } else if kind == 2 {
            let expand = |shift: u32| {
                let v = (*pixel >> shift) & 31;
                (v << 3) | (v >> 2)
            };
            *pixel = (expand(16) << 16) | (expand(8) << 8) | expand(0);
        }
        if kind != 5 {
            *pixel |= 0xff00_0000;
        }
    }
    Ok(decoder.pixels)
}

// Low six bits of the QUIC wire format's fixed sampling sequence.
const SAMPLE: [u8; 256] = [
    2, 23, 19, 21, 7, 18, 40, 14, 12, 32, 47, 44, 36, 6, 18, 11, 4, 19, 42, 29, 16, 63, 57, 51, 16,
    3, 5, 14, 15, 30, 20, 9, 36, 45, 3, 57, 61, 0, 8, 5, 41, 14, 43, 27, 43, 59, 31, 35, 39, 60,
    42, 38, 13, 54, 62, 1, 26, 3, 11, 61, 52, 34, 7, 48, 4, 59, 22, 12, 22, 8, 50, 4, 10, 6, 44, 8,
    11, 46, 31, 51, 53, 17, 1, 43, 46, 60, 36, 54, 10, 27, 39, 44, 22, 41, 23, 2, 44, 0, 40, 53,
    40, 48, 16, 43, 32, 28, 33, 12, 20, 2, 56, 62, 55, 58, 18, 20, 13, 55, 38, 29, 57, 19, 49, 33,
    47, 63, 59, 0, 35, 61, 19, 25, 46, 38, 60, 4, 35, 1, 26, 40, 28, 10, 5, 14, 25, 18, 48, 49, 20,
    59, 62, 34, 46, 7, 33, 52, 34, 58, 9, 29, 8, 55, 62, 60, 37, 36, 37, 50, 6, 3, 10, 28, 48, 24,
    26, 61, 57, 30, 11, 54, 2, 6, 50, 47, 7, 17, 35, 53, 32, 12, 23, 17, 5, 21, 49, 55, 51, 45, 52,
    27, 39, 25, 39, 63, 22, 24, 42, 52, 56, 38, 41, 41, 56, 32, 13, 24, 26, 51, 27, 56, 58, 37, 24,
    42, 15, 28, 17, 30, 31, 15, 16, 23, 25, 54, 33, 34, 1, 29, 31, 50, 53, 21, 49, 15, 47, 9, 0,
    58, 37, 45, 30, 13, 9, 45, 63, 21,
];

#[cfg(test)]
#[path = "quic_tests.rs"]
mod tests;
