// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::cursor::{Command, Shape};
use super::memory::{Memory, RAM_HEADER, Region};
use super::protocol::DecodeError;
use super::test_utils::memory;
use super::transport::{AddressSpace, TransportError};
use crate::devices::display::Frame;

const SLOT: u64 = 0x0101_0000_0000_0000;
const SHAPE: usize = 1024;

fn decode_shape(
    memory: &Memory,
    addresses: &AddressSpace,
    kind: u16,
    width: u16,
    hotspot: [u16; 2],
    data: &[u8],
) -> Result<Shape, TransportError> {
    let mut bytes = Vec::new();
    bytes.extend(0u64.to_le_bytes());
    for value in [kind, width, 1, hotspot[0], hotspot[1]] {
        bytes.extend(value.to_le_bytes());
    }
    let length = u32::try_from(data.len()).unwrap();
    bytes.extend(length.to_le_bytes());
    bytes.extend(length.to_le_bytes());
    bytes.extend([0; 16]);
    bytes.extend(data);
    memory.write(Region::Ram, SHAPE, &bytes).unwrap();
    Shape::decode(memory, addresses, SLOT | SHAPE as u64)
}

fn frame() -> Frame {
    Frame {
        width: 4,
        height: 1,
        pixels: vec![0x123456; 4],
    }
}

#[test]
fn monochrome_cursor_covers_all_and_xor_combinations() {
    let (memory, addresses) = memory();
    let shape = decode_shape(&memory, &addresses, 1, 4, [0, 0], &[0x30, 0x50]).unwrap();
    let mut frame = frame();
    shape.draw(&mut frame, [0, 0]);
    assert_eq!(frame.pixels, [0, 0xffffff, 0x123456, 0xedcba9]);
}

#[test]
fn alpha_cursor_blends_premultiplied_colours_and_clips_its_hotspot() {
    let (memory, addresses) = memory();
    let pixels = [0, 0, 0x40, 0x80, 0, 0, 0, 0];
    let shape = decode_shape(&memory, &addresses, 0, 2, [0, 0], &pixels).unwrap();
    let mut frame = frame();
    frame.pixels.fill(0x80);
    shape.draw(&mut frame, [0, 0]);
    assert_eq!(frame.pixels, [0x400040, 0x80, 0x80, 0x80]);
    let shape = decode_shape(&memory, &addresses, 0, 2, [1, 0], &pixels).unwrap();
    shape.draw(&mut frame, [0, 0]);
    assert_eq!(frame.pixels, [0x400040, 0x80, 0x80, 0x80]);
    shape.draw(&mut frame, [i16::MIN, i16::MAX]);
    assert_eq!(frame.pixels, [0x400040, 0x80, 0x80, 0x80]);
}

#[test]
fn every_colour_format_preserves_the_cursor_mask_rules() {
    let (memory, addresses) = memory();
    for kind in 2..=6 {
        let mut data = match kind {
            2 => vec![0x12],
            3 => vec![1, 2],
            4 => vec![0, 0x7c, 0xff, 0x7f],
            5 => vec![0, 0, 255, 255, 255, 255],
            _ => vec![0, 0, 255, 0, 255, 255, 255, 0],
        };
        if kind <= 3 {
            let mut palette = vec![0u32; if kind == 2 { 16 } else { 256 }];
            palette[1] = 0xff0000;
            palette[2] = 0xffffff;
            data.extend(palette.into_iter().flat_map(u32::to_le_bytes));
        }
        data.push(0xc0);
        let shape = decode_shape(&memory, &addresses, kind, 2, [0, 0], &data).unwrap();
        let mut frame = frame();
        shape.draw(&mut frame, [0, 0]);
        assert_eq!(frame.pixels, [0xff0000, 0xedcba9, 0x123456, 0x123456]);
    }
}

#[test]
fn empty_transparent_cursor_ignores_uninitialised_chunk_links() {
    let (memory, addresses) = memory();
    let address = RAM_HEADER - 22;
    let mut header = [0; 22];
    header[8..10].copy_from_slice(&1u16.to_le_bytes());
    memory.write(Region::Ram, address, &header).unwrap();
    let shape = Shape::decode(&memory, &addresses, SLOT | address as u64).unwrap();
    let mut frame = frame();
    shape.draw(&mut frame, [0, 0]);
    assert_eq!(frame.pixels, [0x123456; 4]);
}

#[test]
fn cursor_commands_read_only_their_active_union_fields() {
    let (memory, addresses) = memory();
    for (kind, payload) in [
        (1, vec![0xfe, 0xff, 3, 0]),
        (2, vec![]),
        (3, vec![2, 0, 16, 0]),
    ] {
        let mut bytes = vec![0xa5; 9];
        bytes[8] = kind;
        bytes.extend(payload);
        let address = RAM_HEADER - bytes.len();
        memory.write(Region::Ram, address, &bytes).unwrap();
        let command = Command::decode(&memory, &addresses, SLOT | address as u64).unwrap();
        match kind {
            1 => assert!(matches!(command, Command::Move([-2, 3]))),
            2 => assert!(matches!(command, Command::Hide)),
            _ => assert!(matches!(
                command,
                Command::Trail {
                    length: 2,
                    frequency: 16
                }
            )),
        }
    }
    decode_shape(&memory, &addresses, 1, 1, [0, 0], &[0, 0]).unwrap();
    let mut data = vec![0; 22];
    data[9..13].copy_from_slice(&[4, 0, 5, 0]);
    data[13] = 1;
    data[14..22].copy_from_slice(&(SLOT | SHAPE as u64).to_le_bytes());
    memory.write(Region::Ram, 0, &data).unwrap();
    assert!(matches!(
        Command::decode(&memory, &addresses, SLOT).unwrap(),
        Command::Set {
            position: [4, 5],
            visible: true,
            ..
        }
    ));
    data[13] = 2;
    memory.write(Region::Ram, 0, &data).unwrap();
    assert!(matches!(
        Command::decode(&memory, &addresses, SLOT).unwrap_err(),
        TransportError::Decode(DecodeError::Parameter)
    ));
}

#[test]
fn cursor_dimensions_hotspots_types_and_data_lengths_are_bounded() {
    let (memory, addresses) = memory();
    for (kind, width, hotspot, data) in [
        (7, 1, [0, 0], vec![0; 4]),
        (0, 257, [0, 0], vec![]),
        (0, 1, [1, 0], vec![0; 4]),
        (0, 1, [0, 1], vec![0; 4]),
        (0, 1, [0, 0], vec![0; 3]),
        (1, 1, [0, 0], vec![0]),
    ] {
        assert!(matches!(
            decode_shape(&memory, &addresses, kind, width, hotspot, &data).unwrap_err(),
            TransportError::Decode(DecodeError::Parameter)
        ));
    }
}
