// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Owned secondary surfaces from display/surface.c and display/res.c.

use core::marker::PhantomData;
use core::num::NonZeroU32;

use crate::display_resources::{Command, Error, Output, Pool, Region, Resource};
use crate::ring::CommandRing;
use crate::slots::MemorySlot;

const COMMAND_BYTES: usize = 41;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Format {
    Rgb555 = 16,
    Xrgb8888 = 32,
}

impl Format {
    pub fn bitmap_format(self) -> u32 {
        match self {
            Self::Rgb555 => 4,
            Self::Xrgb8888 => 6,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    width: u32,
    height: u32,
    stride: i32,
    bytes: usize,
    format: Format,
}

impl Layout {
    pub fn new(width: u32, height: u32, format: Format) -> Result<Self, Error> {
        let pixel_bytes = match format {
            Format::Rgb555 => 2,
            Format::Xrgb8888 => 4,
        };
        let stride = width
            .checked_mul(pixel_bytes)
            .and_then(|bytes| bytes.checked_add(3))
            .map(|bytes| bytes & !3)
            .ok_or(Error::InvalidSize)?;
        Self::with_stride(width, height, format, stride)
    }

    pub fn with_stride(
        width: u32,
        height: u32,
        format: Format,
        stride: u32,
    ) -> Result<Self, Error> {
        let pixel_bytes = match format {
            Format::Rgb555 => 2,
            Format::Xrgb8888 => 4,
        };
        if width == 0
            || height == 0
            || width > i32::MAX as u32
            || height > i32::MAX as u32
            || !stride.is_multiple_of(4)
            || stride > i32::MAX as u32
            || width
                .checked_mul(pixel_bytes)
                .is_none_or(|minimum| stride < minimum)
        {
            return Err(Error::InvalidSize);
        }
        let bytes = stride
            .checked_mul(height)
            .filter(|bytes| (*bytes as usize) <= isize::MAX as usize)
            .ok_or(Error::InvalidSize)? as usize;
        Ok(Self {
            width,
            height,
            stride: stride as i32,
            bytes,
            format,
        })
    }

    pub fn width(self) -> u32 {
        self.width
    }

    pub fn height(self) -> u32 {
        self.height
    }

    pub fn stride(self) -> i32 {
        self.stride
    }

    pub fn bytes(self) -> usize {
        self.bytes
    }

    pub fn format(self) -> Format {
        self.format
    }

    /// The packed 40-byte QXLSurfaceCreate record used for the primary surface.
    pub fn primary(self, physical: u64) -> [u8; 40] {
        let mut record = [0; 40];
        record[..4].copy_from_slice(&self.width.to_le_bytes());
        record[4..8].copy_from_slice(&self.height.to_le_bytes());
        record[8..12].copy_from_slice(&(-self.stride).to_le_bytes());
        record[12..16].copy_from_slice(&(self.format as u32).to_le_bytes());
        record[32..].copy_from_slice(&physical.to_le_bytes());
        record
    }
}

#[derive(Debug)]
pub enum Prepared {}
#[derive(Debug)]
pub enum Active {}

#[derive(Debug)]
#[must_use]
pub struct Surface<'memory, State> {
    id: NonZeroU32,
    layout: Layout,
    physical: u64,
    memory: Resource<'memory>,
    state: PhantomData<State>,
}

impl<'memory, State> Surface<'memory, State> {
    pub fn id(&self) -> u32 {
        self.id.get()
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    pub fn physical(&self) -> u64 {
        self.physical
    }

    pub fn as_ptr(&self) -> *mut u8 {
        self.memory.as_ptr()
    }

    fn release(self, pool: &mut Pool<'memory>) -> Result<(), (Error, Self)> {
        let Self {
            id,
            layout,
            physical,
            memory,
            state,
        } = self;
        pool.release(memory).map_err(|(error, memory)| {
            (
                error,
                Self {
                    id,
                    layout,
                    physical,
                    memory,
                    state,
                },
            )
        })
    }

    fn change_state<Next>(self) -> Surface<'memory, Next> {
        Surface {
            id: self.id,
            layout: self.layout,
            physical: self.physical,
            memory: self.memory,
            state: PhantomData,
        }
    }
}

impl<'memory> Surface<'memory, Prepared> {
    pub fn allocate(
        pool: &mut Pool<'memory>,
        slot: MemorySlot,
        id: NonZeroU32,
        layout: Layout,
    ) -> Result<Self, Error> {
        let memory = pool.resource(Region::Video, layout.bytes)?;
        let physical = match slot.encode(memory.as_ptr().addr() as u64, memory.len() as u64) {
            Ok(physical) => physical,
            Err(error) => {
                pool.release(memory).expect("surface allocation ownership");
                return Err(Error::Address(error));
            }
        };
        Ok(Self {
            id,
            layout,
            physical,
            memory,
            state: PhantomData,
        })
    }

    /// Only an unpublished surface has an exclusive, safely borrowed pixel slice.
    pub fn pixels(&mut self) -> &mut [u8] {
        // SAFETY: this prepared surface owns the sole reference to its allocation.
        // Publication consumes self, so the slice cannot overlap host access.
        unsafe { core::slice::from_raw_parts_mut(self.memory.as_ptr(), self.memory.len()) }
    }

    pub fn cancel(self, pool: &mut Pool<'memory>) -> Result<(), (Error, Self)> {
        self.release(pool)
    }

    /// # Safety
    /// The pool, video slot and command ring must describe the same active QXL
    /// device/generation. Finish all pixel writes before publication; preserve
    /// the mapping until host completion and eventual surface destruction.
    /// The ID must be unused, or its prior destroy must precede this command
    /// on the same ring. Keep IDs within the device's advertised surface count.
    pub unsafe fn publish(
        self,
        pool: &mut Pool<'memory>,
        ring: &mut CommandRing,
        keep_data: bool,
    ) -> Result<(Surface<'memory, Active>, bool), (Error, Self)> {
        let output = match request(pool, self.id, Some((self.layout, self.physical, keep_data))) {
            Ok(output) => output,
            Err(error) => return Err((error, self)),
        };
        // SAFETY: the caller supplies the active ring and mapping relationship.
        match unsafe { pool.submit_retained(output, ring, &[&self.memory]) } {
            Ok(notify) => Ok((self.change_state(), notify)),
            Err(error) => Err((error, self)),
        }
    }
}

impl<'memory> Surface<'memory, Active> {
    /// # Safety
    /// The device/host must have stopped all surface access, including queued
    /// commands and delayed release writes. Keep mappings live through cleanup.
    /// A command timeout alone does not prove that the surface is unused.
    pub unsafe fn discard(self, pool: &mut Pool<'memory>) -> Result<(), (Error, Self)> {
        self.release(pool)
    }

    /// # Safety
    /// Stop all GDI and guest access to this surface before destruction. Its
    /// create and destroy commands must use the same device/ring generation.
    /// Host ordering must execute creation and drawing before destruction.
    /// Do not reuse this ID until its destroy command has been published.
    pub unsafe fn destroy(
        self,
        pool: &mut Pool<'memory>,
        ring: &mut CommandRing,
    ) -> Result<bool, (Error, Self)> {
        let output = match request(pool, self.id, None) {
            Ok(output) => output,
            Err(error) => return Err((error, self)),
        };
        // SAFETY: the caller retains the device and command ordering contract.
        match unsafe { pool.submit_retained(output, ring, &[&self.memory]) } {
            Ok(notify) => {
                // The destroy command retains its own reference through release.
                pool.release(self.memory)
                    .expect("surface allocation ownership");
                Ok(notify)
            }
            Err(error) => Err((error, self)),
        }
    }
}

fn request<'memory>(
    pool: &mut Pool<'memory>,
    id: NonZeroU32,
    create: Option<(Layout, u64, bool)>,
) -> Result<Output<'memory>, Error> {
    let mut output = pool.output(Command::Surface, COMMAND_BYTES)?;
    let bytes = pool
        .payload(&mut output)
        .expect("surface command ownership");
    bytes[8..12].copy_from_slice(&id.get().to_le_bytes());
    if let Some((layout, physical, keep_data)) = create {
        bytes[13..17].copy_from_slice(&u32::from(keep_data).to_le_bytes());
        for (offset, value) in [
            (17, layout.format as u32),
            (21, layout.width),
            (25, layout.height),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[29..33].copy_from_slice(&(-layout.stride).to_le_bytes());
        bytes[33..41].copy_from_slice(&physical.to_le_bytes());
    } else {
        bytes[12] = 1;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_resources::test_utils::{Bytes, HostRing, empty, pool};
    use crate::ring::ReleaseRing;
    use crate::slots::SlotLayout;

    fn video_slot(video: &mut [u64]) -> MemorySlot {
        SlotLayout::new(8, 8)
            .unwrap()
            .slot(
                2,
                6,
                0xf0000000,
                video.as_mut_ptr().addr() as u64,
                size_of_val(video) as u64,
            )
            .unwrap()
    }

    #[test]
    fn checked_layout_and_primary_record_match_the_packed_protocol() {
        let layout = Layout::new(3, 2, Format::Rgb555).unwrap();
        assert_eq!(
            (
                layout.width(),
                layout.height(),
                layout.stride(),
                layout.bytes()
            ),
            (3, 2, 8, 16)
        );
        let padded = Layout::with_stride(3, 2, Format::Rgb555, 12).unwrap();
        assert_eq!((padded.stride(), padded.bytes()), (12, 24));
        assert_eq!(&padded.primary(0)[8..12], &(-12i32).to_le_bytes());
        assert_eq!(
            Layout::with_stride(3, 2, Format::Rgb555, 10),
            Err(Error::InvalidSize)
        );
        let record = layout.primary(0x0104000000002000);
        assert_eq!(&record[..8], &[3, 0, 0, 0, 2, 0, 0, 0]);
        assert_eq!(&record[8..12], &(-8i32).to_le_bytes());
        assert_eq!(&record[12..16], &16u32.to_le_bytes());
        assert_eq!(&record[16..32], &[0; 16]);
        assert_eq!(&record[32..], &0x0104000000002000u64.to_le_bytes());
        for (width, height) in [
            (0, 1),
            (1, 0),
            (u32::MAX, 1),
            (1, u32::MAX),
            (i32::MAX as u32, 1),
            (70000, 70000),
        ] {
            assert_eq!(
                Layout::new(width, height, Format::Xrgb8888),
                Err(Error::InvalidSize)
            );
        }
    }

    #[test]
    fn surface_memory_lasts_through_create_and_destroy_completion() {
        let mut memory = [0u64; 1024];
        let mut video = [0u64; 16];
        let slot = video_slot(&mut video);
        let mut pool = pool(&mut memory, Some(&mut video));
        let mut command_bytes = Bytes([0; 532]);
        let command_host = HostRing::new(&mut command_bytes, 32, 16);
        let mut release_bytes = Bytes([0; 84]);
        let mut release_host = HostRing::new(&mut release_bytes, 8, 8);
        // SAFETY: both live ring mappings have one guest endpoint.
        let (mut commands, mut releases) = unsafe {
            (
                CommandRing::new(command_host.base, 532).unwrap(),
                ReleaseRing::new(release_host.base, 84).unwrap(),
            )
        };
        let layout = Layout::new(8, 4, Format::Xrgb8888).unwrap();
        let id = NonZeroU32::new(7).unwrap();
        let mut prepared = Surface::allocate(&mut pool, slot, id, layout).unwrap();
        prepared.pixels().fill(0x5a);
        // SAFETY: this pool, video slot and command ring share the active device.
        let (surface, _) = unsafe { prepared.publish(&mut pool, &mut commands, true) }.unwrap();
        let pixel_address = surface.as_ptr();
        assert_eq!(
            (surface.id(), surface.layout(), surface.physical()),
            (7, layout, 0x0206000000000000)
        );
        let create = command_host.command(&pool, 0);
        assert_eq!(create.command, 5);
        // SAFETY: QXLSurfaceCmd has the complete packed 41-byte extent.
        let bytes = unsafe { create.snapshot::<41>() };
        assert_eq!(&bytes[8..13], &[7, 0, 0, 0, 0]);
        assert_eq!(&bytes[13..17], &1u32.to_le_bytes());
        assert_eq!(&bytes[17..29], &[32, 0, 0, 0, 8, 0, 0, 0, 4, 0, 0, 0]);
        assert_eq!(&bytes[29..33], &(-32i32).to_le_bytes());
        assert_eq!(&bytes[33..], &surface.physical().to_le_bytes());
        command_host.set(12, 1);
        create.complete(0);
        release_host.release(create.id);
        // SAFETY: the host completed the create command, retaining the surface.
        assert_eq!(unsafe { pool.reap(&mut releases, 1) }.unwrap().outputs, 1);
        assert!(matches!(
            pool.resource(Region::Video, layout.bytes()),
            Err(Error::OutOfMemory)
        ));
        // SAFETY: all guest users have stopped; destruction follows creation.
        unsafe { surface.destroy(&mut pool, &mut commands) }.unwrap();
        let destroy = command_host.command(&pool, 1);
        // SAFETY: destroy commands use the same complete 41-byte packed record.
        let bytes = unsafe { destroy.snapshot::<41>() };
        assert_eq!(&bytes[8..13], &[7, 0, 0, 0, 1]);
        assert_eq!(&bytes[13..], &[0; 28]);
        assert!(matches!(
            pool.resource(Region::Video, layout.bytes()),
            Err(Error::OutOfMemory)
        ));
        // SAFETY: the pending destroy output still retains the video allocation.
        assert_eq!(unsafe { pixel_address.add(127).read() }, 0x5a);
        command_host.set(12, 2);
        destroy.complete(0);
        release_host.release(destroy.id);
        // SAFETY: the host stopped all access and completed destruction.
        assert_eq!(unsafe { pool.reap(&mut releases, 1) }.unwrap().outputs, 1);
        empty(&mut pool);
    }

    #[test]
    fn failed_create_and_destroy_can_retry_without_losing_pixels_or_ownership() {
        let mut memory = [0u64; 1024];
        let mut video = [0u64; 16];
        let slot = video_slot(&mut video);
        let mut pool = pool(&mut memory, Some(&mut video));
        let mut command_bytes = Bytes([0; 532]);
        let command_host = HostRing::new(&mut command_bytes, 32, 16);
        let mut release_bytes = Bytes([0; 84]);
        let mut release_host = HostRing::new(&mut release_bytes, 8, 8);
        // SAFETY: both ring mappings remain live with exclusive guest endpoints.
        let (mut commands, mut releases) = unsafe {
            (
                CommandRing::new(command_host.base, 532).unwrap(),
                ReleaseRing::new(release_host.base, 84).unwrap(),
            )
        };
        let layout = Layout::new(8, 4, Format::Xrgb8888).unwrap();
        let mut prepared =
            Surface::allocate(&mut pool, slot, NonZeroU32::new(1).unwrap(), layout).unwrap();
        prepared.pixels().fill(0x7b);
        command_host.set(4, 32);
        // SAFETY: the full ring belongs to this active device and generation.
        let (error, mut prepared) =
            unsafe { prepared.publish(&mut pool, &mut commands, true) }.unwrap_err();
        assert_eq!(error, Error::RingFull);
        assert_eq!(prepared.pixels(), &[0x7b; 128]);
        prepared.pixels()[127] = 0x4a;
        command_host.set(12, 32);
        // SAFETY: the host freed ring slots; no prepared pixel borrow remains.
        let (surface, _) = unsafe { prepared.publish(&mut pool, &mut commands, true) }.unwrap();
        let create = command_host.command(&pool, 32);
        command_host.set(12, 33);
        create.complete(0);
        release_host.release(create.id);
        // SAFETY: creation completed and the host retains the surface itself.
        unsafe { pool.reap(&mut releases, 1) }.unwrap();
        command_host.set(4, 65);
        // SAFETY: all guest access ended; the ring is deliberately full again.
        let (error, surface) = unsafe { surface.destroy(&mut pool, &mut commands) }.unwrap_err();
        assert_eq!(error, Error::RingFull);
        // SAFETY: the returned active surface still owns its live pixel range.
        assert_eq!(unsafe { surface.as_ptr().add(127).read() }, 0x4a);
        command_host.set(12, 65);
        // SAFETY: the host freed ring slots, preserving command ordering.
        unsafe { surface.destroy(&mut pool, &mut commands) }.unwrap();
        let destroy = command_host.command(&pool, 65);
        command_host.set(12, 66);
        destroy.complete(0);
        release_host.release(destroy.id);
        // SAFETY: the host finished destruction and stopped surface access.
        unsafe { pool.reap(&mut releases, 1) }.unwrap();
        empty(&mut pool);
    }

    #[test]
    fn invalid_slot_rolls_back_and_wrong_pool_cancellation_returns_the_builder() {
        let mut memory = [0u64; 512];
        let mut video = [0u64; 16];
        let slot = video_slot(&mut video);
        let mut first = pool(&mut memory, Some(&mut video));
        let layout = Layout::new(8, 4, Format::Xrgb8888).unwrap();
        let id = NonZeroU32::new(1).unwrap();
        let wrong = SlotLayout::new(8, 8).unwrap().slot(2, 6, 0, 0, 8).unwrap();
        assert!(matches!(
            Surface::allocate(&mut first, wrong, id, layout),
            Err(Error::Address(_))
        ));
        empty(&mut first);
        let prepared = Surface::allocate(&mut first, slot, id, layout).unwrap();
        let mut other_memory = [0u64; 512];
        let mut second = pool(&mut other_memory, None);
        let (error, prepared) = prepared.cancel(&mut second).unwrap_err();
        assert_eq!(error, Error::WrongPool);
        prepared.cancel(&mut first).unwrap();
        empty(&mut first);
        empty(&mut second);
    }

    #[test]
    fn device_stop_cleanup_handles_a_surface_whose_create_is_still_pending() {
        let mut memory = [0u64; 512];
        let mut video = [0u64; 16];
        let slot = video_slot(&mut video);
        let mut pool = pool(&mut memory, Some(&mut video));
        let mut bytes = Bytes([0; 532]);
        let host = HostRing::new(&mut bytes, 32, 16);
        // SAFETY: the ring mapping remains live with one guest producer.
        let mut ring = unsafe { CommandRing::new(host.base, 532).unwrap() };
        let prepared = Surface::allocate(
            &mut pool,
            slot,
            NonZeroU32::new(1).unwrap(),
            Layout::new(8, 4, Format::Xrgb8888).unwrap(),
        )
        .unwrap();
        // SAFETY: the prepared surface, slot and ring belong to this device.
        let (surface, _) = unsafe { prepared.publish(&mut pool, &mut ring, false) }.unwrap();
        // SAFETY: the simulated device is now stopped, including delayed writes.
        unsafe { pool.discard_pending() };
        assert!(!pool.is_empty());
        // SAFETY: stopped hardware no longer uses this live surface allocation.
        unsafe { surface.discard(&mut pool) }.unwrap();
        empty(&mut pool);
    }
}
