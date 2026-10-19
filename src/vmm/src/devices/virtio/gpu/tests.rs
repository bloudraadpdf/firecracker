// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::*;

fn request(kind: u32, words: &[u32]) -> Vec<u8> {
    let mut bytes = vec![0; HEADER_SIZE];
    bytes[..4].copy_from_slice(&kind.to_le_bytes());
    bytes.extend(words.iter().flat_map(|word| word.to_le_bytes()));
    bytes
}

#[test]
fn fixed_commands_require_the_complete_wire_structure() {
    for (kind, queue, words) in [
        (0x0100, Queue::Control, vec![]),
        (0x0101, Queue::Control, vec![1, 2, 1024, 768]),
        (0x0102, Queue::Control, vec![1, 0]),
        (0x0103, Queue::Control, vec![0, 0, 1024, 768, 0, 1]),
        (0x0104, Queue::Control, vec![0, 0, 1024, 768, 1, 0]),
        (0x0105, Queue::Control, vec![0, 0, 1024, 768, 0, 0, 1, 0]),
        (0x0107, Queue::Control, vec![1, 0]),
        (0x0300, Queue::Cursor, vec![0, 11, 22, 0, 1, 3, 4, 0]),
        (0x0301, Queue::Cursor, vec![0, 11, 22, 0, 0, 0, 0, 0]),
    ] {
        let bytes = request(kind, &words);
        assert!(Command::decode(queue, &bytes).is_ok(), "{kind:#x}");
        for length in 0..bytes.len() {
            assert!(
                Command::decode(queue, &bytes[..length]).is_err(),
                "{kind:#x}, {length}"
            );
        }
        let other = if queue == Queue::Control {
            Queue::Cursor
        } else {
            Queue::Control
        };
        assert_eq!(
            Command::decode(other, &bytes),
            Err(DecodeError::InvalidParameter)
        );
    }
}

#[test]
fn transfer_keeps_64_bit_offset_and_rectangle_coordinates() {
    let bytes = request(0x0105, &[7, 9, 30, 40, 0x87654321, 0xfedcba98, 12, 0]);
    assert_eq!(
        Command::decode(Queue::Control, &bytes).unwrap().1,
        Command::Transfer {
            rect: Rect {
                x: 7,
                y: 9,
                width: 30,
                height: 40
            },
            offset: 0xfedcba9887654321,
            id: ResourceId::new(12).unwrap(),
        }
    );
}

#[test]
fn zero_resource_disables_scanout_and_hides_cursor() {
    let bytes = request(0x0103, &[0; 6]);
    assert!(matches!(
        Command::decode(Queue::Control, &bytes).unwrap().1,
        Command::Scanout { resource: None, .. }
    ));
    let bytes = request(0x0300, &[0; 8]);
    assert!(matches!(
        Command::decode(Queue::Cursor, &bytes).unwrap().1,
        Command::UpdateCursor { resource: None, .. }
    ));
    for kind in [0x0101, 0x0102, 0x0104, 0x0105, 0x0106, 0x0107] {
        let bytes = request(kind, &[0; 8]);
        assert_eq!(
            Command::decode(Queue::Control, &bytes),
            Err(DecodeError::InvalidParameter)
        );
    }
}

#[test]
fn move_cursor_ignores_the_unused_resource_and_hotspot() {
    let bytes = request(0x0301, &[0, 17, 23, 0, u32::MAX, u32::MAX, u32::MAX, 0]);
    assert_eq!(
        Command::decode(Queue::Cursor, &bytes).unwrap().1,
        Command::MoveCursor {
            scanout: 0,
            x: 17,
            y: 23
        }
    );
}

#[test]
fn backing_entries_are_bounded_and_decoded_without_alignment_requirements() {
    let bytes = request(
        0x0106,
        &[
            5, 2, 0x87654321, 0xfedcba98, 4096, 0, 0x12345678, 0, 8192, 0,
        ],
    );
    let mut unaligned = vec![0];
    unaligned.extend(&bytes);
    let Command::Attach { id, backing } =
        Command::decode(Queue::Control, &unaligned[1..]).unwrap().1
    else {
        panic!()
    };
    assert_eq!(id.get(), 5);
    assert_eq!(
        backing.entries().collect::<Vec<_>>(),
        [
            BackingEntry {
                address: 0xfedcba9887654321,
                length: 4096
            },
            BackingEntry {
                address: 0x12345678,
                length: 8192
            },
        ]
    );
    for length in 0..bytes.len() {
        Command::decode(Queue::Control, &bytes[..length]).unwrap_err();
    }
    for count in [0, u32::try_from(MAX_BACKING_ENTRIES).unwrap() + 1, u32::MAX] {
        let bytes = request(0x0106, &[1, count]);
        assert_eq!(
            Command::decode(Queue::Control, &bytes),
            Err(DecodeError::InvalidParameter)
        );
    }
    let mut bytes = request(0x0106, &[1, u32::try_from(MAX_BACKING_ENTRIES).unwrap()]);
    bytes.resize(HEADER_SIZE + 8 + MAX_BACKING_ENTRIES * 16, 0);
    let Command::Attach { backing, .. } = Command::decode(Queue::Control, &bytes).unwrap().1 else {
        panic!()
    };
    assert_eq!(backing.entries().len(), MAX_BACKING_ENTRIES);
}

#[test]
fn fence_responses_preserve_the_full_id_only_when_requested() {
    for fence in [0u64, 0xfedcba9887654321, u64::MAX] {
        for fenced in [false, true] {
            let mut bytes = request(0x0100, &[]);
            bytes[4..8].copy_from_slice(&u32::from(fenced).to_le_bytes());
            bytes[8..16].copy_from_slice(&fence.to_le_bytes());
            let header = Header::decode(&bytes).unwrap();
            let response = header.response(0x1100);
            assert_eq!(&response[..4], &0x1100u32.to_le_bytes());
            assert_eq!(&response[4..8], &u32::from(fenced).to_le_bytes());
            assert_eq!(
                &response[8..16],
                &if fenced { fence } else { 0 }.to_le_bytes()
            );
            assert_eq!(&response[16..], &[0; 8]);
        }
    }
}

#[test]
fn unsupported_commands_and_3d_headers_are_rejected() {
    for kind in [0, 0x0108, 0x0109, 0x010a, 0x010c, 0x0200, u32::MAX] {
        let bytes = request(kind, &[]);
        assert_eq!(
            Command::decode(Queue::Control, &bytes),
            Err(DecodeError::Unsupported(kind))
        );
    }
    for (offset, value) in [(4, 2u32), (4, u32::MAX), (16, 1)] {
        let mut bytes = request(0x0100, &[]);
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert_eq!(Header::decode(&bytes), Err(DecodeError::InvalidParameter));
    }
}
