// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Cursor shape conversion and output ownership from display/pointer.c and res.c.

use crate::display_resources::{
    Command as OutputCommand, Error as PoolError, Pool, Region, Resource,
};
use crate::ring::CommandRing;
use crate::slots::MemorySlot;

const SHAPE_HEADER: usize = 42;
const COMMAND_BYTES: usize = 150;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    OutputTooSmall,
    Memory(PoolError),
}

impl From<PoolError> for Error {
    fn from(error: PoolError) -> Self {
        Self::Memory(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Mono,
    Indexed4,
    Indexed8,
    Rgb555,
    Bgr24,
    Bgrx32,
}

impl Format {
    fn bits(self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Indexed4 => 4,
            Self::Indexed8 => 8,
            Self::Rgb555 => 16,
            Self::Bgr24 => 24,
            Self::Bgrx32 => 32,
        }
    }
}

/// A checked immutable GDI bitmap view; first is the logical top row's offset.
#[derive(Clone, Copy, Debug)]
pub struct Bitmap<'pixels> {
    pixels: &'pixels [u8],
    width: u16,
    height: u16,
    first: usize,
    stride: i32,
    row_bytes: usize,
    format: Format,
}

impl<'pixels> Bitmap<'pixels> {
    pub fn new(
        pixels: &'pixels [u8],
        width: u16,
        height: u16,
        first: usize,
        stride: i32,
        format: Format,
    ) -> Result<Self, Error> {
        let row_bytes = (usize::from(width) * format.bits()).div_ceil(8);
        let last = i64::try_from(first).ok().and_then(|first| {
            first.checked_add(i64::from(height.saturating_sub(1)) * i64::from(stride))
        });
        if width == 0
            || height == 0
            || (stride.unsigned_abs() as usize) < row_bytes
            || first
                .checked_add(row_bytes)
                .is_none_or(|end| end > pixels.len())
            || last.is_none_or(|last| {
                last < 0
                    || usize::try_from(last)
                        .ok()
                        .and_then(|last| last.checked_add(row_bytes))
                        .is_none_or(|end| end > pixels.len())
            })
        {
            return Err(Error::InvalidInput);
        }
        Ok(Self {
            pixels,
            width,
            height,
            first,
            stride,
            row_bytes,
            format,
        })
    }

    fn row(self, row: u16) -> &'pixels [u8] {
        let offset = (self.first as i64 + i64::from(row) * i64::from(self.stride)) as usize;
        &self.pixels[offset..offset + self.row_bytes]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Palette<'colors> {
    pub colors: &'colors [u32],
    pub format: crate::surface::Format,
}

#[derive(Clone, Copy, Debug)]
pub enum Input<'pixels> {
    Transparent,
    Alpha {
        bitmap: Bitmap<'pixels>,
        hotspot: [u16; 2],
    },
    Mono {
        mask: Bitmap<'pixels>,
        hotspot: [u16; 2],
    },
    Color {
        bitmap: Bitmap<'pixels>,
        mask: Bitmap<'pixels>,
        palette: Option<Palette<'pixels>>,
        hotspot: [u16; 2],
    },
}

#[derive(Clone, Copy, Debug)]
struct Description {
    kind: u16,
    width: u16,
    height: u16,
    hotspot: [u16; 2],
    data_bytes: usize,
}

impl Input<'_> {
    fn describe(self) -> Result<Description, Error> {
        let (bitmap, kind, height, hotspot, extra) = match self {
            Self::Transparent => {
                return Ok(Description {
                    kind: 1,
                    width: 0,
                    height: 0,
                    hotspot: [0, 0],
                    data_bytes: 0,
                });
            }
            Self::Alpha { bitmap, hotspot } if bitmap.format == Format::Bgrx32 => {
                (bitmap, 0, bitmap.height, hotspot, 0)
            }
            Self::Mono { mask, hotspot }
                if mask.format == Format::Mono && mask.height.is_multiple_of(2) =>
            {
                (mask, 1, mask.height / 2, hotspot, 0)
            }
            Self::Color {
                bitmap,
                mask,
                palette,
                hotspot,
            } => {
                if mask.format != Format::Mono
                    || mask.width != bitmap.width
                    || bitmap.height.checked_mul(2) != Some(mask.height)
                {
                    return Err(Error::InvalidInput);
                }
                let (kind, entries) = match bitmap.format {
                    Format::Indexed4 => (2, 16),
                    Format::Indexed8 => (3, 256),
                    Format::Rgb555 => (4, 0),
                    Format::Bgr24 => (5, 0),
                    Format::Bgrx32 => (6, 0),
                    _ => return Err(Error::InvalidInput),
                };
                if (entries != 0 && palette.is_none_or(|palette| palette.colors.len() != entries))
                    || (entries == 0 && palette.is_some())
                {
                    return Err(Error::InvalidInput);
                }
                (
                    bitmap,
                    kind,
                    bitmap.height,
                    hotspot,
                    entries * 4 + mask.row_bytes * usize::from(bitmap.height),
                )
            }
            _ => return Err(Error::InvalidInput),
        };
        if hotspot[0] >= bitmap.width || hotspot[1] >= height {
            return Err(Error::InvalidInput);
        }
        let data_bytes = bitmap
            .row_bytes
            .checked_mul(usize::from(bitmap.height))
            .and_then(|bytes| bytes.checked_add(extra))
            .filter(|bytes| *bytes <= u32::MAX as usize)
            .ok_or(Error::InvalidInput)?;
        Ok(Description {
            kind,
            width: bitmap.width,
            height,
            hotspot,
            data_bytes,
        })
    }

    pub fn encoded_len(self) -> Result<usize, Error> {
        self.describe()?
            .data_bytes
            .checked_add(SHAPE_HEADER)
            .ok_or(Error::InvalidInput)
    }

    /// Encode a single packed chunk. Invalid or short output remains unchanged.
    pub fn encode(self, unique: u64, output: &mut [u8]) -> Result<usize, Error> {
        let description = self.describe()?;
        let length = description
            .data_bytes
            .checked_add(SHAPE_HEADER)
            .ok_or(Error::InvalidInput)?;
        let output = output.get_mut(..length).ok_or(Error::OutputTooSmall)?;
        output.fill(0);
        // A transparent shape is never a host-cache identity.
        output[..8].copy_from_slice(
            &if matches!(self, Self::Transparent) {
                0
            } else {
                unique
            }
            .to_le_bytes(),
        );
        for (offset, value) in [
            (8, description.kind),
            (10, description.width),
            (12, description.height),
            (14, description.hotspot[0]),
            (16, description.hotspot[1]),
        ] {
            output[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        let bytes = (description.data_bytes as u32).to_le_bytes();
        output[18..22].copy_from_slice(&bytes);
        output[22..26].copy_from_slice(&bytes);
        let mut position = SHAPE_HEADER;
        let mut append = |bytes: &[u8]| {
            output[position..position + bytes.len()].copy_from_slice(bytes);
            position += bytes.len();
        };
        let bitmap = match self {
            Self::Transparent => return Ok(length),
            Self::Alpha { bitmap, .. } | Self::Color { bitmap, .. } => bitmap,
            Self::Mono { mask, .. } => mask,
        };
        for row in 0..bitmap.height {
            append(bitmap.row(row));
        }
        if let Self::Color { mask, palette, .. } = self {
            if let Some(palette) = palette {
                for &color in palette.colors {
                    let color = match palette.format {
                        crate::surface::Format::Xrgb8888 => color,
                        crate::surface::Format::Rgb555 => {
                            ((color & 0x001f) << 3)
                                | ((color & 0x001c) >> 2)
                                | ((color & 0x03e0) << 6)
                                | ((color & 0x0380) << 1)
                                | ((color & 0x7c00) << 9)
                                | ((color & 0x7000) << 4)
                        }
                    };
                    append(&color.to_le_bytes());
                }
            }
            for row in 0..bitmap.height {
                append(mask.row(row));
            }
        }
        Ok(length)
    }
}

#[derive(Debug)]
#[must_use]
pub struct Shape<'memory> {
    memory: Resource<'memory>,
    physical: u64,
}

impl<'memory> Shape<'memory> {
    pub fn allocate(
        pool: &mut Pool<'memory>,
        slot: MemorySlot,
        input: Input<'_>,
        unique: u64,
    ) -> Result<Self, Error> {
        let length = input.encoded_len()?;
        let memory = pool.resource(Region::Main, length)?;
        let physical = match slot.encode(memory.as_ptr().addr() as u64, length as u64) {
            Ok(physical) => physical,
            Err(error) => {
                pool.release(memory).expect("cursor allocation ownership");
                return Err(Error::Memory(PoolError::Address(error)));
            }
        };
        // SAFETY: this unpublished resource has one reference and contains the
        // full checked extent. Its immutable shape API never exposes this slice.
        let bytes = unsafe { core::slice::from_raw_parts_mut(memory.as_ptr(), length) };
        if let Err(error) = input.encode(unique, bytes) {
            pool.release(memory).expect("cursor allocation ownership");
            return Err(error);
        }
        Ok(Self { memory, physical })
    }

    pub fn release(self, pool: &mut Pool<'memory>) -> Result<(), (PoolError, Self)> {
        match pool.release(self.memory) {
            Ok(()) => Ok(()),
            Err((error, memory)) => Err((
                error,
                Self {
                    memory,
                    physical: self.physical,
                },
            )),
        }
    }

    /// # Safety
    /// Use the same active mapping, pool and cursor ring generation. Keep them
    /// live through host release, and serialise ring/pool access. If unique is
    /// nonzero, it must identify these exact immutable shape bytes for the host.
    pub unsafe fn set(
        &self,
        pool: &mut Pool<'memory>,
        ring: &mut CommandRing,
        position: Position,
    ) -> Result<bool, PoolError> {
        let mut output = pool.output(OutputCommand::Cursor, COMMAND_BYTES)?;
        let bytes = pool.payload(&mut output).expect("cursor command ownership");
        bytes[8] = 0;
        if let Position::At([x, y]) = position {
            bytes[9..11].copy_from_slice(&x.to_le_bytes());
            bytes[11..13].copy_from_slice(&y.to_le_bytes());
            bytes[13] = 1;
        }
        bytes[14..22].copy_from_slice(&self.physical.to_le_bytes());
        // SAFETY: the caller retains the active mapping/ring; the attached
        // reference keeps the immutable shape live independently of this owner.
        unsafe { pool.submit_retained(output, ring, &self.memory) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Hidden,
    At([i16; 2]),
}

impl Position {
    pub fn new(x: i32, y: i32) -> Result<Self, Error> {
        if x < 0 {
            return Ok(Self::Hidden);
        }
        if y < 0 {
            return Err(Error::InvalidInput);
        }
        Ok(Self::At([
            i16::try_from(x).map_err(|_| Error::InvalidInput)?,
            i16::try_from(y).map_err(|_| Error::InvalidInput)?,
        ]))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trail {
    length: u16,
    frequency: u16,
}

impl Trail {
    pub fn from_flags(flags: u32) -> Self {
        Self {
            length: ((flags & 0xf00) >> 8) as u16,
            frequency: ((flags & 0xff000) >> 12) as u16,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Move(Position),
    Trail(Trail),
}

impl Command {
    /// # Safety
    /// Supply the same live device/pool/cursor-ring generation and serialised
    /// access as Shape::set; retain mappings through host release.
    pub unsafe fn submit(
        self,
        pool: &mut Pool<'_>,
        ring: &mut CommandRing,
    ) -> Result<bool, PoolError> {
        let mut output = pool.output(OutputCommand::Cursor, COMMAND_BYTES)?;
        let bytes = pool.payload(&mut output).expect("cursor command ownership");
        match self {
            Self::Move(Position::Hidden) => bytes[8] = 2,
            Self::Move(Position::At([x, y])) => {
                bytes[8] = 1;
                bytes[9..11].copy_from_slice(&x.to_le_bytes());
                bytes[11..13].copy_from_slice(&y.to_le_bytes());
            }
            Self::Trail(trail) => {
                bytes[8] = 3;
                bytes[9..11].copy_from_slice(&trail.length.to_le_bytes());
                bytes[11..13].copy_from_slice(&trail.frequency.to_le_bytes());
            }
        }
        // SAFETY: the caller retains the live ring/mapping until host release.
        match unsafe { pool.submit(output, ring) } {
            Ok(notify) => Ok(notify),
            Err((error, output)) => {
                pool.cancel(output).expect("cursor command ownership");
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_resources::test_utils::{Bytes, HostRing, empty, pool};
    use crate::ring::ReleaseRing;
    use crate::slots::SlotLayout;

    #[test]
    fn signed_strides_select_logical_rows_and_omit_gdi_padding() {
        let pixels = [
            1, 2, 3, 4, 0xaa, 0xaa, 0xaa, 0xaa, 5, 6, 7, 8, 0xbb, 0xbb, 0xbb, 0xbb,
        ];
        for (first, stride, expected) in [
            (0, 8, [1, 2, 3, 4, 5, 6, 7, 8]),
            (8, -8, [5, 6, 7, 8, 1, 2, 3, 4]),
        ] {
            let bitmap = Bitmap::new(&pixels, 1, 2, first, stride, Format::Bgrx32).unwrap();
            let mut output = [0xcc; 54];
            let length = Input::Alpha {
                bitmap,
                hotspot: [0, 1],
            }
            .encode(7, &mut output)
            .unwrap();
            assert_eq!(length, 50);
            assert_eq!(&output[..8], &7u64.to_le_bytes());
            assert_eq!(&output[8..18], &[0, 0, 1, 0, 2, 0, 0, 0, 1, 0]);
            assert_eq!(&output[18..26], &[8, 0, 0, 0, 8, 0, 0, 0]);
            assert_eq!(&output[26..42], &[0; 16]);
            assert_eq!(&output[42..50], &expected);
            assert_eq!(&output[50..], &[0xcc; 4]);
        }
        for (width, height, first, stride) in [
            (0, 1, 0, 4),
            (1, 0, 0, 4),
            (1, 2, 0, -4),
            (1, 1, 13, 4),
            (1, 2, usize::MAX, 4),
            (2, 1, 0, 4),
        ] {
            assert!(Bitmap::new(&pixels, width, height, first, stride, Format::Bgrx32).is_err());
        }
    }

    #[test]
    fn mono_preserves_and_xor_planes_and_transparent_has_no_cache_identity() {
        let bitmap = Bitmap::new(&[0x80, 0xee, 0x40, 0xee], 2, 2, 0, 2, Format::Mono).unwrap();
        let mut output = [0xcc; 44];
        Input::Mono {
            mask: bitmap,
            hotspot: [1, 0],
        }
        .encode(9, &mut output)
        .unwrap();
        assert_eq!(&output[8..18], &[1, 0, 2, 0, 1, 0, 1, 0, 0, 0]);
        assert_eq!(&output[42..], &[0x80, 0x40]);
        let mut transparent = [0xcc; SHAPE_HEADER];
        Input::Transparent.encode(99, &mut transparent).unwrap();
        assert_eq!(transparent[8], 1);
        transparent[8] = 0;
        assert_eq!(transparent, [0; SHAPE_HEADER]);
    }

    #[test]
    fn every_colour_depth_keeps_palette_then_and_mask_order() {
        let colors = [0x7c00u32; 256];
        for (format, pixels, palette_entries, kind) in [
            (Format::Indexed4, 1, 16, 2),
            (Format::Indexed8, 2, 256, 3),
            (Format::Rgb555, 4, 0, 4),
            (Format::Bgr24, 6, 0, 5),
            (Format::Bgrx32, 8, 0, 6),
        ] {
            let data = [0x5a; 8];
            let bitmap = Bitmap::new(&data, 2, 1, 0, pixels as i32, format).unwrap();
            let mask = Bitmap::new(&[0x80, 0xff], 2, 2, 0, 1, Format::Mono).unwrap();
            let palette = (palette_entries != 0).then_some(Palette {
                colors: &colors[..palette_entries],
                format: crate::surface::Format::Rgb555,
            });
            let input = Input::Color {
                bitmap,
                mask,
                palette,
                hotspot: [0, 0],
            };
            let length = SHAPE_HEADER + pixels + palette_entries * 4 + 1;
            assert_eq!(input.encoded_len(), Ok(length));
            let mut output = [0xcc; 1100];
            input.encode(0, &mut output).unwrap();
            assert_eq!(u16::from_le_bytes(output[8..10].try_into().unwrap()), kind);
            assert_eq!(&output[42..42 + pixels], &[0x5a; 8][..pixels]);
            for color in output[42 + pixels..length - 1].as_chunks::<4>().0 {
                assert_eq!(u32::from_le_bytes(*color), 0xff0000);
            }
            assert_eq!(output[length - 1], 0x80);
            assert_eq!(output[length], 0xcc);
        }
    }

    #[test]
    fn invalid_shapes_and_short_outputs_are_unchanged() {
        let mono = Bitmap::new(&[0; 2], 1, 2, 0, 1, Format::Mono).unwrap();
        let color = Bitmap::new(&[0; 4], 1, 1, 0, 4, Format::Bgrx32).unwrap();
        let indexed = Bitmap::new(&[0], 1, 1, 0, 1, Format::Indexed8).unwrap();
        for input in [
            Input::Alpha {
                bitmap: mono,
                hotspot: [0, 0],
            },
            Input::Alpha {
                bitmap: color,
                hotspot: [1, 0],
            },
            Input::Mono {
                mask: color,
                hotspot: [0, 0],
            },
            Input::Color {
                bitmap: indexed,
                mask: mono,
                palette: None,
                hotspot: [0, 0],
            },
        ] {
            let mut output = [0xcc; 128];
            assert_eq!(input.encode(0, &mut output), Err(Error::InvalidInput));
            assert_eq!(output, [0xcc; 128]);
        }
        let mut short = [0xcc; 41];
        assert_eq!(
            Input::Transparent.encode(0, &mut short),
            Err(Error::OutputTooSmall)
        );
        assert_eq!(short, [0xcc; 41]);
        assert_eq!(Position::new(-1, -1), Ok(Position::Hidden));
        assert_eq!(Position::new(4, 5), Ok(Position::At([4, 5])));
        assert_eq!(Position::new(0, -1), Err(Error::InvalidInput));
        assert_eq!(Position::new(32768, 0), Err(Error::InvalidInput));
    }

    #[test]
    fn indexed_palette_preserves_32bit_translations() {
        let colors = [0x00123456u32; 16];
        let bitmap = Bitmap::new(&[0x10], 2, 1, 0, 1, Format::Indexed4).unwrap();
        let mask = Bitmap::new(&[0x80, 0], 2, 2, 0, 1, Format::Mono).unwrap();
        let mut output = [0xcc; SHAPE_HEADER + 1 + 64 + 1];
        Input::Color {
            bitmap,
            mask,
            palette: Some(Palette {
                colors: &colors,
                format: crate::surface::Format::Xrgb8888,
            }),
            hotspot: [0, 0],
        }
        .encode(1, &mut output)
        .unwrap();
        for color in output[43..107].as_chunks::<4>().0 {
            assert_eq!(u32::from_le_bytes(*color), 0x00123456);
        }
        assert_eq!(output[107], 0x80);
    }

    fn slot(memory: &mut [u64]) -> MemorySlot {
        SlotLayout::new(8, 8)
            .unwrap()
            .slot(
                1,
                4,
                0xe0000000,
                memory.as_ptr().addr() as u64,
                size_of_val(memory) as u64,
            )
            .unwrap()
    }

    #[test]
    fn cache_release_does_not_free_a_shape_before_host_completion() {
        let mut memory = [0u64; 1024];
        let slot = slot(&mut memory);
        let mut pool = pool(&mut memory, None);
        let mut command_bytes = Bytes([0; 532]);
        let host = HostRing::new(&mut command_bytes, 32, 16);
        let mut release_bytes = Bytes([0; 84]);
        let mut release_host = HostRing::new(&mut release_bytes, 8, 8);
        // SAFETY: the independent live mappings each have one guest endpoint.
        let (mut ring, mut releases) = unsafe {
            (
                CommandRing::new(host.base, 532).unwrap(),
                ReleaseRing::new(release_host.base, 84).unwrap(),
            )
        };
        let shape = Shape::allocate(&mut pool, slot, Input::Transparent, 9).unwrap();
        let pointer = shape.memory.as_ptr();
        let physical = shape.physical;
        // SAFETY: the shape, pool, slot and cursor ring use this active mapping.
        unsafe { shape.set(&mut pool, &mut ring, Position::At([3, 4])) }.unwrap();
        let published = host.command(&pool, 0);
        assert_eq!(published.command, 3);
        // SAFETY: the cursor command has its full packed 150-byte extent.
        let bytes = unsafe { published.snapshot::<COMMAND_BYTES>() };
        assert_eq!(&bytes[8..14], &[0, 3, 0, 4, 0, 1]);
        assert_eq!(&bytes[14..22], &physical.to_le_bytes());
        assert_eq!(&bytes[22..], &[0; 128]);
        shape.release(&mut pool).unwrap();
        assert!(!pool.is_empty());
        // SAFETY: the pending output retains its shape despite cache eviction.
        assert_eq!(unsafe { pointer.add(8).read() }, 1);
        host.set(12, 1);
        published.complete(0);
        release_host.release(published.id);
        // SAFETY: the host stopped all command/shape reads before completion.
        assert_eq!(unsafe { pool.reap(&mut releases, 1) }.unwrap().outputs, 1);
        empty(&mut pool);
    }

    #[test]
    fn full_ring_and_wrong_pool_failures_keep_the_cached_shape_owned() {
        let mut memory = [0u64; 1024];
        let slot = slot(&mut memory);
        let mut pool = pool(&mut memory, None);
        let mut command_bytes = Bytes([0; 532]);
        let host = HostRing::new(&mut command_bytes, 32, 16);
        // SAFETY: the live mapping has one guest endpoint.
        let mut ring = unsafe { CommandRing::new(host.base, 532).unwrap() };
        let shape = Shape::allocate(&mut pool, slot, Input::Transparent, 0).unwrap();
        host.set(4, 32);
        // SAFETY: mapping and generation match; full publication must roll back.
        let result = unsafe { shape.set(&mut pool, &mut ring, Position::Hidden) };
        assert_eq!(result, Err(PoolError::RingFull));
        let mut foreign_memory = [0u64; 128];
        let mut foreign = self::pool(&mut foreign_memory, None);
        // SAFETY: wrong-pool rejection happens before publication/ring access.
        let result = unsafe { shape.set(&mut foreign, &mut ring, Position::Hidden) };
        assert_eq!(result, Err(PoolError::WrongPool));
        empty(&mut foreign);
        shape.release(&mut pool).unwrap();
        empty(&mut pool);
    }

    #[test]
    fn move_hide_and_trail_use_the_same_release_queue() {
        let mut memory = [0u64; 1024];
        let mut pool = pool(&mut memory, None);
        let mut command_bytes = Bytes([0; 532]);
        let host = HostRing::new(&mut command_bytes, 32, 16);
        // SAFETY: the live mapping has one guest endpoint.
        let mut ring = unsafe { CommandRing::new(host.base, 532).unwrap() };
        for (index, (command, expected)) in [
            (Command::Move(Position::At([7, 8])), [1, 7, 0, 8, 0]),
            (Command::Move(Position::Hidden), [2, 0, 0, 0, 0]),
            (
                Command::Trail(Trail::from_flags(0x000abf11)),
                [3, 15, 0, 171, 0],
            ),
        ]
        .into_iter()
        .enumerate()
        {
            // SAFETY: this pool and cursor ring share the active mapping.
            unsafe { command.submit(&mut pool, &mut ring) }.unwrap();
            let output = host.command(&pool, index as u32);
            // SAFETY: each published command has the complete packed extent.
            let bytes = unsafe { output.snapshot::<COMMAND_BYTES>() };
            assert_eq!(&bytes[8..13], &expected);
        }
        // SAFETY: this simulated host stops all access without release writes.
        unsafe { pool.discard_pending() };
        empty(&mut pool);
    }
}
