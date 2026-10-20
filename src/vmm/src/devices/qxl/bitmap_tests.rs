// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::bitmap::{Bitmap, ScaleMode, ScaledBitmap};
use super::memory::{Memory, RAM_HEADER, Region};
use super::protocol::Rect;
use super::test_utils::memory as setup;
use super::transport::AddressSpace;

const SLOT: u64 = 0x0101_0000_0000_0000;
const IMAGE: usize = 64;
const DATA: usize = 512;
const PALETTE: usize = 256;

#[test]
fn nearest_uses_gdi_pixel_centres_for_enlargement_and_reduction() {
    let bitmap = Bitmap::from_pixels(24, 20, (0..480).collect()).unwrap();
    let source = Rect::new(0, 0, 24, 20).unwrap();
    let enlarged = ScaledBitmap::new(&bitmap, source, Rect::new(16, 40, 56, 68).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(enlarged.sample(19, 40), 2);
    assert_eq!(enlarged.sample(55, 67), 479);
    let reduced = ScaledBitmap::new(&bitmap, source, Rect::new(80, 40, 97, 53).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(reduced.sample(81, 40), 2);
    assert_eq!(reduced.sample(88, 40), 12);
    assert_eq!(reduced.sample(96, 52), 479);
}

#[test]
fn interpolation_samples_pixel_centres_and_clamps_source_edges() {
    let bitmap = Bitmap::from_pixels(
        2,
        2,
        vec![0xff00_0000, 0xffff_0000, 0xff00_ff00, 0xff00_00ff],
    )
    .unwrap();
    let source = Rect::new(0, 0, 2, 2).unwrap();
    let target = Rect::new(0, 0, 3, 3).unwrap();
    let sampler = ScaledBitmap::with_mode(&bitmap, source, target, ScaleMode::Interpolate)
        .unwrap()
        .unwrap();
    assert_eq!(sampler.sample(0, 0), 0xff00_0000);
    assert_eq!(sampler.sample(1, 1), 0xff40_4040);
    assert_eq!(sampler.sample(2, 2), 0xff00_00ff);
    let nearest = ScaledBitmap::new(&bitmap, source, target).unwrap().unwrap();
    assert_eq!(nearest.sample(1, 1), 0xff00_00ff);
    let crop = ScaledBitmap::with_mode(
        &bitmap,
        Rect::new(1, 0, 2, 2).unwrap(),
        target,
        ScaleMode::Interpolate,
    )
    .unwrap()
    .unwrap();
    assert_eq!(crop.sample(1, 1), 0xff80_0080);
}

#[test]
fn interpolation_keeps_one_pixel_and_extreme_target_coordinates_bounded() {
    let bitmap = Bitmap::from_pixels(1, 1, vec![0x8040_2010]).unwrap();
    let source = Rect::new(0, 0, 1, 1).unwrap();
    let target = Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX).unwrap();
    let sampler = ScaledBitmap::with_mode(&bitmap, source, target, ScaleMode::Interpolate)
        .unwrap()
        .unwrap();
    for (x, y) in [(0, 0), (1024, 768), (4095, 4095)] {
        assert_eq!(sampler.sample(x, y), 0x8040_2010);
    }
    ScaleMode::decode(2).unwrap_err();
    Bitmap::from_pixels(2, 2, vec![0]).unwrap_err();
}

#[test]
fn interpolation_keeps_wide_atlases_bounded_with_extreme_target_coordinates() {
    let bitmap = Bitmap::from_pixels(65536, 1, vec![0x8040_2010; 65536]).unwrap();
    let sampler = ScaledBitmap::with_mode(
        &bitmap,
        Rect::new(0, 0, 65536, 1).unwrap(),
        Rect::new(i32::MIN, 0, i32::MAX, 1).unwrap(),
        ScaleMode::Interpolate,
    )
    .unwrap()
    .unwrap();
    assert_eq!(sampler.sample(0, 0), 0x8040_2010);
}

fn image(memory: &Memory, format: u8, flags: u8, width: u32, height: u32, stride: u32) {
    let mut header = [0; 48];
    header[10..14].copy_from_slice(&width.to_le_bytes());
    header[14..18].copy_from_slice(&height.to_le_bytes());
    header[18] = format;
    header[19] = flags;
    header[20..24].copy_from_slice(&width.to_le_bytes());
    header[24..28].copy_from_slice(&height.to_le_bytes());
    header[28..32].copy_from_slice(&stride.to_le_bytes());
    if format <= 5 {
        header[32..40].copy_from_slice(&(SLOT | PALETTE as u64).to_le_bytes());
    }
    header[40..48].copy_from_slice(&(SLOT | DATA as u64).to_le_bytes());
    memory.write(Region::Ram, IMAGE, &header).unwrap();
}

fn decode(
    memory: &Memory,
    addresses: &AddressSpace,
) -> Result<Bitmap, super::transport::TransportError> {
    Bitmap::decode(memory, addresses, SLOT | IMAGE as u64)
}

#[test]
fn direct_rgb_observes_row_order_padding_and_alpha() {
    for (format, stride, bytes, expected) in [
        (
            7,
            4,
            vec![3, 2, 1, 0xee, 6, 5, 4, 0xdd],
            [0xff01_0203, 0xff04_0506],
        ),
        (
            8,
            4,
            vec![3, 2, 1, 7, 6, 5, 4, 8],
            [0xff01_0203, 0xff04_0506],
        ),
        (
            9,
            4,
            vec![3, 2, 1, 7, 6, 5, 4, 8],
            [0x0701_0203, 0x0804_0506],
        ),
        (6, 2, vec![0, 0x7c, 0xe0, 3], [0xffff_0000, 0xff00_ff00]),
        (10, 1, vec![0x40, 0x80], [0x40ff_ffff, 0x80ff_ffff]),
    ] {
        for top_down in [false, true] {
            let (memory, addresses) = setup();
            image(
                &memory,
                format,
                1 | if top_down { 4 } else { 0 },
                1,
                2,
                stride,
            );
            memory.write(Region::Ram, DATA, &bytes).unwrap();
            let bitmap = decode(&memory, &addresses).unwrap();
            assert_eq!(bitmap.dimensions(), (1, 2));
            assert_eq!(
                bitmap.pixels(),
                if top_down {
                    expected
                } else {
                    [expected[1], expected[0]]
                }
            );
        }
    }
}

#[test]
fn indexed_pixels_follow_bit_and_nibble_order_without_reading_padding() {
    for (format, bytes, expected) in [
        (1, vec![0b0000_0101], vec![1, 0, 1]),
        (2, vec![0b1010_0000], vec![1, 0, 1]),
        (3, vec![0x21, 0xf0], vec![1, 2, 0]),
        (4, vec![0x12, 0x0f], vec![1, 2, 0]),
        (5, vec![1, 2, 0], vec![1, 2, 0]),
    ] {
        let (memory, addresses) = setup();
        image(
            &memory,
            format,
            5,
            3,
            1,
            u32::try_from(bytes.len()).unwrap(),
        );
        let mut palette = vec![0; 10];
        palette[8..10].copy_from_slice(&3u16.to_le_bytes());
        palette.extend([0u32, 1, 2].into_iter().flat_map(u32::to_le_bytes));
        memory.write(Region::Ram, PALETTE, &palette).unwrap();
        memory.write(Region::Ram, DATA, &bytes).unwrap();
        assert_eq!(
            decode(&memory, &addresses).unwrap().pixels(),
            expected
                .into_iter()
                .map(|p| p | 0xff00_0000)
                .collect::<Vec<_>>()
        );
        memory
            .write(Region::Ram, PALETTE + 8, &1u16.to_le_bytes())
            .unwrap();
        decode(&memory, &addresses).unwrap_err();
    }
}

fn chunk(memory: &Memory, offset: usize, previous: u64, next: u64, bytes: &[u8]) {
    let mut data = Vec::new();
    data.extend(u32::try_from(bytes.len()).unwrap().to_le_bytes());
    data.extend(previous.to_le_bytes());
    data.extend(next.to_le_bytes());
    data.extend(bytes);
    memory.write(Region::Ram, offset, &data).unwrap();
}

#[test]
fn chained_data_can_split_pixels_and_start_with_an_empty_chunk() {
    let (memory, addresses) = setup();
    image(&memory, 9, 4, 2, 1, 8);
    let next = SLOT | 1024;
    chunk(&memory, DATA, 0, next, &[]);
    chunk(&memory, 1024, SLOT | DATA as u64, SLOT | 2048, &[1, 2, 3]);
    chunk(&memory, 2048, next, 0, &[4, 5, 6, 7, 8]);
    assert_eq!(
        decode(&memory, &addresses).unwrap().pixels(),
        [0x0403_0201, 0x0807_0605]
    );
}

#[test]
fn bitmap_atlases_can_extend_beyond_display_dimensions() {
    let (memory, addresses) = setup();
    for (width, height, area) in [
        (32, 4256, [0, 992, 32, 1019]),
        (4256, 32, [992, 0, 1019, 32]),
    ] {
        let pixels = vec![0x8040_2010u32; width as usize * height as usize];
        let data: Vec<_> = pixels
            .iter()
            .flat_map(|pixel| pixel.to_le_bytes())
            .collect();
        image(&memory, 9, 0, width, height, width * 4);
        chunk(&memory, DATA, 0, 0, &data);
        let bitmap = decode(&memory, &addresses).unwrap();
        assert_eq!(bitmap.dimensions(), (width, height));
        assert_eq!(bitmap.pixels(), pixels);
        bitmap
            .validate_rect(Rect::new(area[0], area[1], area[2], area[3]).unwrap())
            .unwrap();
        assert_eq!(
            Bitmap::from_pixels(width, height, pixels.clone())
                .unwrap()
                .pixels(),
            pixels
        );
    }
}

#[test]
fn malformed_chains_fail_before_exposing_pixels() {
    for (previous, next, bytes) in [
        (1, 0, vec![0; 4]),
        (0, 0, vec![0; 3]),
        (0, 0, vec![0; 5]),
        (0, SLOT | DATA as u64, vec![]),
        (0, 0x0102_0000_0000_0800, vec![]),
    ] {
        let (memory, addresses) = setup();
        image(&memory, 8, 4, 1, 1, 4);
        chunk(&memory, DATA, previous, next, &bytes);
        decode(&memory, &addresses).unwrap_err();
    }
    let (memory, addresses) = setup();
    image(&memory, 8, 4, 1, 1, 4);
    memory
        .write(
            Region::Ram,
            IMAGE + 40,
            &(SLOT | (RAM_HEADER - 20) as u64).to_le_bytes(),
        )
        .unwrap();
    chunk(&memory, RAM_HEADER - 20, 0, 0, &[]);
    memory
        .write(Region::Ram, RAM_HEADER - 20, &4u32.to_le_bytes())
        .unwrap();
    decode(&memory, &addresses).unwrap_err();
}

#[test]
fn empty_chunk_work_is_bounded_even_with_valid_backlinks() {
    let (memory, addresses) = setup();
    image(&memory, 8, 4, 1, 1, 4);
    for index in 0..4097 {
        let offset = DATA + index * 24;
        let previous = if index == 0 {
            0
        } else {
            SLOT | (offset - 24) as u64
        };
        chunk(&memory, offset, previous, SLOT | (offset + 24) as u64, &[]);
    }
    chunk(
        &memory,
        DATA + 4097 * 24,
        SLOT | (DATA + 4096 * 24) as u64,
        0,
        &[0; 4],
    );
    decode(&memory, &addresses).unwrap_err();
}

#[test]
fn invalid_metadata_and_palette_sizes_are_rejected() {
    let (memory, addresses) = setup();
    for (offset, bytes) in [
        (8, vec![1]),
        (9, vec![4]),
        (18, vec![0]),
        (19, vec![8]),
        (10, 0u32.to_le_bytes().to_vec()),
        (10, 4097u32.to_le_bytes().to_vec()),
        (20, 2u32.to_le_bytes().to_vec()),
        (28, 3u32.to_le_bytes().to_vec()),
        (28, u32::MAX.to_le_bytes().to_vec()),
        (32, SLOT.to_le_bytes().to_vec()),
        (40, 0x0102_0000_0000_0200u64.to_le_bytes().to_vec()),
    ] {
        image(&memory, 8, 5, 1, 1, 4);
        memory.write(Region::Ram, IMAGE + offset, &bytes).unwrap();
        decode(&memory, &addresses).unwrap_err();
    }
    for count in [0u16, 257, u16::MAX] {
        image(&memory, 5, 5, 1, 1, 1);
        memory
            .write(Region::Ram, PALETTE + 8, &count.to_le_bytes())
            .unwrap();
        decode(&memory, &addresses).unwrap_err();
    }
    image(&memory, 8, 5, 4096, 4096, 16384);
    decode(&memory, &addresses).unwrap_err();
}
