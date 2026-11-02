// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::memory::{Memory, Region};
use super::path::Path;
use super::protocol::DecodeError;
use super::test_utils::{memory, path};
use super::transport::{AddressSpace, TransportError};

const SLOT: u64 = 0x0101_0000_0000_0000;
const PATH: usize = 256;
const STYLE: usize = 8192;

fn decode(
    memory: &Memory,
    addresses: &AddressSpace,
    segments: &[(u32, &[[i32; 2]])],
    lengths: Option<&[i32]>,
    start_with_gap: bool,
) -> Result<Path, TransportError> {
    memory.write(Region::Ram, PATH, &path(segments)).unwrap();
    let mut attr = [0xa5; 20];
    attr[0] = if lengths.is_some() { 8 } else { 0 } | if start_with_gap { 4 } else { 0 };
    attr[4..8].copy_from_slice(&16i32.to_le_bytes());
    if let Some(lengths) = lengths {
        attr[3] = u8::try_from(lengths.len()).unwrap();
        attr[12..20].copy_from_slice(&(SLOT | STYLE as u64).to_le_bytes());
        let data: Vec<_> = lengths
            .iter()
            .flat_map(|length| length.to_le_bytes())
            .collect();
        memory.write(Region::Ram, STYLE, &data).unwrap();
    }
    Path::decode(memory, addresses, SLOT | PATH as u64, &attr)
}

#[test]
fn continued_records_close_the_figure_without_repeating_vertices() {
    let (memory, addresses) = memory();
    let path = decode(
        &memory,
        &addresses,
        &[(1, &[[0, 0], [32, 0]]), (10, &[[32, 32], [0, 32]])],
        None,
        false,
    )
    .unwrap();
    let pixels = path.pixels(4, 4).unwrap();
    assert_eq!(
        pixels.iter().map(|pixel| pixel.point).collect::<Vec<_>>(),
        vec![
            [0, 0],
            [1, 0],
            [2, 0],
            [0, 1],
            [2, 1],
            [0, 2],
            [1, 2],
            [2, 2]
        ]
    );
    assert!(pixels.iter().all(|pixel| pixel.foreground));
}

#[test]
fn dash_phase_counts_clipped_pixels_and_continues_until_explicit_reset() {
    let (memory, addresses) = memory();
    let segments: &[(u32, &[[i32; 2]])] = &[
        (3, &[[-32, 0], [48, 0]]),
        (3, &[[0, 16], [48, 16]]),
        (7, &[[0, 32], [48, 32]]),
    ];
    let path = decode(&memory, &addresses, segments, Some(&[16]), false).unwrap();
    let pixels = path.pixels(4, 4).unwrap();
    assert_eq!(
        pixels
            .iter()
            .map(|pixel| pixel.foreground)
            .collect::<Vec<_>>(),
        vec![true, false, true, false, true, false, true, false, true]
    );
    let reversed = decode(
        &memory,
        &addresses,
        &[(3, &[[48, 0], [-32, 0]])],
        Some(&[16]),
        true,
    )
    .unwrap();
    let pixels = reversed.pixels(4, 4).unwrap();
    assert_eq!(
        pixels
            .iter()
            .map(|pixel| (pixel.point, pixel.foreground))
            .collect::<Vec<_>>(),
        vec![
            ([0, 0], true),
            ([1, 0], false),
            ([2, 0], true),
            ([3, 0], false)
        ]
    );
}

#[test]
fn fixed_point_style_lengths_and_odd_cycles_alternate() {
    let (memory, addresses) = memory();
    for (lengths, expected) in [
        (&[8, 24][..], vec![true, false, true, false]),
        (&[32][..], vec![true, true, false, false]),
    ] {
        let path = decode(
            &memory,
            &addresses,
            &[(3, &[[0, 0], [64, 0]])],
            Some(lengths),
            false,
        )
        .unwrap();
        assert_eq!(
            path.pixels(4, 1)
                .unwrap()
                .iter()
                .map(|pixel| pixel.foreground)
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn invalid_paths_and_styles_fail_before_rasterisation() {
    let (memory, addresses) = memory();
    for segments in [
        vec![(2, &[[0, 0]][..])],
        vec![(9, &[[0, 0]][..])],
        vec![(5, &[[0, 0]][..]), (1, &[[16, 16]][..])],
        vec![(19, &[[0, 0]][..])],
        vec![(3, &[][..])],
    ] {
        assert!(matches!(
            decode(&memory, &addresses, &segments, None, false).unwrap_err(),
            TransportError::Decode(DecodeError::Parameter)
        ));
    }
    for lengths in [vec![], vec![0], vec![-16], vec![16; 101]] {
        assert!(matches!(
            decode(
                &memory,
                &addresses,
                &[(3, &[[0, 0], [16, 16]])],
                Some(&lengths),
                false
            )
            .unwrap_err(),
            TransportError::Decode(DecodeError::Parameter)
        ));
    }
    assert!(matches!(
        decode(&memory, &addresses, &[(1, &[[0, 0]])], None, false).unwrap_err(),
        TransportError::Decode(DecodeError::Truncated)
    ));
}

#[test]
fn path_records_can_cross_chained_chunk_boundaries() {
    let (memory, addresses) = memory();
    let data = path(&[(3, &[[0, 0], [32, 0]])]);
    let second = PATH + 4096;
    let mut first = data[..29].to_vec();
    first[4..8].copy_from_slice(&5u32.to_le_bytes());
    first[16..24].copy_from_slice(&(SLOT | second as u64).to_le_bytes());
    memory.write(Region::Ram, PATH, &first).unwrap();
    let mut tail = Vec::new();
    tail.extend(u32::try_from(data.len() - 29).unwrap().to_le_bytes());
    tail.extend((SLOT | (PATH + 4) as u64).to_le_bytes());
    tail.extend([0; 8]);
    tail.extend(&data[29..]);
    memory.write(Region::Ram, second, &tail).unwrap();
    let mut attr = [0; 20];
    attr[4..8].copy_from_slice(&16i32.to_le_bytes());
    let path = Path::decode(&memory, &addresses, SLOT | PATH as u64, &attr).unwrap();
    assert_eq!(
        path.pixels(4, 4)
            .unwrap()
            .iter()
            .map(|pixel| pixel.point)
            .collect::<Vec<_>>(),
        vec![[0, 0], [1, 0]]
    );
}

#[test]
fn path_input_and_prepared_pixels_have_finite_limits() {
    let (memory, addresses) = memory();
    let points = vec![[0, 0]; 65537];
    assert!(matches!(
        decode(&memory, &addresses, &[(3, &points)], None, false).unwrap_err(),
        TransportError::Decode(DecodeError::Parameter)
    ));
    let line = [[0, 0], [65536, 0]];
    let segments = vec![(3, line.as_slice()); 257];
    let path = decode(&memory, &addresses, &segments, None, false).unwrap();
    assert_eq!(path.pixels(4096, 1).unwrap_err(), DecodeError::Parameter);
    let path = decode(&memory, &addresses, &[], None, false).unwrap();
    assert!(path.pixels(4, 4).unwrap().is_empty());
}
