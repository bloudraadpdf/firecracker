// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::protocol::*;

fn words(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

#[test]
fn packed_command_keeps_the_full_pointer_and_ignores_padding() {
    let data = words(&[0x89ab_cdef, 0x0102_0000, 5, u32::MAX]);
    assert_eq!(
        Command::decode(&data).unwrap(),
        Command {
            address: 0x0102_0000_89ab_cdef,
            kind: CommandKind::Surface
        }
    );
    for length in 0..Command::SIZE {
        assert_eq!(
            Command::decode(&data[..length]),
            Err(DecodeError::Truncated)
        );
    }
    assert_eq!(
        Command::decode(&words(&[0, 0, 6, 0])),
        Err(DecodeError::Command(6))
    );
}

#[test]
fn slot_resolves_relative_offsets_and_rejects_stale_or_crossing_pointers() {
    let slot = MemorySlot::new(1, 2, 0xc000_1000..0xc000_2000, 0xc000_0000..0xc100_0000).unwrap();
    let address = 0x0102_0000_0000_0800;
    assert_eq!(
        slot.resolve(address, 0x800).unwrap(),
        0xc000_1800..0xc000_2000
    );
    assert_eq!(
        slot.resolve(address + 0x800, 0).unwrap(),
        0xc000_2000..0xc000_2000
    );
    for invalid in [
        0x0101_0000_0000_0800,
        0x0202_0000_0000_0800,
        0x0000_0000_0000_0800,
    ] {
        assert_eq!(slot.resolve(invalid, 1), Err(DecodeError::Slot));
    }
    for (address, length) in [
        (address, 0x801),
        (address, u64::MAX),
        (0x0102_ffff_ffff_ffff, 1),
    ] {
        assert_eq!(slot.resolve(address, length), Err(DecodeError::Address));
    }
}

#[test]
fn slot_creation_requires_a_bounded_range_inside_one_backing() {
    for (id, range) in [
        (0, 0x1000..0x2000),
        (1, 0x1000..0x1000),
        (1, 0..0x2000),
        (1, 0x1000..0x4000),
    ] {
        assert_eq!(
            MemorySlot::new(id, 0, range, 0x1000..0x3000),
            Err(DecodeError::Slot)
        );
    }
    assert_eq!(
        MemorySlot::new(1, 0, 0..(64 << 20) + 1, 0..u64::MAX),
        Err(DecodeError::Slot)
    );
    let slot = MemorySlot::new(255, 255, u64::MAX - 4096..u64::MAX, 0..u64::MAX).unwrap();
    assert_eq!(
        slot.resolve(0xffff_0000_0000_0fff, 1).unwrap(),
        u64::MAX - 1..u64::MAX
    );
}

#[test]
fn command_ring_wraps_and_notifies_at_the_requested_consumer() {
    let data = words(&[32, 1, 0, u32::MAX - 1, 0]);
    let mut ring = Ring::decode(&data, RingSize::Commands, u32::MAX - 1).unwrap();
    assert_eq!(ring.pending(), 3);
    assert_eq!(ring.pop(), Some((30, false)));
    assert_eq!(ring.pop(), Some((31, true)));
    assert_eq!(ring.pop(), Some((0, false)));
    assert_eq!(ring.pop(), None);
    assert_eq!(ring.consumer(), 1);
}

#[test]
fn ring_rejects_overrun_and_forged_consumer_before_using_indices() {
    for values in [
        [32, 34, 0, 1, 0],
        [32, 0, 0, 0, 0],
        [8, 1, 0, 1, 0],
        [32, u32::MAX, 0, 1, 0],
    ] {
        assert_eq!(
            Ring::decode(&words(&values), RingSize::Commands, 1),
            Err(DecodeError::Ring)
        );
    }
    let data = words(&[32, 32, 0, 0, 0]);
    let mut ring = Ring::decode(&data, RingSize::Commands, 0).unwrap();
    assert_eq!(ring.pending(), 32);
    for index in 0..32 {
        assert_eq!(ring.pop().unwrap().0, index);
    }
    assert_eq!(ring.pop(), None);
    for length in 0..Ring::HEADER_SIZE {
        assert_eq!(
            Ring::decode(&data[..length], RingSize::Commands, 0),
            Err(DecodeError::Truncated)
        );
    }
    assert_eq!(
        Ring::decode(&words(&[8, 8, 0, 0, 0]), RingSize::Releases, 0)
            .unwrap()
            .pending(),
        8
    );
}

#[test]
fn rectangles_clip_signed_coordinates_without_subtraction_overflow() {
    let data = words(&[
        i32::MIN.cast_unsigned(),
        (-5i32).cast_unsigned(),
        i32::MAX.cast_unsigned(),
        i32::MAX.cast_unsigned(),
    ]);
    assert_eq!(
        Rect::decode(&data).unwrap().clipped(1366, 768).unwrap(),
        [0, 0, 1366, 768]
    );
    let empty = words(&[0, 0, 0, 0]);
    assert_eq!(
        Rect::decode(&empty).unwrap().clipped(1024, 768).unwrap(),
        [0; 4]
    );
    assert_eq!(
        Rect::decode(&words(&[10, 0, 9, 0])),
        Err(DecodeError::Parameter)
    );
    assert_eq!(
        Rect::decode(&words(&[0, 10, 0, 9])),
        Err(DecodeError::Parameter)
    );
    assert_eq!(Rect::decode(&data[..15]), Err(DecodeError::Truncated));
}

fn primary() -> Vec<u8> {
    words(&[1366, 768, 5464, 32, 0, 0, 1, 0, 0x1234, 0x0102_0000])
}

#[test]
fn primary_surface_decodes_the_packed_layout_and_signed_stride() {
    let mut data = primary();
    let surface = PrimarySurface::decode(&data).unwrap();
    assert_eq!(surface.dimensions(), (1366, 768));
    assert_eq!(surface.stride(), 5464);
    assert_eq!(surface.length(), 5464 * 768);
    assert_eq!(surface.address(), 0x0102_0000_0000_1234);
    assert_eq!(surface.format(), SurfaceFormat::Xrgb8888);
    assert!(surface.keep_data());
    data[8..12].copy_from_slice(&(-5464i32).to_le_bytes());
    data[12..16].copy_from_slice(&96u32.to_le_bytes());
    let surface = PrimarySurface::decode(&data).unwrap();
    assert_eq!(surface.stride(), -5464);
    assert_eq!(surface.length(), 5464 * 768);
    assert_eq!(surface.format(), SurfaceFormat::Argb8888);
}

#[test]
fn primary_surface_rejects_invalid_geometry_format_flags_and_oversized_storage() {
    for (offset, value) in [
        (0, 0),
        (4, 0),
        (0, 4097),
        (4, 4097),
        (8, 5463),
        (8, i32::MIN.cast_unsigned()),
        (12, 31),
        (24, 2),
        (28, 1),
    ] {
        let mut data = primary();
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert_eq!(PrimarySurface::decode(&data), Err(DecodeError::Parameter));
    }
    let data = primary();
    for length in 0..PrimarySurface::SIZE {
        assert_eq!(
            PrimarySurface::decode(&data[..length]),
            Err(DecodeError::Truncated)
        );
    }
    let mut data = words(&[4096, 2048, 16384, 32, 0, 0, 0, 0, 0, 0]);
    assert_eq!(PrimarySurface::decode(&data).unwrap().length(), 32 << 20);
    data[4..8].copy_from_slice(&2049u32.to_le_bytes());
    assert_eq!(PrimarySurface::decode(&data), Err(DecodeError::Parameter));
}
