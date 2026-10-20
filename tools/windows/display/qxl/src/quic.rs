// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! SPICE QUIC codec from display/quic*.c, with caller-owned scratch buffers.

mod bits;
mod model;

use bits::{Reader, Writer};
pub use model::Model;
use model::State;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidImage,
    BufferTooSmall,
    InvalidStream,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Format {
    Gray = 1,
    Rgb555 = 2,
    Bgr24 = 3,
    Bgrx32 = 4,
    Bgra32 = 5,
}

impl Format {
    fn bytes(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb555 => 2,
            Self::Bgr24 => 3,
            Self::Bgrx32 | Self::Bgra32 => 4,
        }
    }

    fn channels(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Bgra32 => 4,
            _ => 3,
        }
    }

    fn depth(self) -> u32 {
        if self == Self::Rgb555 { 5 } else { 8 }
    }

    fn get(self, pixels: &[u8], pixel: usize, channel: usize) -> u8 {
        let base = pixel * self.bytes();
        if self == Self::Rgb555 {
            let value = u16::from_le_bytes([pixels[base], pixels[base + 1]]);
            ((value >> ((2 - channel) * 5)) & 31) as u8
        } else {
            pixels[base + self.offset(channel)]
        }
    }

    fn set(self, pixels: &mut [u8], pixel: usize, channel: usize, value: u8) {
        let base = pixel * self.bytes();
        if self == Self::Rgb555 {
            let shift = (2 - channel) * 5;
            let old = u16::from_le_bytes([pixels[base], pixels[base + 1]]);
            let value = (old & !(31 << shift)) | (u16::from(value) << shift);
            pixels[base..base + 2].copy_from_slice(&value.to_le_bytes());
        } else {
            pixels[base + self.offset(channel)] = value;
        }
    }

    fn offset(self, channel: usize) -> usize {
        if self == Self::Gray || channel == 3 {
            channel
        } else {
            2 - channel
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub format: Format,
    pub width: usize,
    pub height: usize,
}

impl Header {
    pub fn pixel_bytes(self) -> Result<usize, Error> {
        if self.width == 0 || self.height == 0 || self.width > 4096 || self.height > 4096 {
            return Err(Error::InvalidImage);
        }
        self.width
            .checked_mul(self.height)
            .and_then(|n| n.checked_mul(self.format.bytes()))
            .ok_or(Error::InvalidImage)
    }

    pub fn correlation_bytes(self) -> Result<usize, Error> {
        self.pixel_bytes()?;
        Ok((self.width + 1) * self.format.channels())
    }
}

/// Encode tightly packed top-down pixels. BGRX padding is not part of the stream.
pub fn encode(
    header: Header,
    pixels: &[u8],
    output: &mut [u8],
    correlations: &mut [u8],
    models: &mut [Model; 4],
) -> Result<usize, Error> {
    if pixels.len() != header.pixel_bytes()? {
        return Err(Error::InvalidImage);
    }
    let mut writer = Writer::new(output);
    for word in [
        u32::from_le_bytes(*b"QUIC"),
        0,
        header.format as u32,
        header.width as u32,
        header.height as u32,
    ] {
        writer.put(word, 32)?;
    }
    let mut operation = Encode { pixels, writer };
    process(header, &mut operation, correlations, models)?;
    operation.writer.finish()
}

/// Decode to tightly packed top-down pixels, clearing undefined X and RGB555 bits.
pub fn decode(
    input: &[u8],
    pixels: &mut [u8],
    correlations: &mut [u8],
    models: &mut [Model; 4],
) -> Result<Header, Error> {
    let mut reader = Reader::new(input)?;
    if reader.get(32)? != u32::from_le_bytes(*b"QUIC") || reader.get(32)? != 0 {
        return Err(Error::InvalidStream);
    }
    let format = match reader.get(32)? {
        1 => Format::Gray,
        2 => Format::Rgb555,
        3 => Format::Bgr24,
        4 => Format::Bgrx32,
        5 => Format::Bgra32,
        _ => return Err(Error::InvalidStream),
    };
    let header = Header {
        format,
        width: reader.get(32)? as usize,
        height: reader.get(32)? as usize,
    };
    if pixels.len() != header.pixel_bytes()? {
        return Err(Error::InvalidImage);
    }
    pixels.fill(0);
    process(header, &mut Decode { pixels, reader }, correlations, models)?;
    Ok(header)
}

trait Operation {
    fn get(&self, format: Format, pixel: usize, channel: usize) -> u8;
    fn symbol(
        &mut self,
        format: Format,
        pixel: usize,
        channel: usize,
        prediction: u8,
        code: u32,
    ) -> Result<u8, Error>;
    fn run(
        &mut self,
        header: Header,
        row: usize,
        start: usize,
        end: usize,
        channels: core::ops::Range<usize>,
        state: &mut State,
    ) -> Result<usize, Error>;
}

struct Encode<'a, 'b> {
    pixels: &'a [u8],
    writer: Writer<'b>,
}
struct Decode<'a, 'b> {
    pixels: &'a mut [u8],
    reader: Reader<'b>,
}

impl Operation for Encode<'_, '_> {
    fn get(&self, format: Format, pixel: usize, channel: usize) -> u8 {
        format.get(self.pixels, pixel, channel)
    }

    fn symbol(
        &mut self,
        format: Format,
        pixel: usize,
        channel: usize,
        prediction: u8,
        code: u32,
    ) -> Result<u8, Error> {
        let mask = (1u32 << format.depth()) - 1;
        let difference = self.get(format, pixel, channel).wrapping_sub(prediction) as u32 & mask;
        let value = if difference <= mask / 2 {
            difference * 2
        } else {
            (mask - difference) * 2 + 1
        };
        let (word, length) = model::golomb(value, code, format.depth());
        self.writer.put(word, length)?;
        Ok(value as u8)
    }

    fn run(
        &mut self,
        header: Header,
        row: usize,
        start: usize,
        end: usize,
        channels: core::ops::Range<usize>,
        state: &mut State,
    ) -> Result<usize, Error> {
        let mut length = 0;
        while start + length < end
            && channels.clone().all(|c| {
                self.get(header.format, row * header.width + start + length, c)
                    == self.get(header.format, row * header.width + start - 1, c)
            })
        {
            length += 1;
        }
        let mut remainder = length as u32;
        while remainder >= state.run_order() {
            remainder -= state.run_order();
            self.writer.put(1, 1)?;
            state.run_hit();
        }
        self.writer.put(remainder, state.run_bits() + 1)?;
        state.run_done();
        Ok(length)
    }
}

impl Operation for Decode<'_, '_> {
    fn get(&self, format: Format, pixel: usize, channel: usize) -> u8 {
        format.get(self.pixels, pixel, channel)
    }

    fn symbol(
        &mut self,
        format: Format,
        pixel: usize,
        channel: usize,
        prediction: u8,
        code: u32,
    ) -> Result<u8, Error> {
        let depth = format.depth();
        let (prefix, threshold, suffix) = model::family(code, depth);
        let mut zeros = 0;
        while zeros < prefix && self.reader.get(1)? == 0 {
            zeros += 1;
        }
        let value = if zeros == prefix {
            threshold + self.reader.get(suffix)?
        } else {
            (zeros << code) | self.reader.get(code)?
        };
        let mask = (1 << depth) - 1;
        if value > mask {
            return Err(Error::InvalidStream);
        }
        let difference = if value & 1 != 0 {
            mask - (value >> 1)
        } else {
            value >> 1
        };
        format.set(
            self.pixels,
            pixel,
            channel,
            ((difference + u32::from(prediction)) & mask) as u8,
        );
        Ok(value as u8)
    }

    fn run(
        &mut self,
        header: Header,
        row: usize,
        start: usize,
        end: usize,
        channels: core::ops::Range<usize>,
        state: &mut State,
    ) -> Result<usize, Error> {
        let mut length = 0usize;
        while self.reader.get(1)? != 0 {
            length = length
                .checked_add(state.run_order() as usize)
                .filter(|n| *n <= end - start)
                .ok_or(Error::InvalidStream)?;
            state.run_hit();
        }
        length = length
            .checked_add(self.reader.get(state.run_bits())? as usize)
            .filter(|n| *n <= end - start)
            .ok_or(Error::InvalidStream)?;
        state.run_done();
        for x in start..start + length {
            for c in channels.clone() {
                let value = self.get(header.format, row * header.width + start - 1, c);
                header
                    .format
                    .set(self.pixels, row * header.width + x, c, value);
            }
        }
        Ok(length)
    }
}

fn process(
    header: Header,
    operation: &mut impl Operation,
    correlations: &mut [u8],
    models: &mut [Model; 4],
) -> Result<(), Error> {
    let correlations = correlations
        .get_mut(..header.correlation_bytes()?)
        .ok_or(Error::BufferTooSmall)?;
    correlations.fill(0);
    for model in models.iter_mut() {
        model.reset(header.format.depth());
    }
    let mut rgb = State::new();
    let mut alpha = State::new();
    for row in 0..header.height {
        let channels = header.format.channels();
        for c in 0..channels {
            let base = c * (header.width + 1);
            correlations[base] = if row == 0 { 0 } else { correlations[base + 1] };
        }
        process_row(
            header,
            row,
            0..channels.min(3),
            operation,
            correlations,
            models,
            &mut rgb,
        )?;
        if channels == 4 {
            process_row(
                header,
                row,
                3..4,
                operation,
                correlations,
                models,
                &mut alpha,
            )?;
        }
    }
    Ok(())
}

struct Scratch<'a> {
    correlations: &'a mut [u8],
    models: &'a mut [Model; 4],
}

fn process_row(
    header: Header,
    row: usize,
    channels: core::ops::Range<usize>,
    operation: &mut impl Operation,
    correlations: &mut [u8],
    models: &mut [Model; 4],
    state: &mut State,
) -> Result<(), Error> {
    let mut scratch = Scratch {
        correlations,
        models,
    };
    let mut start = 0;
    while start < header.width {
        let length = if state.window < 6 {
            state.left.min(header.width - start)
        } else {
            header.width - start
        };
        if length != 0 {
            process_segment(
                header,
                row,
                start..start + length,
                channels.clone(),
                operation,
                &mut scratch,
                state,
            )?;
            start += length;
            if state.window < 6 {
                state.left -= length;
            }
        }
        if state.window < 6 && state.left == 0 {
            state.window += 1;
            state.left = 2048;
        }
    }
    Ok(())
}

fn process_segment(
    header: Header,
    row: usize,
    range: core::ops::Range<usize>,
    channels: core::ops::Range<usize>,
    operation: &mut impl Operation,
    scratch: &mut Scratch<'_>,
    state: &mut State,
) -> Result<(), Error> {
    let end = range.end;
    let mut x = range.start;
    let mut run_index = 0;
    let mut stop = x + state.wait;
    if x == 0 {
        pixel(
            header,
            row,
            x,
            channels.clone(),
            operation,
            scratch.correlations,
            scratch.models,
        )?;
        if state.wait != 0 {
            state.wait -= 1;
        } else {
            state.wait = state.random_wait();
            update(
                header,
                x,
                channels.clone(),
                scratch.correlations,
                scratch.models,
                state,
            );
        }
        x += 1;
        stop = x + state.wait;
    }
    while x < end {
        if row != 0
            && x > 2
            && run_index != x
            && channels.clone().all(|c| {
                operation.get(header.format, (row - 1) * header.width + x - 1, c)
                    == operation.get(header.format, (row - 1) * header.width + x, c)
                    && operation.get(header.format, row * header.width + x - 1, c)
                        == operation.get(header.format, row * header.width + x - 2, c)
            })
        {
            run_index = x;
            state.wait = stop - x;
            x += operation.run(header, row, x, end, channels.clone(), state)?;
            if x == end {
                return Ok(());
            }
            stop = x + state.wait;
            continue;
        }
        pixel(
            header,
            row,
            x,
            channels.clone(),
            operation,
            scratch.correlations,
            scratch.models,
        )?;
        if x == stop {
            update(
                header,
                x,
                channels.clone(),
                scratch.correlations,
                scratch.models,
                state,
            );
            stop = x + 1 + state.random_wait();
        }
        x += 1;
    }
    state.wait = stop - end;
    Ok(())
}

fn pixel(
    header: Header,
    row: usize,
    x: usize,
    channels: core::ops::Range<usize>,
    operation: &mut impl Operation,
    correlations: &mut [u8],
    models: &mut [Model; 4],
) -> Result<(), Error> {
    for c in channels {
        let left = if x == 0 {
            0
        } else {
            operation.get(header.format, row * header.width + x - 1, c)
        };
        let prediction = if row == 0 {
            left
        } else {
            let above = operation.get(header.format, (row - 1) * header.width + x, c);
            if x == 0 {
                above
            } else {
                ((u16::from(left) + u16::from(above)) / 2) as u8
            }
        };
        let offset = c * (header.width + 1) + x;
        let context = correlations[offset];
        correlations[offset + 1] = operation.symbol(
            header.format,
            row * header.width + x,
            c,
            prediction,
            models[c].code(context),
        )?;
    }
    Ok(())
}

fn update(
    header: Header,
    x: usize,
    channels: core::ops::Range<usize>,
    correlations: &[u8],
    models: &mut [Model; 4],
    state: &State,
) {
    for c in channels {
        let offset = c * (header.width + 1) + x;
        models[c].update(
            correlations[offset],
            correlations[offset + 1],
            header.format.depth(),
            state.trigger(),
        );
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub(crate) mod native {
    use super::*;
    use crate::display_resources::Pool;
    use crate::engine::Memory;
    use crate::image::Image;
    use crate::slots::MemorySlot;

    /// # Safety
    /// Run at PASSIVE_LEVEL with the exclusive live pool and its active slot.
    pub(crate) unsafe fn compress<'mapping>(
        header: Header,
        pixels: &[u8],
        pool: &mut Pool<'mapping>,
        slot: MemorySlot,
    ) -> Option<Image<'mapping>> {
        let capacity = header.pixel_bytes().ok()?.checked_sub(42)?.checked_div(4)? * 4;
        if capacity < 24 {
            return None;
        }
        // SAFETY: the callback retains GDI at PASSIVE_LEVEL; each buffer is private.
        let (output, correlation, model) = unsafe {
            (
                Memory::zeroed(u32::try_from(capacity).ok()?).ok()?,
                Memory::zeroed(u32::try_from(header.correlation_bytes().ok()?).ok()?).ok()?,
                Memory::zeroed(core::mem::size_of::<[Model; 4]>() as u32).ok()?,
            )
        };
        let base = model.as_ptr().as_ptr().cast::<Model>();
        if !base.addr().is_multiple_of(core::mem::align_of::<Model>()) {
            return None;
        }
        for index in 0..4 {
            // SAFETY: the fresh aligned allocation contains four disjoint models.
            unsafe { base.add(index).write(Model::new()) };
        }
        // SAFETY: these fresh, disjoint allocations cover the checked extents;
        // every model is initialised and the owners outlive all mutable borrows.
        let (bytes, correlations, models) = unsafe {
            (
                core::slice::from_raw_parts_mut(output.as_ptr().as_ptr(), capacity),
                core::slice::from_raw_parts_mut(
                    correlation.as_ptr().as_ptr(),
                    header.correlation_bytes().ok()?,
                ),
                &mut *base.cast::<[Model; 4]>(),
            )
        };
        let length = encode(header, pixels, bytes, correlations, models).ok()?;
        Image::allocate_quic(pool, slot, header, &bytes[..length]).ok()
    }
}
