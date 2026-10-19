// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::drawing::{Drawable, Layout};
use super::memory::{COMMAND_RING, Memory, Region};
use super::protocol::{PrimarySurface, Rect};
use super::surface::Surface;
use super::transport::{AddressSpace, Queue, Queues};

const SLOT: u64 = 0x0101_0000_0000_0000;
const DRAW: usize = 1024;
const CLIPS: usize = 2048;
const IMAGE: usize = 4096;
const MASK: usize = 8192;

struct Fixture {
    memory: Memory,
    addresses: AddressSpace,
    primary: PrimarySurface,
}

impl Fixture {
    fn new(stride: i32) -> Self {
        let (memory, addresses) = super::test_utils::memory();
        let mut primary = [0; 40];
        primary[..4].copy_from_slice(&4u32.to_le_bytes());
        primary[4..8].copy_from_slice(&4u32.to_le_bytes());
        primary[8..12].copy_from_slice(&stride.to_le_bytes());
        primary[12..16].copy_from_slice(&96u32.to_le_bytes());
        primary[32..40].copy_from_slice(&(SLOT | 64).to_le_bytes());
        memory.write(Region::Ram, 64, &[0xa5; 80]).unwrap();
        let fixture = Self {
            memory,
            addresses,
            primary: PrimarySurface::decode(&primary).unwrap(),
        };
        fixture
            .surface()
            .fill(Rect::new(0, 0, 4, 4).unwrap(), 0)
            .unwrap();
        fixture
    }

    fn surface(&self) -> Surface<'_> {
        Surface::open(&self.memory, &self.addresses, self.primary).unwrap()
    }

    fn draw(&self, layout: Layout) {
        Drawable::decode(&self.memory, &self.addresses, SLOT | DRAW as u64, layout)
            .unwrap()
            .render(&self.surface())
            .unwrap();
    }

    fn store(&self, offset: usize, bytes: &[u8]) {
        self.memory.write(Region::Ram, offset, bytes).unwrap();
    }

    fn image(&self, address: usize, format: u8, width: u32, height: u32, data: &[u8]) {
        let mut header = [0; 48];
        for (offset, value) in [(10, width), (14, height), (20, width), (24, height)] {
            header[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        header[18] = format;
        header[19] = 5;
        let stride = if format == 2 {
            width.div_ceil(8)
        } else {
            width * 4
        };
        header[28..32].copy_from_slice(&stride.to_le_bytes());
        header[40..48].copy_from_slice(&(SLOT | (address + 48) as u64).to_le_bytes());
        self.store(address, &header);
        self.store(address + 48, data);
    }

    fn clips(&self, command: &mut [u8], layout: Layout, rects: &[[i32; 4]]) {
        let offset = match layout {
            Layout::Modern => 47,
            Layout::Compat => 44,
        };
        command[offset..offset + 4].copy_from_slice(&1u32.to_le_bytes());
        command[offset + 4..offset + 12].copy_from_slice(&(SLOT | CLIPS as u64).to_le_bytes());
        let count = u32::try_from(rects.len()).unwrap();
        let mut data = Vec::new();
        data.extend(count.to_le_bytes());
        data.extend((count * 16).to_le_bytes());
        data.extend([0; 16]);
        for bounds in rects {
            data.extend(rect(*bounds));
        }
        self.store(CLIPS, &data);
    }
}

fn rect([left, top, right, bottom]: [i32; 4]) -> Vec<u8> {
    [top, left, bottom, right]
        .into_iter()
        .flat_map(i32::to_le_bytes)
        .collect()
}

fn command(layout: Layout, opcode: u8, bounds: [i32; 4], payload: &[u8]) -> Vec<u8> {
    let (size, rectangle, clip) = match layout {
        Layout::Modern => (123, 31, 47),
        Layout::Compat => (60, 28, 44),
    };
    let mut data = vec![0xa5; size];
    data[..8].copy_from_slice(&0x1234u64.to_le_bytes());
    match layout {
        Layout::Modern => {
            data[8..12].copy_from_slice(&9u32.to_le_bytes());
            data[12] = 0;
            data[13] = opcode;
            data[14] = 0;
        }
        Layout::Compat => {
            data[8] = 0;
            data[9] = opcode;
            data[10..12].fill(0);
        }
    }
    data[rectangle..rectangle + 16].copy_from_slice(&rect(bounds));
    data[clip..clip + 4].fill(0);
    data.extend(payload);
    data
}

fn fill(color: u32, descriptor: u16) -> Vec<u8> {
    let mut data = vec![0xa5; 39];
    data[..4].copy_from_slice(&1u32.to_le_bytes());
    data[4..8].copy_from_slice(&color.to_le_bytes());
    data[20..22].copy_from_slice(&descriptor.to_le_bytes());
    data[31..39].fill(0);
    data
}

#[test]
fn both_layouts_fill_without_dereferencing_unused_clip_and_mask_fields() {
    for layout in [Layout::Modern, Layout::Compat] {
        let fixture = Fixture::new(-20);
        fixture.store(
            DRAW,
            &command(layout, 1, [-1, -1, 2, 2], &fill(0x123456, 8)),
        );
        let prepared = Drawable::decode(
            &fixture.memory,
            &fixture.addresses,
            SLOT | DRAW as u64,
            layout,
        )
        .unwrap();
        assert_eq!(
            prepared.surface_id(),
            if matches!(layout, Layout::Modern) {
                9
            } else {
                0
            }
        );
        prepared.render(&fixture.surface()).unwrap();
        assert_eq!(
            fixture.surface().frame().unwrap().pixels,
            [
                0x123456, 0x123456, 0, 0, 0x123456, 0x123456, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );
        for offset in [80, 100, 120, 140] {
            let mut pad = [0; 4];
            fixture.memory.read(Region::Ram, offset, &mut pad).unwrap();
            assert_eq!(pad, [0xa5; 4]);
        }
    }
}

#[test]
fn overlapping_clip_rectangles_apply_xor_only_once() {
    let fixture = Fixture::new(20);
    let mut data = command(Layout::Modern, 1, [0, 0, 4, 4], &fill(1, 64));
    fixture.clips(
        &mut data,
        Layout::Modern,
        &[[1, 0, 4, 2], [0, 1, 2, 4], [1, 1, 2, 2]],
    );
    fixture.store(DRAW, &data);
    fixture.draw(Layout::Modern);
    assert_eq!(
        fixture.surface().frame().unwrap().pixels,
        [0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0]
    );
    fixture.clips(&mut data, Layout::Modern, &[]);
    fixture.store(DRAW, &data);
    fixture.draw(Layout::Modern);
    assert_eq!(fixture.surface().frame().unwrap().pixels[1], 1);
}

#[test]
fn patterns_and_unpaletted_masks_keep_their_origins_and_inversion() {
    for inverted in [false, true] {
        let fixture = Fixture::new(20);
        fixture.image(IMAGE, 9, 2, 1, &[2, 0, 0, 0xff, 1, 0, 0, 0xff]);
        fixture.image(MASK, 2, 4, 2, &[0xa0, 0x50]);
        let mut payload = fill(0, 8);
        payload[..4].copy_from_slice(&2u32.to_le_bytes());
        payload[4..12].copy_from_slice(&(SLOT | IMAGE as u64).to_le_bytes());
        payload[12..16].copy_from_slice(&(-1i32).to_le_bytes());
        payload[16..20].fill(0);
        payload[22] = u8::from(inverted);
        payload[23..31].fill(0);
        payload[31..39].copy_from_slice(&(SLOT | MASK as u64).to_le_bytes());
        fixture.store(DRAW, &command(Layout::Modern, 1, [0, 0, 4, 2], &payload));
        fixture.draw(Layout::Modern);
        let expected = if inverted {
            [0, 2, 0, 2, 1, 0, 1, 0]
        } else {
            [1, 0, 1, 0, 0, 2, 0, 2]
        };
        assert_eq!(&fixture.surface().frame().unwrap().pixels[..8], &expected);
    }
}

#[test]
fn bitmap_copies_prepare_pixels_before_rendering_clipped_scaling() {
    let fixture = Fixture::new(20);
    fixture.image(
        IMAGE,
        9,
        2,
        2,
        &[1, 0, 0, 0x11, 2, 0, 0, 0x22, 3, 0, 0, 0x33, 4, 0, 0, 0x44],
    );
    let mut payload = vec![0; 44];
    payload[..8].copy_from_slice(&(SLOT | IMAGE as u64).to_le_bytes());
    payload[8..24].copy_from_slice(&rect([0, 0, 2, 2]));
    payload[24..26].copy_from_slice(&8u16.to_le_bytes());
    payload[26] = 1;
    let mut data = command(Layout::Modern, 3, [0, 0, 4, 4], &payload);
    fixture.clips(&mut data, Layout::Modern, &[[1, 1, 4, 4]]);
    fixture.store(DRAW, &data);
    let prepared = Drawable::decode(
        &fixture.memory,
        &fixture.addresses,
        SLOT | DRAW as u64,
        Layout::Modern,
    )
    .unwrap();
    fixture.store(IMAGE, &[0; 64]);
    fixture.store(CLIPS, &[0; 40]);
    prepared.render(&fixture.surface()).unwrap();
    assert_eq!(
        fixture.surface().frame().unwrap().pixels,
        [0, 0, 0, 0, 0, 1, 2, 2, 0, 3, 4, 4, 0, 3, 4, 4]
    );
}

#[test]
fn clipped_overlapping_screen_copies_read_the_original_surface() {
    let fixture = Fixture::new(-20);
    for y in 0..4 {
        for x in 0..4 {
            fixture
                .surface()
                .fill(
                    Rect::new(x, y, x + 1, y + 1).unwrap(),
                    0x6700_0000 | u32::try_from(y * 4 + x + 1).unwrap(),
                )
                .unwrap();
        }
    }
    let mut data = command(Layout::Modern, 4, [0, 1, 4, 4], &[0; 8]);
    fixture.clips(&mut data, Layout::Modern, &[[0, 1, 2, 4], [1, 1, 4, 3]]);
    fixture.store(DRAW, &data);
    fixture.draw(Layout::Modern);
    assert_eq!(
        fixture.surface().frame().unwrap().pixels,
        [1, 2, 3, 4, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 15, 16]
    );
    let mut pixel = [0; 4];
    fixture.memory.read(Region::Ram, 104, &mut pixel).unwrap();
    assert_eq!(u32::from_le_bytes(pixel), 0x6700_0001);
}

#[test]
fn invalid_draws_are_not_written_or_released() {
    let fixture = Fixture::new(20);
    fixture.store(DRAW, &command(Layout::Modern, 1, [0, 0, 4, 4], &fill(7, 8)));
    let mut entry = [0; 16];
    entry[..8].copy_from_slice(&(SLOT | DRAW as u64).to_le_bytes());
    entry[8..12].copy_from_slice(&1u32.to_le_bytes());
    fixture.store(COMMAND_RING + 20, &entry);
    fixture.memory.store(COMMAND_RING + 4, 1).unwrap();
    let mut queues = Queues::default();
    let drain = |queues: &mut Queues| {
        queues.drain(
            &fixture.memory,
            &fixture.addresses,
            Queue::Commands,
            |cmd| {
                Drawable::decode(
                    &fixture.memory,
                    &fixture.addresses,
                    cmd.address,
                    Layout::Modern,
                )?
                .render(&fixture.surface())
            },
        )
    };
    fixture.store(DRAW + 13, &[99]);
    drain(&mut queues).unwrap_err();
    assert_eq!(fixture.memory.load(COMMAND_RING + 12).unwrap(), 0);
    assert_eq!(fixture.surface().frame().unwrap().pixels, [0; 16]);
    fixture.store(DRAW + 13, &[1]);
    assert_eq!(drain(&mut queues).unwrap(), 1);
    assert_eq!(fixture.memory.load(COMMAND_RING + 12).unwrap(), 1);
    assert_eq!(fixture.surface().frame().unwrap().pixels, [7; 16]);
}

#[test]
fn malformed_clip_mask_and_copy_parameters_fail_before_writes() {
    let fixture = Fixture::new(20);
    let mut data = command(Layout::Modern, 1, [0, 0, 4, 4], &fill(7, 8));
    fixture.clips(&mut data, Layout::Modern, &[[0, 0, 2, 2], [0, 0, 1, 1]]);
    fixture.store(CLIPS + 24 + 16, &rect([3, 0, 1, 1]));
    fixture.store(DRAW, &data);
    Drawable::decode(
        &fixture.memory,
        &fixture.addresses,
        SLOT | DRAW as u64,
        Layout::Modern,
    )
    .unwrap_err();
    fixture.store(CLIPS, &4097u32.to_le_bytes());
    Drawable::decode(
        &fixture.memory,
        &fixture.addresses,
        SLOT | DRAW as u64,
        Layout::Modern,
    )
    .unwrap_err();
    let mut payload = fill(7, 8);
    payload[31..39].copy_from_slice(&0x0102_0000_0000_2000u64.to_le_bytes());
    fixture.store(DRAW, &command(Layout::Modern, 1, [0, 0, 4, 4], &payload));
    Drawable::decode(
        &fixture.memory,
        &fixture.addresses,
        SLOT | DRAW as u64,
        Layout::Modern,
    )
    .unwrap_err();
    let mut source = vec![0; 8];
    source[..4].copy_from_slice(&i32::MAX.to_le_bytes());
    fixture.store(DRAW, &command(Layout::Modern, 4, [0, 0, 4, 4], &source));
    Drawable::decode(
        &fixture.memory,
        &fixture.addresses,
        SLOT | DRAW as u64,
        Layout::Modern,
    )
    .unwrap()
    .render(&fixture.surface())
    .unwrap_err();
    assert_eq!(fixture.surface().frame().unwrap().pixels, [0; 16]);
}
