// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Owned raw and QUIC QXL images from display/res.c.

use crate::bitmap::{Bitmap, Format};
use crate::display_resources::{Error, Pool, Region, Resource};
use crate::palette::Palette;
use crate::slots::MemorySlot;

const HEADER_BYTES: usize = 48;

#[derive(Debug)]
pub struct Image<'memory> {
    memory: Resource<'memory>,
    physical: u64,
    width: u32,
    height: u32,
    format: Format,
    palette: Option<Palette<'memory>>,
    compressed: bool,
}

impl<'memory> Image<'memory> {
    pub fn allocate(
        pool: &mut Pool<'memory>,
        slot: MemorySlot,
        width: u32,
        height: u32,
        pixels: &[u8],
        alpha: bool,
    ) -> Result<Self, Error> {
        let format = if alpha {
            Format::Bgra32
        } else {
            Format::Bgrx32
        };
        let bitmap = Bitmap::new(width, height, format, pixels).ok_or(Error::InvalidSize)?;
        Self::allocate_bitmap(pool, slot, bitmap, None)
    }

    pub fn allocate_bitmap(
        pool: &mut Pool<'memory>,
        slot: MemorySlot,
        bitmap: Bitmap<'_>,
        palette: Option<&Palette<'memory>>,
    ) -> Result<Self, Error> {
        if !bitmap.validate_palette(palette.map_or(0, |palette| palette.colors().len() / 4)) {
            return Err(Error::InvalidSize);
        }
        if let Some(palette) = palette {
            if !pool.owns(palette.resource()) {
                return Err(Error::WrongPool);
            }
            palette.validate_slot(slot)?;
        }
        let bytes = HEADER_BYTES
            .checked_add(bitmap.pixels.len())
            .ok_or(Error::InvalidSize)?;
        let (memory, physical) = allocate_record(pool, slot, bytes)?;
        let retained = match palette.map(|palette| palette.retain(pool)).transpose() {
            Ok(retained) => retained,
            Err(error) => {
                pool.release(memory).map_err(|(error, _)| error)?;
                return Err(error);
            }
        };
        let image = Self {
            memory,
            physical,
            width: bitmap.width,
            height: bitmap.height,
            format: bitmap.format,
            palette: retained,
            compressed: false,
        };
        // SAFETY: the fresh allocation owns the entire unpublished image record.
        let output = unsafe { core::slice::from_raw_parts_mut(image.memory.as_ptr(), bytes) };
        if let Err(error) = encode_bitmap(
            bitmap,
            physical,
            palette.map_or(0, Palette::address),
            output,
        ) {
            image.release(pool).map_err(|(error, _)| error)?;
            return Err(error);
        }
        Ok(image)
    }

    #[cfg(any(test, all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
    pub(crate) fn allocate_quic(
        pool: &mut Pool<'memory>,
        slot: MemorySlot,
        header: crate::quic::Header,
        stream: &[u8],
    ) -> Result<Self, Error> {
        header.pixel_bytes().map_err(|_| Error::InvalidSize)?;
        if stream.len() < 24 || !stream.len().is_multiple_of(4) {
            return Err(Error::InvalidSize);
        }
        let expected = [
            u32::from_le_bytes(*b"QUIC"),
            0,
            header.format as u32,
            header.width as u32,
            header.height as u32,
        ];
        for (word, expected) in stream[..20].as_chunks::<4>().0.iter().zip(expected) {
            if *word != expected.to_le_bytes() {
                return Err(Error::InvalidSize);
            }
        }
        let format = match header.format {
            crate::quic::Format::Rgb555 => Format::Rgb555,
            crate::quic::Format::Bgr24 => Format::Bgr24,
            crate::quic::Format::Bgrx32 => Format::Bgrx32,
            crate::quic::Format::Bgra32 => Format::Bgra32,
            crate::quic::Format::Gray => return Err(Error::InvalidSize),
        };
        let size = u32::try_from(stream.len()).map_err(|_| Error::InvalidSize)?;
        let bytes = 42usize
            .checked_add(stream.len())
            .ok_or(Error::InvalidSize)?;
        let (memory, physical) = allocate_record(pool, slot, bytes)?;
        // SAFETY: the fresh resource exclusively owns this unpublished record.
        let output = unsafe { core::slice::from_raw_parts_mut(memory.as_ptr(), bytes) };
        output[..42].fill(0);
        output[..8].copy_from_slice(&3u64.to_le_bytes());
        output[8] = 1;
        output[10..14].copy_from_slice(&(header.width as u32).to_le_bytes());
        output[14..18].copy_from_slice(&(header.height as u32).to_le_bytes());
        output[18..22].copy_from_slice(&size.to_le_bytes());
        output[22..26].copy_from_slice(&size.to_le_bytes());
        output[42..].copy_from_slice(stream);
        Ok(Self {
            memory,
            physical,
            width: header.width as u32,
            height: header.height as u32,
            format,
            palette: None,
            compressed: true,
        })
    }

    pub fn address(&self) -> u64 {
        self.physical
    }
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    pub(crate) fn resource(&self) -> &Resource<'memory> {
        &self.memory
    }

    pub fn retain(&self, pool: &mut Pool<'memory>) -> Result<Self, Error> {
        let memory = pool.retain(&self.memory)?;
        let palette = match self
            .palette
            .as_ref()
            .map(|palette| palette.retain(pool))
            .transpose()
        {
            Ok(palette) => palette,
            Err(error) => {
                pool.release(memory).map_err(|(error, _)| error)?;
                return Err(error);
            }
        };
        Ok(Self {
            memory,
            physical: self.physical,
            width: self.width,
            height: self.height,
            format: self.format,
            palette,
            compressed: self.compressed,
        })
    }

    pub fn matches(&self, width: u32, height: u32, pixels: &[u8], alpha: bool) -> bool {
        let format = if alpha {
            Format::Bgra32
        } else {
            Format::Bgrx32
        };
        Bitmap::new(width, height, format, pixels)
            .is_some_and(|bitmap| self.matches_bitmap(bitmap, None))
    }

    pub(crate) fn matches_bitmap(
        &self,
        bitmap: Bitmap<'_>,
        palette: Option<&Palette<'memory>>,
    ) -> bool {
        if self.compressed
            || (self.width, self.height, self.format)
                != (bitmap.width, bitmap.height, bitmap.format)
            || self.memory.len() - HEADER_BYTES != bitmap.pixels.len()
        {
            return false;
        }
        let same_palette = match (&self.palette, palette) {
            (None, None) => true,
            (Some(stored), Some(palette)) => stored.colors() == palette.colors(),
            _ => false,
        };
        // SAFETY: this reference retains the complete immutable image payload.
        let stored = unsafe {
            core::slice::from_raw_parts(self.memory.as_ptr().add(HEADER_BYTES), bitmap.pixels.len())
        };
        same_palette && stored == bitmap.pixels
    }

    pub(crate) fn palette_resource(&self) -> Option<&Resource<'memory>> {
        self.palette.as_ref().map(Palette::resource)
    }

    pub fn bytes(&self) -> usize {
        self.memory.len()
    }

    pub fn release(self, pool: &mut Pool<'memory>) -> Result<(), (Error, Self)> {
        match self.palette {
            Some(palette) => {
                palette
                    .release_with(self.memory, pool)
                    .map_err(|(error, memory, palette)| {
                        (
                            error,
                            Self {
                                memory,
                                palette: Some(palette),
                                ..self
                            },
                        )
                    })
            }
            None => pool.release(self.memory).map_err(|(error, memory)| {
                (
                    error,
                    Self {
                        memory,
                        palette: None,
                        ..self
                    },
                )
            }),
        }
    }
}

fn allocate_record<'memory>(
    pool: &mut Pool<'memory>,
    slot: MemorySlot,
    bytes: usize,
) -> Result<(Resource<'memory>, u64), Error> {
    let memory = pool.resource(Region::Main, bytes)?;
    match slot.encode(memory.as_ptr().addr() as u64, bytes as u64) {
        Ok(physical) => Ok((memory, physical)),
        Err(error) => {
            pool.release(memory).map_err(|(error, _)| error)?;
            Err(Error::Address(error))
        }
    }
}

pub(crate) fn encode(
    width: u32,
    height: u32,
    pixels: &[u8],
    alpha: bool,
    physical: u64,
    output: &mut [u8],
) -> Result<(), Error> {
    let format = if alpha {
        Format::Bgra32
    } else {
        Format::Bgrx32
    };
    let bitmap = Bitmap::new(width, height, format, pixels).ok_or(Error::InvalidSize)?;
    encode_bitmap(bitmap, physical, 0, output)
}

fn encode_bitmap(
    bitmap: Bitmap<'_>,
    physical: u64,
    palette: u64,
    output: &mut [u8],
) -> Result<(), Error> {
    let bytes = HEADER_BYTES
        .checked_add(bitmap.pixels.len())
        .ok_or(Error::InvalidSize)?;
    let output = output.get_mut(..bytes).ok_or(Error::InvalidSize)?;
    let data = physical
        .checked_add(HEADER_BYTES as u64)
        .ok_or(Error::InvalidSize)?;
    output[..HEADER_BYTES].fill(0);
    output[..8].copy_from_slice(&3u64.to_le_bytes());
    output[10..14].copy_from_slice(&bitmap.width.to_le_bytes());
    output[14..18].copy_from_slice(&bitmap.height.to_le_bytes());
    output[18] = bitmap.format as u8;
    output[19] = 5;
    output[20..24].copy_from_slice(&bitmap.width.to_le_bytes());
    output[24..28].copy_from_slice(&bitmap.height.to_le_bytes());
    output[28..32].copy_from_slice(&bitmap.stride.to_le_bytes());
    output[32..40].copy_from_slice(&palette.to_le_bytes());
    output[40..48].copy_from_slice(&data.to_le_bytes());
    output[48..].copy_from_slice(bitmap.pixels);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_resources::test_utils::{Bytes, HostRing, empty, pool};
    use crate::draw::ImageDraw;
    use crate::ring::{CommandRing, ReleaseRing};
    use crate::slots::SlotLayout;

    fn slot(memory: &mut [u64]) -> MemorySlot {
        SlotLayout::new(8, 8)
            .unwrap()
            .slot(
                1,
                4,
                0xe0000000,
                memory.as_ptr().addr() as u64,
                core::mem::size_of_val(memory) as u64,
            )
            .unwrap()
    }

    #[test]
    fn compressed_image_has_one_retained_chunk_and_never_aliases_raw_cache_entries() {
        let header = crate::quic::Header {
            format: crate::quic::Format::Bgrx32,
            width: 32,
            height: 8,
        };
        let pixels = [0x37; 32 * 8 * 4];
        let mut stream = [0; 2048];
        let mut correlations = [0; 33 * 3];
        let mut models = core::array::from_fn(|_| crate::quic::Model::new());
        let length =
            crate::quic::encode(header, &pixels, &mut stream, &mut correlations, &mut models)
                .unwrap();
        let mut memory = [0u64; 1024];
        let slot = slot(&mut memory);
        let mut pool = pool(&mut memory, None);
        let image = Image::allocate_quic(&mut pool, slot, header, &stream[..length]).unwrap();
        assert_eq!(image.bytes(), 42 + length);
        assert!(image.bytes() < pixels.len());
        assert!(!image.matches(32, 8, &pixels, false));
        // SAFETY: the live image retains its complete immutable allocation.
        let bytes = unsafe { core::slice::from_raw_parts(image.memory.as_ptr(), image.bytes()) };
        assert_eq!(bytes[8], 1);
        assert_eq!(&bytes[18..22], &(length as u32).to_le_bytes());
        assert_eq!(&bytes[22..26], &(length as u32).to_le_bytes());
        assert_eq!(&bytes[26..42], &[0; 16]);
        assert_eq!(&bytes[42..], &stream[..length]);
        let retained = image.retain(&mut pool).unwrap();
        image.release(&mut pool).unwrap();
        assert!(!pool.is_empty());
        retained.release(&mut pool).unwrap();
        empty(&mut pool);
        stream[8] ^= 1;
        assert!(matches!(
            Image::allocate_quic(&mut pool, slot, header, &stream[..length]),
            Err(Error::InvalidSize)
        ));
        empty(&mut pool);
    }

    #[test]
    fn image_remains_live_until_its_draw_is_released() {
        let mut memory = [0u64; 1024];
        let slot = slot(&mut memory);
        let mut pool = pool(&mut memory, None);
        let mut command_bytes = Bytes([0; 532]);
        let host = HostRing::new(&mut command_bytes, 32, 16);
        let mut release_bytes = Bytes([0; 84]);
        let mut release_host = HostRing::new(&mut release_bytes, 8, 8);
        // SAFETY: independent bounded mappings each have one guest endpoint.
        let (mut commands, mut releases) = unsafe {
            (
                CommandRing::new(host.base, 532).unwrap(),
                ReleaseRing::new(release_host.base, 84).unwrap(),
            )
        };
        let pixels = [1, 2, 3, 4, 5, 6, 7, 8];
        let image = Image::allocate(&mut pool, slot, 2, 1, &pixels, true).unwrap();
        let pointer = image.memory.as_ptr();
        let address = image.address();
        let draw = ImageDraw::copy([3, 5, 5, 6], [0, 0, 2, 1], 0xcc).unwrap();
        // SAFETY: the simulated primary, image and mapped endpoints remain live.
        unsafe { draw.submit(&image, &mut pool, &mut commands, 7) }.unwrap();
        image.release(&mut pool).unwrap();
        assert!(!pool.is_empty());
        // SAFETY: the pending draw retains this complete immutable image payload.
        let encoded = unsafe { core::slice::from_raw_parts(pointer, 56) };
        assert_eq!(&encoded[18..20], &[9, 5]);
        assert_eq!(&encoded[48..], &pixels);
        assert_eq!(
            u64::from_le_bytes(encoded[40..48].try_into().unwrap()),
            address + 48
        );
        let output = host.command(&pool, 0);
        // SAFETY: the submitted ImageDraw has a complete 167-byte payload.
        let command = unsafe { output.snapshot::<167>() };
        assert_eq!(command[13], 3);
        assert_eq!(
            u64::from_le_bytes(command[123..131].try_into().unwrap()),
            address
        );
        assert_eq!(&command[147..150], &[8, 0, 1]);
        output.complete(0);
        release_host.release(output.id);
        // SAFETY: the simulated host stopped all image and command reads.
        assert_eq!(unsafe { pool.reap(&mut releases, 1) }.unwrap().outputs, 1);
        empty(&mut pool);
    }

    #[test]
    fn rejected_image_and_full_ring_return_all_unpublished_memory() {
        let mut memory = [0u64; 512];
        let slot = slot(&mut memory);
        let mut pool = pool(&mut memory, None);
        assert!(matches!(
            Image::allocate(&mut pool, slot, 2, 1, &[0; 7], false),
            Err(Error::InvalidSize)
        ));
        empty(&mut pool);
        let image = Image::allocate(&mut pool, slot, 2, 1, &[0; 8], false).unwrap();
        let mut bytes = Bytes([0; 532]);
        let host = HostRing::new(&mut bytes, 32, 16);
        host.set(4, 32);
        // SAFETY: the live ring mapping has a single guest endpoint.
        let mut ring = unsafe { CommandRing::new(host.base, 532) }.unwrap();
        let draw = ImageDraw::copy([0, 0, 2, 1], [0, 0, 2, 1], 0xcc).unwrap();
        // SAFETY: the primary and immutable image remain owned during the attempt.
        let result = unsafe { draw.submit(&image, &mut pool, &mut ring, 0) };
        assert_eq!(result, Err(Error::RingFull));
        image.release(&mut pool).unwrap();
        empty(&mut pool);
    }

    #[test]
    fn indexed_image_and_palette_survive_independent_owner_release_until_completion() {
        let mut backing = [0u64; 512];
        let slot = slot(&mut backing);
        let mut pool = pool(&mut backing, None);
        let palette = Palette::allocate(
            &mut pool,
            slot,
            crate::palette::Colors::new(7, &[0x112233, 0xaabbcc]).unwrap(),
        )
        .unwrap();
        let palette_pointer = palette.resource().as_ptr();
        let palette_address = palette.address();
        let bitmap = Bitmap::new(9, 1, Format::Indexed1, &[0xaa, 0x80]).unwrap();
        let stale = SlotLayout::new(8, 8)
            .unwrap()
            .slot(
                1,
                5,
                0xe0000000,
                slot.virtual_range().0,
                slot.virtual_range().1 - slot.virtual_range().0,
            )
            .unwrap();
        assert!(matches!(
            Image::allocate_bitmap(&mut pool, stale, bitmap, Some(&palette)),
            Err(Error::Address(crate::slots::SlotError::WrongSlot))
        ));
        let image = Image::allocate_bitmap(&mut pool, slot, bitmap, Some(&palette)).unwrap();
        let image_pointer = image.resource().as_ptr();
        let mut commands_memory = Bytes([0; 532]);
        let host = HostRing::new(&mut commands_memory, 32, 16);
        let mut release_memory = Bytes([0; 84]);
        let mut release_host = HostRing::new(&mut release_memory, 8, 8);
        // SAFETY: the independent fake hosts retain both complete ring mappings.
        let (mut commands, mut releases) = unsafe {
            (
                CommandRing::new(host.base, 532).unwrap(),
                ReleaseRing::new(release_host.base, 84).unwrap(),
            )
        };
        let draw = ImageDraw::copy([0, 0, 9, 1], [0, 0, 9, 1], 0xcc).unwrap();
        host.set(4, 32);
        // SAFETY: the complete full ring and immutable source owners remain live.
        let full = unsafe { draw.submit(&image, &mut pool, &mut commands, 0) };
        assert_eq!(full, Err(Error::RingFull));
        host.set(4, 0);
        // SAFETY: the fake primary, image and palette stay live during publication.
        unsafe { draw.submit(&image, &mut pool, &mut commands, 0) }.unwrap();
        image.release(&mut pool).unwrap();
        palette.release(&mut pool).unwrap();
        // SAFETY: the pending command independently retains both entire payloads.
        let (header, colors) = unsafe {
            (
                core::slice::from_raw_parts(image_pointer, 50),
                core::slice::from_raw_parts(palette_pointer, 18),
            )
        };
        assert_eq!(header[18], 2);
        assert_eq!(&header[28..32], &2u32.to_le_bytes());
        assert_eq!(&header[32..40], &palette_address.to_le_bytes());
        assert_eq!(&header[48..], &[0xaa, 0x80]);
        assert_eq!(&colors[..10], &[7, 0, 0, 0, 0, 0, 0, 0, 2, 0]);
        assert_eq!(&colors[10..], &[0x33, 0x22, 0x11, 0, 0xcc, 0xbb, 0xaa, 0]);
        let output = host.command(&pool, 0);
        output.complete(0);
        release_host.release(output.id);
        // SAFETY: the fake host stopped all image and palette reads before release.
        assert_eq!(unsafe { pool.reap(&mut releases, 1) }.unwrap().outputs, 1);
        empty(&mut pool);
    }
}
