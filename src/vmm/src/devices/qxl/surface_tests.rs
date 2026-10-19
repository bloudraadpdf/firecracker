// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::bitmap::Bitmap;
use super::memory::{Memory, Region};
use super::protocol::{PrimarySurface, Rect};
use super::surface::Surface;
use super::test_utils::memory as setup;
use super::transport::AddressSpace;

const POINTER: u64 = 0x0101_0000_0000_0040;

fn primary(stride: i32) -> PrimarySurface {
    let mut data = [0; 40];
    data[..4].copy_from_slice(&3u32.to_le_bytes());
    data[4..8].copy_from_slice(&3u32.to_le_bytes());
    data[8..12].copy_from_slice(&stride.to_le_bytes());
    data[12..16].copy_from_slice(&32u32.to_le_bytes());
    data[32..40].copy_from_slice(&POINTER.to_le_bytes());
    PrimarySurface::decode(&data).unwrap()
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
    let data: Vec<u8> = [top, left, bottom, right]
        .into_iter()
        .flat_map(i32::to_le_bytes)
        .collect();
    Rect::decode(&data).unwrap()
}

fn seed(memory: &Memory, stride: i32) {
    memory.write(Region::Ram, 63, &[0xa5; 50]).unwrap();
    for y in 0..3 {
        let physical_y = if stride < 0 { 2 - y } else { y };
        for x in 0..3 {
            let pixel = 0xff00_0000u32 | u32::try_from(y * 3 + x + 1).unwrap();
            memory
                .write(
                    Region::Ram,
                    64 + physical_y * 16 + x * 4,
                    &pixel.to_le_bytes(),
                )
                .unwrap();
        }
    }
}

fn assert_padding(memory: &Memory) {
    for offset in [63, 76, 92, 108, 112] {
        let mut byte = [0];
        memory.read(Region::Ram, offset, &mut byte).unwrap();
        assert_eq!(byte, [0xa5]);
    }
}

fn bitmap_fixture() -> (Memory, AddressSpace, Bitmap) {
    let (memory, addresses) = setup();
    seed(&memory, 16);
    let bitmap = bitmap(&memory, &addresses);
    (memory, addresses, bitmap)
}

fn bitmap(memory: &Memory, addresses: &AddressSpace) -> Bitmap {
    let mut header = [0; 48];
    for offset in [10, 14, 20, 24] {
        header[offset..offset + 4].copy_from_slice(&2u32.to_le_bytes());
    }
    header[18] = 9;
    header[19] = 5;
    header[28..32].copy_from_slice(&8u32.to_le_bytes());
    header[40..48].copy_from_slice(&0x0101_0000_0000_0400u64.to_le_bytes());
    memory.write(Region::Ram, 256, &header).unwrap();
    let pixels: Vec<u8> = [0x1100_0001u32, 0x2200_0002, 0x3300_0003, 0x4400_0004]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    memory.write(Region::Ram, 1024, &pixels).unwrap();
    Bitmap::decode(memory, addresses, 0x0101_0000_0000_0100).unwrap()
}

#[test]
fn scaled_bitmap_copies_use_unclipped_coordinates_and_preserve_padding() {
    for stride in [-16, 16] {
        let (memory, addresses) = setup();
        seed(&memory, stride);
        let bitmap = bitmap(&memory, &addresses);
        let surface = Surface::open(&memory, &addresses, primary(stride)).unwrap();
        surface
            .blit(&bitmap, rect(0, 0, 2, 2), rect(-1, -1, 3, 3), 8)
            .unwrap();
        assert_eq!(surface.frame().unwrap().pixels, [1, 2, 2, 3, 4, 4, 3, 4, 4]);
        assert_padding(&memory);
        let mut pixel = [0; 4];
        let logical_top = if stride < 0 { 96 } else { 64 };
        memory.read(Region::Ram, logical_top, &mut pixel).unwrap();
        assert_eq!(u32::from_le_bytes(pixel), 0x1100_0001);
        surface
            .blit(&bitmap, rect(1, 0, 2, 2), rect(0, 0, 3, 3), 8)
            .unwrap();
        assert_eq!(surface.frame().unwrap().pixels, [2, 2, 2, 2, 2, 2, 4, 4, 4]);
        surface
            .blit(
                &bitmap,
                rect(0, 0, 2, 2),
                rect(i32::MIN, i32::MIN, i32::MAX, i32::MAX),
                8,
            )
            .unwrap();
        assert_eq!(surface.frame().unwrap().pixels, [4; 9]);
    }
}

#[test]
fn raster_fills_and_copies_use_the_existing_destination() {
    let (memory, addresses, bitmap) = bitmap_fixture();
    let surface = Surface::open(&memory, &addresses, primary(16)).unwrap();
    surface.raster_fill(rect(1, 0, 3, 2), 0x123456, 64).unwrap();
    assert_eq!(
        surface.frame().unwrap().pixels,
        [
            1,
            2 ^ 0x123456,
            3 ^ 0x123456,
            4,
            5 ^ 0x123456,
            6 ^ 0x123456,
            7,
            8,
            9
        ]
    );
    surface
        .blit(&bitmap, rect(0, 0, 2, 2), rect(0, 0, 2, 2), 64)
        .unwrap();
    assert_eq!(
        surface.frame().unwrap().pixels,
        [
            0,
            0x123456,
            3 ^ 0x123456,
            7,
            1 ^ 0x123456,
            6 ^ 0x123456,
            7,
            8,
            9
        ]
    );
}

#[test]
fn malformed_blits_and_raster_descriptors_leave_the_destination_unchanged() {
    let (memory, addresses, bitmap) = bitmap_fixture();
    let surface = Surface::open(&memory, &addresses, primary(16)).unwrap();
    for source in [
        rect(-1, 0, 2, 2),
        rect(0, -1, 2, 2),
        rect(0, 0, 3, 2),
        rect(0, 0, 2, 3),
        rect(0, 0, 0, 2),
    ] {
        surface
            .blit(&bitmap, source, rect(0, 0, 3, 3), 8)
            .unwrap_err();
    }
    for descriptor in [0, 24, 0x808] {
        surface
            .blit(&bitmap, rect(0, 0, 2, 2), rect(0, 0, 3, 3), descriptor)
            .unwrap_err();
        surface
            .raster_fill(rect(0, 0, 3, 3), 0, descriptor)
            .unwrap_err();
    }
    surface
        .blit(&bitmap, rect(0, 0, 2, 2), rect(0, 0, 0, 3), 8)
        .unwrap();
    assert_eq!(surface.frame().unwrap().pixels, (1..=9).collect::<Vec<_>>());
}

#[test]
fn frames_follow_signed_stride_and_remove_alpha_without_exposing_padding() {
    for stride in [-16, 16] {
        let (memory, addresses) = setup();
        seed(&memory, stride);
        let frame = Surface::open(&memory, &addresses, primary(stride))
            .unwrap()
            .frame()
            .unwrap();
        assert_eq!((frame.width, frame.height), (3, 3));
        assert_eq!(frame.pixels, (1..=9).collect::<Vec<_>>());
    }
}

#[test]
fn clipped_fills_leave_padding_and_neighbouring_memory_unchanged() {
    let (memory, addresses) = setup();
    seed(&memory, -16);
    let surface = Surface::open(&memory, &addresses, primary(-16)).unwrap();
    surface.fill(rect(i32::MIN, -1, 2, 2), 0xff12_3456).unwrap();
    assert_eq!(
        surface.frame().unwrap().pixels,
        [0x123456, 0x123456, 3, 0x123456, 0x123456, 6, 7, 8, 9]
    );
    assert_padding(&memory);
    surface.fill(rect(100, 100, i32::MAX, i32::MAX), 0).unwrap();
    assert_eq!(surface.frame().unwrap().pixels[0], 0x123456);
}

#[test]
fn overlapping_copies_preserve_source_pixels_in_each_direction() {
    for stride in [-16, 16] {
        for (target, source, expected) in [
            (rect(0, 1, 3, 3), (0, 0), [1, 2, 3, 1, 2, 3, 4, 5, 6]),
            (rect(0, 0, 3, 2), (0, 1), [4, 5, 6, 7, 8, 9, 7, 8, 9]),
            (rect(1, 0, 3, 3), (0, 0), [1, 1, 2, 4, 4, 5, 7, 7, 8]),
            (rect(0, 0, 2, 3), (1, 0), [2, 3, 3, 5, 6, 6, 8, 9, 9]),
        ] {
            let (memory, addresses) = setup();
            seed(&memory, stride);
            let surface = Surface::open(&memory, &addresses, primary(stride)).unwrap();
            surface.copy_bits(target, source).unwrap();
            assert_eq!(surface.frame().unwrap().pixels, expected);
        }
    }
}

#[test]
fn clipped_copy_adjusts_the_source_and_rejects_invalid_sources_before_writes() {
    let (memory, addresses) = setup();
    seed(&memory, 16);
    let surface = Surface::open(&memory, &addresses, primary(16)).unwrap();
    surface.copy_bits(rect(-1, -1, 2, 2), (0, 0)).unwrap();
    let expected = [5, 6, 3, 8, 9, 6, 7, 8, 9];
    assert_eq!(surface.frame().unwrap().pixels, expected);
    for source in [(i32::MIN, 0), (0, i32::MAX), (2, 0), (0, 2)] {
        surface.copy_bits(rect(0, 0, 2, 2), source).unwrap_err();
        assert_eq!(surface.frame().unwrap().pixels, expected);
    }
}

#[test]
fn clear_changes_visible_pixels_without_touching_row_padding() {
    let (memory, addresses) = setup();
    seed(&memory, -16);
    let surface = Surface::open(&memory, &addresses, primary(-16)).unwrap();
    surface.clear().unwrap();
    assert_eq!(surface.frame().unwrap().pixels, [0; 9]);
    let mut padding = [0; 4];
    memory.read(Region::Ram, 76, &mut padding).unwrap();
    assert_eq!(padding, [0xa5; 4]);
}

#[test]
fn opening_a_surface_checks_its_entire_extent_and_slot_generation() {
    let (memory, mut addresses) = setup();
    addresses.remove(1).unwrap();
    addresses.add(1, 0x8000_0000..0x8000_1000).unwrap();
    Surface::open(&memory, &addresses, primary(16)).unwrap_err();
    let mut addresses = AddressSpace::new(0x8000_0000, 0x8200_0000).unwrap();
    addresses.add(1, 0x8000_0000..0x8000_0060).unwrap();
    Surface::open(&memory, &addresses, primary(16)).unwrap_err();
}
