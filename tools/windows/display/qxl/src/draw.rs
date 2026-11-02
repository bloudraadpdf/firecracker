// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Drawable headers and solid-brush operations from display/res.c and rop.c.

use crate::display_resources::{Command, Error, Pool};
use crate::ring::CommandRing;
use crate::rop::{Method, Rop3};

const HEADER_BYTES: usize = 123;
const FILL_BYTES: usize = HEADER_BYTES + 39;

#[derive(Clone, Copy, Debug)]
pub struct Solid {
    bounds: [i32; 4],
    color: u32,
    operation: Rop3,
}

impl Solid {
    pub fn new(bounds: [i32; 4], color: u32, truth: u8) -> Option<Self> {
        let [left, top, right, bottom] = bounds;
        let operation = Rop3::new(truth);
        if left < 0 || top < 0 || left >= right || top >= bottom || operation.inputs().source {
            return None;
        }
        Some(Self {
            bounds,
            color,
            operation,
        })
    }

    fn encode(self, clock: u32, bytes: &mut [u8]) -> Result<(), Error> {
        let bytes = bytes.get_mut(..FILL_BYTES).ok_or(Error::InvalidSize)?;
        header(bytes, self.bounds, clock, self.operation.effect() as u8, 1);
        let descriptor = match self.operation.method() {
            Method::Fill(descriptor) => descriptor,
            Method::Blackness => 128,
            Method::Whiteness => 256,
            Method::Invert => 512,
            Method::Nop => return Ok(()),
            _ => return Err(Error::InvalidSize),
        };
        bytes[123..127].copy_from_slice(&1u32.to_le_bytes());
        bytes[127..131].copy_from_slice(&self.color.to_le_bytes());
        bytes[143..145].copy_from_slice(&descriptor.to_le_bytes());
        Ok(())
    }

    /// # Safety
    /// Retain the active primary and the pool/ring generation through host release.
    /// Serialise CPU drawing and commands. Bounds must lie within that primary.
    pub unsafe fn submit(
        self,
        pool: &mut Pool<'_>,
        ring: &mut CommandRing,
        clock: u32,
    ) -> Result<bool, Error> {
        if self.operation.method() == Method::Nop {
            return Ok(false);
        }
        // SAFETY: the caller supplies the active primary and endpoint contract.
        unsafe { submit(pool, ring, FILL_BYTES, |bytes| self.encode(clock, bytes)) }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CopyBits {
    bounds: [i32; 4],
    source: [i32; 2],
}

impl CopyBits {
    pub fn new(bounds: [i32; 4], source: [i32; 2]) -> Option<Self> {
        let [left, top, right, bottom] = bounds;
        let [x, y] = source;
        if left < 0 || top < 0 || left >= right || top >= bottom || x < 0 || y < 0 {
            return None;
        }
        x.checked_add(right - left)?;
        y.checked_add(bottom - top)?;
        Some(Self { bounds, source })
    }

    fn encode(self, clock: u32, bytes: &mut [u8]) -> Result<(), Error> {
        let bytes = bytes
            .get_mut(..HEADER_BYTES + 8)
            .ok_or(Error::InvalidSize)?;
        header(bytes, self.bounds, clock, 1, 4);
        bytes[63..67].fill(0);
        let [left, top, right, bottom] = self.bounds;
        let [x, y] = self.source;
        rect(bytes, 75, [x, y, x + (right - left), y + (bottom - top)]);
        bytes[123..127].copy_from_slice(&x.to_le_bytes());
        bytes[127..131].copy_from_slice(&y.to_le_bytes());
        Ok(())
    }

    /// # Safety
    /// Keep both primary regions and the same pool/ring generation live through
    /// host completion. Exclude CPU drawing; source and destination may overlap.
    pub unsafe fn submit(
        self,
        pool: &mut Pool<'_>,
        ring: &mut CommandRing,
        clock: u32,
    ) -> Result<bool, Error> {
        // SAFETY: the caller retains the primary and exclusive mapped endpoints.
        unsafe {
            submit(pool, ring, HEADER_BYTES + 8, |bytes| {
                self.encode(clock, bytes)
            })
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ImageOperation {
    Copy(Rop3),
    Transparent(u32),
    Alpha(u8),
}

#[derive(Clone, Copy, Debug)]
pub struct ImageDraw {
    bounds: [i32; 4],
    source: [i32; 4],
    operation: ImageOperation,
}

impl ImageDraw {
    fn new(bounds: [i32; 4], source: [i32; 4], operation: ImageOperation) -> Option<Self> {
        for [left, top, right, bottom] in [bounds, source] {
            if left < 0 || top < 0 || left >= right || top >= bottom {
                return None;
            }
        }
        Some(Self {
            bounds,
            source,
            operation,
        })
    }

    pub fn copy(bounds: [i32; 4], source: [i32; 4], truth: u8) -> Option<Self> {
        let operation = Rop3::new(truth);
        if !matches!(operation.method(), Method::Copy(_) | Method::Blend(_)) {
            return None;
        }
        Self::new(bounds, source, ImageOperation::Copy(operation))
    }

    pub fn transparent(bounds: [i32; 4], source: [i32; 4], color: u32) -> Option<Self> {
        Self::new(bounds, source, ImageOperation::Transparent(color))
    }

    pub fn alpha(bounds: [i32; 4], source: [i32; 4], constant: u8) -> Option<Self> {
        Self::new(bounds, source, ImageOperation::Alpha(constant))
    }

    pub fn bounds(self) -> [i32; 4] {
        self.bounds
    }

    fn payload_bytes(self) -> usize {
        match self.operation {
            ImageOperation::Copy(_) => 44,
            ImageOperation::Transparent(_) => 32,
            ImageOperation::Alpha(_) => 27,
        }
    }

    fn encode(self, image: u64, clock: u32, bytes: &mut [u8]) -> Result<(), Error> {
        let bytes = bytes
            .get_mut(..HEADER_BYTES + self.payload_bytes())
            .ok_or(Error::InvalidSize)?;
        let (kind, effect) = match self.operation {
            ImageOperation::Copy(operation) => (
                if operation.inputs().destination { 5 } else { 3 },
                operation.effect() as u8,
            ),
            ImageOperation::Transparent(_) => (12, 0),
            ImageOperation::Alpha(_) => (13, 0),
        };
        header(bytes, self.bounds, clock, effect, kind);
        match self.operation {
            ImageOperation::Alpha(constant) => {
                bytes[125] = constant;
                bytes[126..134].copy_from_slice(&image.to_le_bytes());
                rect(bytes, 134, self.source);
            }
            operation => {
                bytes[123..131].copy_from_slice(&image.to_le_bytes());
                rect(bytes, 131, self.source);
                match operation {
                    ImageOperation::Copy(operation) => {
                        let descriptor = match operation.method() {
                            Method::Copy(descriptor) | Method::Blend(descriptor) => descriptor,
                            _ => return Err(Error::InvalidSize),
                        };
                        bytes[147..149].copy_from_slice(&descriptor.to_le_bytes());
                        bytes[149] = 1;
                    }
                    ImageOperation::Transparent(color) => {
                        bytes[147..151].copy_from_slice(&color.to_le_bytes());
                        bytes[151..155].copy_from_slice(&color.to_le_bytes());
                    }
                    ImageOperation::Alpha(_) => unreachable!(),
                }
            }
        }
        Ok(())
    }

    /// # Safety
    /// Retain the active destination and same pool/ring generation until host
    /// release. The image remains immutable; source and destination must fit.
    pub unsafe fn submit<'memory>(
        self,
        image: &crate::image::Image<'memory>,
        pool: &mut Pool<'memory>,
        ring: &mut CommandRing,
        clock: u32,
    ) -> Result<bool, Error> {
        let (width, height) = image.dimensions();
        if self.source[2] as u32 > width || self.source[3] as u32 > height {
            return Err(Error::InvalidSize);
        }
        // SAFETY: source bounds were checked and the caller retains the endpoints.
        unsafe {
            submit_image(
                image,
                pool,
                ring,
                HEADER_BYTES + self.payload_bytes(),
                |bytes| self.encode(image.address(), clock, bytes),
            )
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PatternFill {
    solid: Solid,
    origin: [i32; 2],
}

impl PatternFill {
    pub fn new(bounds: [i32; 4], origin: [i32; 2], truth: u8) -> Option<Self> {
        Some(Self {
            solid: Solid::new(bounds, 0, truth)?,
            origin,
        })
    }

    pub fn bounds(self) -> [i32; 4] {
        self.solid.bounds
    }

    fn encode(self, image: u64, clock: u32, bytes: &mut [u8]) -> Result<(), Error> {
        self.solid.encode(clock, bytes)?;
        bytes[123..127].copy_from_slice(&2u32.to_le_bytes());
        bytes[127..135].copy_from_slice(&image.to_le_bytes());
        bytes[135..139].copy_from_slice(&self.origin[0].to_le_bytes());
        bytes[139..143].copy_from_slice(&self.origin[1].to_le_bytes());
        Ok(())
    }

    /// # Safety
    /// Retain the destination, immutable pattern and exclusive pool/ring generation
    /// until host completion. Exclude concurrent CPU rendering.
    pub unsafe fn submit<'memory>(
        self,
        image: &crate::image::Image<'memory>,
        pool: &mut Pool<'memory>,
        ring: &mut CommandRing,
        clock: u32,
    ) -> Result<bool, Error> {
        if self.solid.operation.method() == Method::Nop {
            return Ok(false);
        }
        // SAFETY: the caller retains the primary and immutable image ownership.
        unsafe {
            submit_image(image, pool, ring, FILL_BYTES, |bytes| {
                self.encode(image.address(), clock, bytes)
            })
        }
    }
}

unsafe fn submit_image<'memory>(
    image: &crate::image::Image<'memory>,
    pool: &mut Pool<'memory>,
    ring: &mut CommandRing,
    length: usize,
    encode: impl FnOnce(&mut [u8]) -> Result<(), Error>,
) -> Result<bool, Error> {
    let mut output = pool.output(Command::Draw, length)?;
    if let Err(error) = pool.payload(&mut output).and_then(encode) {
        pool.cancel(output).map_err(|(error, _)| error)?;
        return Err(error);
    }
    // SAFETY: the output retains its immutable source through host release.
    unsafe {
        match image.palette_resource() {
            Some(palette) => pool.submit_retained(output, ring, &[image.resource(), palette]),
            None => pool.submit_retained(output, ring, &[image.resource()]),
        }
    }
}

pub(crate) fn rect(bytes: &mut [u8], offset: usize, [left, top, right, bottom]: [i32; 4]) {
    for (index, value) in [top, left, bottom, right].into_iter().enumerate() {
        let offset = offset + index * 4;
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}

pub(crate) fn header(bytes: &mut [u8], bounds: [i32; 4], clock: u32, effect: u8, kind: u8) {
    bytes.fill(0);
    bytes[12] = effect;
    bytes[13] = kind;
    rect(bytes, 31, bounds);
    bytes[59..63].copy_from_slice(&clock.to_le_bytes());
    bytes[63..75].fill(0xff);
}

unsafe fn submit(
    pool: &mut Pool<'_>,
    ring: &mut CommandRing,
    length: usize,
    encode: impl FnOnce(&mut [u8]) -> Result<(), Error>,
) -> Result<bool, Error> {
    let mut output = pool.output(Command::Draw, length)?;
    if let Err(error) = pool.payload(&mut output).and_then(encode) {
        pool.cancel(output).map_err(|(error, _)| error)?;
        return Err(error);
    }
    // SAFETY: the caller retains the active primary and exclusive endpoint.
    match unsafe { pool.submit(output, ring) } {
        Ok(notify) => Ok(notify),
        Err((error, output)) => {
            pool.cancel(output).map_err(|(error, _)| error)?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_fill_preserves_signed_origin_and_immutable_image_address() {
        let draw = PatternFill::new([3, 5, 17, 19], [-3, 5], 0x5a).unwrap();
        let mut bytes = [0xa5; FILL_BYTES];
        draw.encode(0x0104000000001000, 7, &mut bytes).unwrap();
        assert_eq!(bytes[13], 1);
        assert_eq!(&bytes[123..127], &[2, 0, 0, 0]);
        assert_eq!(
            u64::from_le_bytes(bytes[127..135].try_into().unwrap()),
            0x0104000000001000
        );
        assert_eq!(i32::from_le_bytes(bytes[135..139].try_into().unwrap()), -3);
        assert_eq!(i32::from_le_bytes(bytes[139..143].try_into().unwrap()), 5);
        assert_eq!(&bytes[143..145], &[64, 0]);
        assert!(PatternFill::new([3, 5, 17, 19], [0, 0], 0xcc).is_none());
    }

    #[test]
    fn transparent_and_alpha_records_match_their_packed_payloads() {
        let bounds = [3, 5, 17, 19];
        let source = [0, 0, 14, 14];
        let address = 0x0104000000001000;
        let mut bytes = [0xa5; 180];
        ImageDraw::transparent(bounds, source, 0x336699)
            .unwrap()
            .encode(address, 7, &mut bytes)
            .unwrap();
        assert_eq!(&bytes[12..15], &[0, 12, 0]);
        assert_eq!(
            u64::from_le_bytes(bytes[123..131].try_into().unwrap()),
            address
        );
        assert_eq!(
            &bytes[147..155],
            &[0x99, 0x66, 0x33, 0, 0x99, 0x66, 0x33, 0]
        );
        assert!(bytes[155..].iter().all(|value| *value == 0xa5));
        bytes.fill(0xa5);
        ImageDraw::alpha(bounds, source, 83)
            .unwrap()
            .encode(address, 7, &mut bytes)
            .unwrap();
        assert_eq!(&bytes[12..15], &[0, 13, 0]);
        assert_eq!(&bytes[123..126], &[0, 0, 83]);
        assert_eq!(
            u64::from_le_bytes(bytes[126..134].try_into().unwrap()),
            address
        );
        assert_eq!(
            &bytes[134..150],
            &[0, 0, 0, 0, 0, 0, 0, 0, 14, 0, 0, 0, 14, 0, 0, 0]
        );
        assert!(bytes[150..].iter().all(|value| *value == 0xa5));
    }

    #[test]
    fn full_ring_cancels_unpublished_draw_without_leaking_pool_space() {
        use crate::display_resources::test_utils::{Bytes, HostRing, empty, pool};
        let mut backing = [0u64; 512];
        let mut pool = pool(&mut backing, None);
        let mut bytes = Bytes([0; 532]);
        let host = HostRing::new(&mut bytes, 32, 16);
        host.set(4, 32);
        // SAFETY: the simulated host retains this bounded ring and pool mapping.
        let mut ring = unsafe { CommandRing::new(host.base, 532) }.unwrap();
        let copy = CopyBits::new([0, 0, 8, 8], [1, 1]).unwrap();
        // SAFETY: the fake active primary and exclusively owned mapping stay live.
        let result = unsafe { copy.submit(&mut pool, &mut ring, 0) };
        assert_eq!(result, Err(Error::RingFull));
        assert_eq!(host.get(4), 32);
        empty(&mut pool);
        let nop = Solid::new([0, 0, 8, 8], 0, 0xaa).unwrap();
        // SAFETY: a NOP accesses no host data and retains the same endpoint contract.
        assert_eq!(unsafe { nop.submit(&mut pool, &mut ring, 0) }, Ok(false));
        assert_eq!(host.get(4), 32);
        empty(&mut pool);
    }

    #[test]
    fn solid_wire_payload_preserves_qxl_field_order_and_dependencies() {
        let draw = Solid::new([3, 5, 17, 19], 0x123456, 0xf0).unwrap();
        let mut bytes = [0xa5; FILL_BYTES];
        draw.encode(0x10203040, &mut bytes).unwrap();
        assert_eq!(&bytes[12..15], &[1, 1, 0]);
        assert_eq!(
            &bytes[31..47],
            &[5, 0, 0, 0, 3, 0, 0, 0, 19, 0, 0, 0, 17, 0, 0, 0]
        );
        assert!(bytes[47..59].iter().all(|byte| *byte == 0));
        assert_eq!(&bytes[59..63], &[0x40, 0x30, 0x20, 0x10]);
        assert_eq!(&bytes[63..75], &[0xff; 12]);
        assert_eq!(&bytes[123..131], &[1, 0, 0, 0, 0x56, 0x34, 0x12, 0]);
        assert_eq!(&bytes[143..145], &[8, 0]);
        assert!(bytes[145..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn copy_bits_encodes_overlapping_source_and_dependency() {
        let copy = CopyBits::new([3, 5, 17, 19], [1, 2]).unwrap();
        let mut bytes = [0xa5; HEADER_BYTES + 8];
        copy.encode(7, &mut bytes).unwrap();
        assert_eq!(&bytes[12..15], &[1, 4, 0]);
        assert_eq!(
            &bytes[63..75],
            &[0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255]
        );
        assert_eq!(
            &bytes[75..91],
            &[2, 0, 0, 0, 1, 0, 0, 0, 16, 0, 0, 0, 15, 0, 0, 0]
        );
        assert_eq!(&bytes[123..], &[1, 0, 0, 0, 2, 0, 0, 0]);
        assert!(CopyBits::new([0, 0, 2, 2], [i32::MAX, 0]).is_none());
    }

    #[test]
    fn solid_rejects_sources_and_invalid_bounds() {
        assert!(Solid::new([0, 0, 1, 1], 0, 0xcc).is_none());
        assert!(Solid::new([-1, 0, 1, 1], 0, 0xf0).is_none());
        assert!(Solid::new([0, 0, 0, 1], 0, 0xf0).is_none());
        for truth in [0, 0x55, 0xaa, 0xff, 0xf0, 0x5a] {
            let draw = Solid::new([0, 0, 1, 1], 0, truth).unwrap();
            draw.encode(0, &mut [0; FILL_BYTES]).unwrap();
        }
    }
}
