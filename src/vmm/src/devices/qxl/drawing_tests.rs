// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::drawing::{Drawable, Layout};
use super::memory::{COMMAND_RING, Memory, RAM_HEADER, Region};
use super::protocol::{DecodeError, PrimarySurface, Rect};
use super::surface::Surface;
use super::test_utils::{fill, glyph, path, rect};
use super::transport::{AddressSpace, Queue, Queues, TransportError};

const SLOT: u64 = 0x0101_0000_0000_0000;
const DRAW: usize = 1024;
const CLIPS: usize = 2048;
const IMAGE: usize = 4096;
const MASK: usize = 8192;
const STRING: usize = 16384;

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
        fixture.background(0);
        fixture
    }

    fn background(&self, color: u32) {
        self.surface()
            .fill(Rect::new(0, 0, 4, 4).unwrap(), color)
            .unwrap();
    }

    fn surface(&self) -> Surface<'_> {
        Surface::open(&self.memory, &self.addresses, self.primary).unwrap()
    }

    fn prepare(&self, layout: Layout) -> Result<Drawable, TransportError> {
        Drawable::decode(&self.memory, &self.addresses, SLOT | DRAW as u64, layout)
    }

    fn draw(&self, layout: Layout) {
        self.prepare(layout)
            .unwrap()
            .render(&self.surface())
            .unwrap();
    }

    fn store(&self, offset: usize, bytes: &[u8]) {
        self.memory.write(Region::Ram, offset, bytes).unwrap();
    }

    fn string(&self, flags: u16, count: u16, glyphs: &[u8]) {
        let length = u32::try_from(glyphs.len()).unwrap();
        let mut data = Vec::new();
        data.extend(length.to_le_bytes());
        data.extend(count.to_le_bytes());
        data.extend(flags.to_le_bytes());
        data.extend(length.to_le_bytes());
        data.extend([0; 16]);
        data.extend(glyphs);
        self.store(STRING, &data);
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

    fn surface_image(&self, address: usize, id: u32) {
        let mut header = [0; 22];
        header[8] = 104;
        header[18..22].copy_from_slice(&id.to_le_bytes());
        self.store(address, &header);
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

fn command(layout: Layout, opcode: u8, bounds: [i32; 4], payload: &[u8]) -> Vec<u8> {
    super::test_utils::drawable(layout, opcode, bounds, payload, 9)
}

fn image_payload(length: usize) -> Vec<u8> {
    let mut data = vec![0; length];
    data[..8].copy_from_slice(&(SLOT | IMAGE as u64).to_le_bytes());
    data[8..24].copy_from_slice(&rect([0, 0, 1, 1]));
    data
}

fn text_payload(background: [i32; 4], foreground_color: u32, background_color: u32) -> Vec<u8> {
    let mut data = vec![0; 68];
    data[..8].copy_from_slice(&(SLOT | STRING as u64).to_le_bytes());
    data[8..24].copy_from_slice(&rect(background));
    data[24..44].copy_from_slice(&fill(foreground_color, 8)[..20]);
    data[44..64].copy_from_slice(&fill(background_color, 8)[..20]);
    data[64..66].copy_from_slice(&8u16.to_le_bytes());
    data[66..68].copy_from_slice(&8u16.to_le_bytes());
    data
}

fn stroke_payload(foreground: u16) -> Vec<u8> {
    let mut data = vec![0xa5; 52];
    data[..8].copy_from_slice(&(SLOT | STRING as u64).to_le_bytes());
    data[8] = 0;
    data[12..16].copy_from_slice(&16i32.to_le_bytes());
    data[28..48].copy_from_slice(&fill(1, 8)[..20]);
    data[48..50].copy_from_slice(&foreground.to_le_bytes());
    data
}

#[test]
fn cosmetic_strokes_share_clip_union_and_signed_surface_rows() {
    for layout in [Layout::Modern, Layout::Compat] {
        let fixture = Fixture::new(-20);
        fixture.background(7);
        fixture.store(STRING, &path(&[(3, &[[0, 0], [64, 0]])]));
        let mut data = command(layout, 10, [0, 0, 4, 4], &stroke_payload(64));
        fixture.clips(&mut data, layout, &[[1, 0, 3, 1], [1, 0, 3, 1]]);
        fixture.store(DRAW, &data);
        fixture.draw(layout);
        assert_eq!(
            fixture.surface().frame().unwrap().pixels,
            [7, 6, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7]
        );
    }
}

#[test]
fn native_styled_strokes_do_not_paint_the_off_dash_pixels() {
    for layout in [Layout::Modern, Layout::Compat] {
        for (flags, expected) in [(8, [1, 7, 1, 7]), (12, [7, 1, 7, 1])] {
            let fixture = Fixture::new(20);
            fixture.background(7);
            fixture.store(STRING, &path(&[(3, &[[0, 0], [64, 0]])]));
            let lengths: Vec<_> = [16i32; 8].into_iter().flat_map(i32::to_le_bytes).collect();
            fixture.store(IMAGE, &lengths);
            let mut data = stroke_payload(8);
            data[8] = flags;
            data[11] = 8;
            data[20..28].copy_from_slice(&(SLOT | IMAGE as u64).to_le_bytes());
            data[50..52].copy_from_slice(&8u16.to_le_bytes());
            fixture.store(DRAW, &command(layout, 10, [0, 0, 4, 1], &data));
            fixture.draw(layout);
            assert_eq!(&fixture.surface().frame().unwrap().pixels[..4], &expected);
        }
    }
}

#[test]
fn styled_strokes_ignore_background_rop_and_accept_destination_only_operations() {
    let fixture = Fixture::new(20);
    fixture.background(7);
    fixture.store(STRING, &path(&[(3, &[[0, 0], [64, 0]])]));
    fixture.store(IMAGE, &16i32.to_le_bytes());
    let mut data = stroke_payload(512);
    data[8] = 8;
    data[11] = 1;
    data[20..28].copy_from_slice(&(SLOT | IMAGE as u64).to_le_bytes());
    data[28..48].fill(0xa5);
    data[50..52].copy_from_slice(&0u16.to_le_bytes());
    fixture.store(DRAW, &command(Layout::Modern, 10, [0, 0, 4, 1], &data));
    fixture.draw(Layout::Modern);
    assert_eq!(
        &fixture.surface().frame().unwrap().pixels[..4],
        &[0xfffff8, 7, 0xfffff8, 7]
    );
    fixture.background(7);
    data[28..48].copy_from_slice(&fill(1, 8)[..20]);
    data[48..50].copy_from_slice(&8u16.to_le_bytes());
    data[50..52].fill(0xa5);
    fixture.store(DRAW, &command(Layout::Compat, 10, [0, 0, 4, 1], &data));
    fixture.draw(Layout::Compat);
    assert_eq!(
        &fixture.surface().frame().unwrap().pixels[..4],
        &[1, 7, 1, 7]
    );
}

#[test]
fn crossing_stroke_segments_preserve_repeated_raster_operations() {
    let fixture = Fixture::new(20);
    fixture.background(7);
    fixture.store(
        STRING,
        &path(&[(3, &[[0, 16], [64, 16]]), (3, &[[16, 0], [16, 64]])]),
    );
    fixture.store(
        DRAW,
        &command(Layout::Modern, 10, [0, 0, 4, 4], &stroke_payload(64)),
    );
    fixture.draw(Layout::Modern);
    assert_eq!(
        fixture.surface().frame().unwrap().pixels,
        [7, 6, 7, 7, 6, 7, 6, 6, 7, 6, 7, 7, 7, 6, 7, 7]
    );
}

#[test]
fn malformed_later_stroke_records_do_not_write_and_nop_ignores_the_brush() {
    let fixture = Fixture::new(20);
    fixture.background(7);
    fixture.store(STRING, &path(&[(3, &[[0, 0], [64, 0]]), (32, &[[0, 0]])]));
    fixture.store(
        DRAW,
        &command(Layout::Modern, 10, [0, 0, 4, 4], &stroke_payload(8)),
    );
    fixture.prepare(Layout::Modern).unwrap_err();
    assert_eq!(fixture.surface().frame().unwrap().pixels, [7; 16]);
    fixture.store(STRING, &path(&[(3, &[[0, 0], [64, 0]])]));
    let mut data = stroke_payload(0);
    data[28..48].fill(0xa5);
    fixture.store(DRAW, &command(Layout::Modern, 10, [0, 0, 4, 4], &data));
    fixture.draw(Layout::Modern);
    assert_eq!(fixture.surface().frame().unwrap().pixels, [7; 16]);
}

#[test]
fn text_background_and_positioned_glyphs_use_the_common_clip() {
    for layout in [Layout::Modern, Layout::Compat] {
        let fixture = Fixture::new(-20);
        fixture.string(1, 1, &glyph([2, 2], [-1, -1], 2, 2, &[0x40, 0x80]));
        let mut data = command(
            layout,
            11,
            [0, 0, 4, 4],
            &text_payload([0, 0, 4, 4], 0x123456, 0xabcdef),
        );
        fixture.clips(&mut data, layout, &[[1, 0, 4, 3]]);
        fixture.store(DRAW, &data);
        fixture.draw(layout);
        assert_eq!(
            fixture.surface().frame().unwrap().pixels,
            [
                0, 0xabcdef, 0xabcdef, 0xabcdef, 0, 0x123456, 0xabcdef, 0xabcdef, 0, 0xabcdef,
                0x123456, 0xabcdef, 0, 0, 0, 0
            ]
        );
    }
}

#[test]
fn antialiased_text_uses_packed_coverage_without_reading_unused_background_paint() {
    for (flags, bits) in [(2, vec![0x18]), (4, vec![16, 128])] {
        let fixture = Fixture::new(20);
        fixture.string(flags, 1, &glyph([0, 0], [0, 0], 2, 1, &bits));
        let mut data = text_payload([0, 0, 0, 0], 0xffffff, 0);
        data[44..64].fill(0xa5);
        data[66..68].fill(0xa5);
        fixture.store(DRAW, &command(Layout::Modern, 11, [0, 0, 2, 1], &data));
        fixture.draw(Layout::Modern);
        assert_eq!(
            &fixture.surface().frame().unwrap().pixels[..2],
            &[0x101010, 0x808080]
        );
    }
}

#[test]
fn overlapping_text_glyphs_apply_the_foreground_raster_only_once() {
    let fixture = Fixture::new(20);
    fixture.background(7);
    let mut bits = glyph([0, 0], [0, 0], 1, 1, &[0x80]);
    bits.extend_from_within(..);
    fixture.string(1, 2, &bits);
    let mut data = text_payload([0, 0, 0, 0], 1, 0);
    data[64..66].copy_from_slice(&64u16.to_le_bytes());
    fixture.store(DRAW, &command(Layout::Modern, 11, [0, 0, 1, 1], &data));
    fixture.draw(Layout::Modern);
    assert_eq!(fixture.surface().frame().unwrap().pixels[0], 6);
}

#[test]
fn empty_text_can_paint_a_background_without_reading_foreground_fields() {
    let fixture = Fixture::new(20);
    fixture.string(0, 0, &[]);
    let mut data = text_payload([1, 1, 3, 3], 0, 0x123456);
    data[24..44].fill(0xa5);
    data[64..66].fill(0xa5);
    fixture.store(DRAW, &command(Layout::Modern, 11, [0, 0, 4, 4], &data));
    fixture.draw(Layout::Modern);
    assert_eq!(
        fixture.surface().frame().unwrap().pixels,
        [
            0, 0, 0, 0, 0, 0x123456, 0x123456, 0, 0, 0x123456, 0x123456, 0, 0, 0, 0, 0
        ]
    );
}

#[test]
fn malformed_later_glyphs_do_not_partially_paint_the_text_background() {
    let fixture = Fixture::new(20);
    fixture.background(7);
    let mut bits = glyph([0, 0], [0, 0], 1, 1, &[0x80]);
    bits.extend([0; 19]);
    fixture.string(1, 2, &bits);
    fixture.store(
        DRAW,
        &command(
            Layout::Modern,
            11,
            [0, 0, 4, 4],
            &text_payload([0, 0, 4, 4], 1, 2),
        ),
    );
    fixture.prepare(Layout::Modern).unwrap_err();
    assert_eq!(fixture.surface().frame().unwrap().pixels, [7; 16]);
}

#[test]
fn opaque_combines_source_with_brush_independently_of_destination() {
    let fixture = Fixture::new(20);
    fixture.background(0x555555);
    fixture.image(IMAGE, 9, 1, 1, &[0x33, 0x22, 0x11, 0xff]);
    let mut data = image_payload(64);
    data[24..28].copy_from_slice(&1u32.to_le_bytes());
    data[28..32].copy_from_slice(&0xff00u32.to_le_bytes());
    data[44..46].copy_from_slice(&(64u16 | 2 | 4).to_le_bytes());
    data[46] = 1;
    fixture.store(DRAW, &command(Layout::Modern, 2, [0, 0, 4, 4], &data));
    fixture.draw(Layout::Modern);
    assert_eq!(fixture.surface().frame().unwrap().pixels, [0xee22cc; 16]);
}

#[test]
fn copy_and_blend_commands_share_raster_semantics() {
    for opcode in [3, 5] {
        let fixture = Fixture::new(20);
        fixture.background(0xabcdef);
        fixture.image(IMAGE, 9, 1, 1, &[3, 2, 1, 0xff]);
        let mut data = image_payload(44);
        data[24..26].copy_from_slice(&64u16.to_le_bytes());
        fixture.store(DRAW, &command(Layout::Modern, opcode, [0, 0, 4, 4], &data));
        fixture.draw(Layout::Modern);
        assert_eq!(fixture.surface().frame().unwrap().pixels, [0xaacfec; 16]);
    }
}

#[test]
fn ternary_commands_do_not_resolve_unused_sources_or_brushes() {
    for (opcode, color) in [
        (0xf0u8, 0x445566),
        (0xcc, 0x112233),
        (0xff, 0xffffff),
        (0, 0),
    ] {
        let fixture = Fixture::new(20);
        fixture.image(IMAGE, 9, 1, 1, &[0x33, 0x22, 0x11, 0xff]);
        let mut data = image_payload(63);
        if opcode != 0xcc {
            data[..24].fill(0xa5);
        }
        data[24..44].fill(0xa5);
        if opcode == 0xf0 {
            data[24..28].copy_from_slice(&1u32.to_le_bytes());
            data[28..32].copy_from_slice(&0x445566u32.to_le_bytes());
        }
        data[44] = opcode;
        data[45] = if opcode == 0xcc { 1 } else { 0xff };
        fixture.store(DRAW, &command(Layout::Modern, 9, [0, 0, 4, 4], &data));
        fixture.draw(Layout::Modern);
        assert_eq!(fixture.surface().frame().unwrap().pixels, [color; 16]);
    }
}

#[test]
fn transparent_copies_compare_true_colour_without_alpha() {
    let fixture = Fixture::new(20);
    fixture.background(0xabcdef);
    fixture.image(
        IMAGE,
        9,
        2,
        1,
        &[0x33, 0x22, 0x11, 0x10, 0x66, 0x55, 0x44, 0xff],
    );
    let mut data = image_payload(32);
    data[8..24].copy_from_slice(&rect([0, 0, 2, 1]));
    data[24..28].copy_from_slice(&0x10112233u32.to_le_bytes());
    data[28..32].copy_from_slice(&0x112233u32.to_le_bytes());
    fixture.store(DRAW, &command(Layout::Modern, 12, [0, 0, 4, 1], &data));
    fixture.draw(Layout::Modern);
    assert_eq!(
        &fixture.surface().frame().unwrap().pixels[..4],
        &[0xabcdef, 0xabcdef, 0x445566, 0x445566]
    );
}

#[test]
fn alpha_commands_decode_both_layouts_and_surface_alpha_flags() {
    for (layout, flags, surface_source, expected) in [
        (Layout::Modern, 1u16, false, 0xa0200060),
        (Layout::Modern, 3, false, 0xa0200060),
        (Layout::Compat, 0, false, 0xff200060),
        (Layout::Modern, 1, true, 0xc0200040),
        (Layout::Modern, 3, true, 0xa0200060),
        (Layout::Compat, 0, true, 0xff200040),
    ] {
        let fixture = Fixture::new(20);
        fixture.background(0x80000080);
        if surface_source {
            fixture.store(64, &0x80400000u32.to_le_bytes());
            fixture.surface_image(IMAGE, 0);
        } else {
            fixture.image(IMAGE, 9, 1, 1, &[0, 0, 0x40, 0x80]);
        }
        let mut data = Vec::new();
        if matches!(layout, Layout::Modern) {
            data.extend(flags.to_le_bytes());
        }
        data.push(128);
        data.extend(image_payload(24));
        fixture.store(DRAW, &command(layout, 13, [1, 0, 2, 1], &data));
        fixture.draw(layout);
        let mut pixel = [0; 4];
        fixture.memory.read(Region::Ram, 68, &mut pixel).unwrap();
        assert_eq!(u32::from_le_bytes(pixel), expected);
    }
}

#[test]
fn alpha_rounding_uses_the_bitmap_format_even_for_opaque_argb_pixels() {
    for (format, expected) in [(8, 0xff2b773e), (9, 0xff2c773e)] {
        let fixture = Fixture::new(20);
        fixture.background(0xff317352);
        fixture.image(IMAGE, format, 1, 1, &[3, 0x83, 0x1b, 0xff]);
        let mut data = vec![0, 0, 64];
        data.extend(image_payload(24));
        fixture.store(DRAW, &command(Layout::Modern, 13, [1, 0, 2, 1], &data));
        fixture.draw(Layout::Modern);
        let mut pixel = [0; 4];
        fixture.memory.read(Region::Ram, 68, &mut pixel).unwrap();
        assert_eq!(u32::from_le_bytes(pixel), expected);
    }
}

#[test]
fn primary_surface_images_capture_overlapping_sources_and_only_read_their_header() {
    for layout in [Layout::Modern, Layout::Compat] {
        let fixture = Fixture::new(20);
        let pixels = [0x11000001u32, 0x22000002, 0x33000003, 0x44000004];
        fixture.store(
            64,
            &pixels
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        let address = RAM_HEADER - 22;
        fixture.surface_image(address, 0);
        let mut data = image_payload(44);
        data[..8].copy_from_slice(&(SLOT | address as u64).to_le_bytes());
        data[8..24].copy_from_slice(&rect([0, 0, 3, 1]));
        data[24..26].copy_from_slice(&8u16.to_le_bytes());
        data[26] = 1;
        fixture.store(DRAW, &command(layout, 3, [1, 0, 4, 1], &data));
        let draw = fixture.prepare(layout).unwrap();
        fixture.store(address, &[0; 22]);
        draw.render(&fixture.surface()).unwrap();
        let mut row = [0; 16];
        fixture.memory.read(Region::Ram, 64, &mut row).unwrap();
        assert_eq!(
            row.as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| u32::from_le_bytes(*pixel))
                .collect::<Vec<_>>(),
            vec![0x11000001, 0x11000001, 0x22000002, 0x33000003]
        );
    }
}

#[test]
fn invalid_surface_ids_and_crops_leave_the_destination_unchanged() {
    let fixture = Fixture::new(20);
    fixture.background(7);
    let mut data = image_payload(44);
    data[24..26].copy_from_slice(&8u16.to_le_bytes());
    data[26] = 1;
    fixture.surface_image(IMAGE, 1);
    fixture.store(DRAW, &command(Layout::Modern, 3, [0, 0, 4, 4], &data));
    assert!(matches!(
        fixture.prepare(Layout::Modern).unwrap_err(),
        TransportError::Decode(DecodeError::Parameter)
    ));
    fixture.surface_image(IMAGE, 0);
    data[8..24].copy_from_slice(&rect([0, 0, 5, 1]));
    fixture.store(DRAW, &command(Layout::Modern, 3, [0, 0, 4, 4], &data));
    let draw = fixture.prepare(Layout::Modern).unwrap();
    assert!(matches!(
        draw.render(&fixture.surface()).unwrap_err(),
        TransportError::Decode(DecodeError::Parameter)
    ));
    assert_eq!(fixture.surface().frame().unwrap().pixels, [7; 16]);
}

#[test]
fn operand_free_rasters_ignore_uninitialised_image_and_brush_fields() {
    let fixture = Fixture::new(20);
    let mut data = fill(0, 128);
    data[..20].fill(0xa5);
    fixture.store(DRAW, &command(Layout::Modern, 1, [0, 0, 4, 4], &data));
    fixture.draw(Layout::Modern);
    let mut data = vec![0xa5; 44];
    data[24..26].copy_from_slice(&256u16.to_le_bytes());
    data[27..].fill(0);
    fixture.store(DRAW, &command(Layout::Modern, 3, [0, 0, 4, 4], &data));
    fixture.draw(Layout::Modern);
    assert_eq!(fixture.surface().frame().unwrap().pixels, [0xffffff; 16]);
}

#[test]
fn bad_scaling_and_alpha_flags_fail_before_destination_writes() {
    let fixture = Fixture::new(20);
    fixture.image(IMAGE, 9, 1, 1, &[1, 2, 3, 0xff]);
    let mut data = image_payload(44);
    data[24..26].copy_from_slice(&8u16.to_le_bytes());
    data[26] = 2;
    fixture.store(DRAW, &command(Layout::Modern, 3, [0, 0, 4, 4], &data));
    fixture.prepare(Layout::Modern).unwrap_err();
    let mut data = vec![4, 0, 255];
    data.extend(image_payload(24));
    fixture.store(DRAW, &command(Layout::Modern, 13, [0, 0, 4, 4], &data));
    fixture.prepare(Layout::Modern).unwrap_err();
    assert_eq!(fixture.surface().frame().unwrap().pixels, [0; 16]);
}

#[test]
fn both_layouts_fill_without_dereferencing_unused_clip_and_mask_fields() {
    for layout in [Layout::Modern, Layout::Compat] {
        let fixture = Fixture::new(-20);
        fixture.store(
            DRAW,
            &command(layout, 1, [-1, -1, 2, 2], &fill(0x123456, 8)),
        );
        let prepared = fixture.prepare(layout).unwrap();
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
    let prepared = fixture.prepare(Layout::Modern).unwrap();
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
    fixture.prepare(Layout::Modern).unwrap_err();
    fixture.store(CLIPS, &4097u32.to_le_bytes());
    fixture.prepare(Layout::Modern).unwrap_err();
    let mut payload = fill(7, 8);
    payload[31..39].copy_from_slice(&0x0102_0000_0000_2000u64.to_le_bytes());
    fixture.store(DRAW, &command(Layout::Modern, 1, [0, 0, 4, 4], &payload));
    fixture.prepare(Layout::Modern).unwrap_err();
    let mut source = vec![0; 8];
    source[..4].copy_from_slice(&i32::MAX.to_le_bytes());
    fixture.store(DRAW, &command(Layout::Modern, 4, [0, 0, 4, 4], &source));
    fixture
        .prepare(Layout::Modern)
        .unwrap()
        .render(&fixture.surface())
        .unwrap_err();
    assert_eq!(fixture.surface().frame().unwrap().pixels, [0; 16]);
}
