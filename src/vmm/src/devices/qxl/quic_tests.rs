// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::*;

const STREAMS: &[(u32, u32, u32, u32, &[u8])] = &[
    (
        1,
        1,
        1,
        0,
        include_bytes!("../../../../../tests/data/qxl/quic/1-1-1-0.quic"),
    ),
    (
        1,
        1,
        3,
        1,
        include_bytes!("../../../../../tests/data/qxl/quic/1-1-3-1.quic"),
    ),
    (
        1,
        2051,
        7,
        2,
        include_bytes!("../../../../../tests/data/qxl/quic/1-2051-7-2.quic"),
    ),
    (
        1,
        37,
        9,
        1,
        include_bytes!("../../../../../tests/data/qxl/quic/1-37-9-1.quic"),
    ),
    (
        1,
        4096,
        5,
        3,
        include_bytes!("../../../../../tests/data/qxl/quic/1-4096-5-3.quic"),
    ),
    (
        2,
        1,
        1,
        0,
        include_bytes!("../../../../../tests/data/qxl/quic/2-1-1-0.quic"),
    ),
    (
        2,
        1,
        3,
        1,
        include_bytes!("../../../../../tests/data/qxl/quic/2-1-3-1.quic"),
    ),
    (
        2,
        2051,
        7,
        2,
        include_bytes!("../../../../../tests/data/qxl/quic/2-2051-7-2.quic"),
    ),
    (
        2,
        37,
        9,
        1,
        include_bytes!("../../../../../tests/data/qxl/quic/2-37-9-1.quic"),
    ),
    (
        2,
        4096,
        5,
        3,
        include_bytes!("../../../../../tests/data/qxl/quic/2-4096-5-3.quic"),
    ),
    (
        3,
        1,
        1,
        0,
        include_bytes!("../../../../../tests/data/qxl/quic/3-1-1-0.quic"),
    ),
    (
        3,
        1,
        3,
        1,
        include_bytes!("../../../../../tests/data/qxl/quic/3-1-3-1.quic"),
    ),
    (
        3,
        2051,
        7,
        2,
        include_bytes!("../../../../../tests/data/qxl/quic/3-2051-7-2.quic"),
    ),
    (
        3,
        37,
        9,
        1,
        include_bytes!("../../../../../tests/data/qxl/quic/3-37-9-1.quic"),
    ),
    (
        3,
        4096,
        5,
        3,
        include_bytes!("../../../../../tests/data/qxl/quic/3-4096-5-3.quic"),
    ),
    (
        4,
        1,
        1,
        0,
        include_bytes!("../../../../../tests/data/qxl/quic/4-1-1-0.quic"),
    ),
    (
        4,
        1,
        3,
        1,
        include_bytes!("../../../../../tests/data/qxl/quic/4-1-3-1.quic"),
    ),
    (
        4,
        2051,
        7,
        2,
        include_bytes!("../../../../../tests/data/qxl/quic/4-2051-7-2.quic"),
    ),
    (
        4,
        37,
        9,
        1,
        include_bytes!("../../../../../tests/data/qxl/quic/4-37-9-1.quic"),
    ),
    (
        4,
        4096,
        5,
        3,
        include_bytes!("../../../../../tests/data/qxl/quic/4-4096-5-3.quic"),
    ),
    (
        5,
        1,
        1,
        0,
        include_bytes!("../../../../../tests/data/qxl/quic/5-1-1-0.quic"),
    ),
    (
        5,
        1,
        3,
        1,
        include_bytes!("../../../../../tests/data/qxl/quic/5-1-3-1.quic"),
    ),
    (
        5,
        2051,
        7,
        2,
        include_bytes!("../../../../../tests/data/qxl/quic/5-2051-7-2.quic"),
    ),
    (
        5,
        37,
        9,
        1,
        include_bytes!("../../../../../tests/data/qxl/quic/5-37-9-1.quic"),
    ),
    (
        5,
        4096,
        5,
        3,
        include_bytes!("../../../../../tests/data/qxl/quic/5-4096-5-3.quic"),
    ),
];

fn expected(kind: u32, x: u32, y: u32, pattern: u32) -> u32 {
    let byte = |channel: u32| -> u32 {
        (match pattern {
            0 => 73 + channel * 17,
            1 => 73 * x + 151 * y + 61 * channel + 7 * x * y,
            2 => 37 * (x / 23 + y / 3) + 19 * channel,
            _ => x + 3 * y + 47 * channel,
        }) & 255
    };
    match kind {
        1 => 0xff00_0000 | (byte(0) * 0x010101),
        2 => {
            let value = byte(0) | (byte(1) << 8);
            let channel = |shift: u32| {
                let c = (value >> shift) & 31;
                (c << 3) | (c >> 2)
            };
            0xff00_0000 | (channel(10) << 16) | (channel(5) << 8) | channel(0)
        }
        _ => {
            (if kind == 5 { byte(3) } else { 255 }) << 24
                | (byte(2) << 16)
                | (byte(1) << 8)
                | byte(0)
        }
    }
}

#[test]
fn reference_streams_cover_all_formats_predictors_runs_and_model_windows() {
    for &(kind, width, height, pattern, stream) in STREAMS {
        let pixels = decode(stream, width, height).unwrap();
        assert_eq!(pixels.len(), (width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                assert_eq!(
                    pixels[(y * width + x) as usize],
                    expected(kind, x, y, pattern),
                    "format={kind} width={width} height={height} pattern={pattern} x={x} y={y}"
                );
            }
        }
    }
}

#[test]
fn malformed_headers_and_short_payloads_fail_before_rendering() {
    for &(_, width, height, _, stream) in STREAMS {
        for end in 0..20 {
            decode(&stream[..end], width, height).unwrap_err();
        }
        for (offset, value) in [(0, 0), (4, 1), (8, 0), (8, 6), (12, 0), (16, 0)] {
            let mut invalid = stream.to_vec();
            invalid[offset..offset + 4].copy_from_slice(&u32::to_le_bytes(value));
            decode(&invalid, width, height).unwrap_err();
        }
        assert_eq!(decode(stream, width + 1, height), Err(Error::Invalid));
        assert_eq!(decode(stream, width, height + 1), Err(Error::Invalid));
        let mut no_payload = stream[..20].to_vec();
        no_payload.extend([0; 4]);
        if width * height > 32 {
            decode(&no_payload, width, height).unwrap_err();
        }
    }
    let mut huge = STREAMS[0].4.to_vec();
    huge[12..16].copy_from_slice(&4096u32.to_le_bytes());
    huge[16..20].copy_from_slice(&4096u32.to_le_bytes());
    assert_eq!(decode(&huge, 4096, 4096), Err(Error::Invalid));
    huge[12..16].copy_from_slice(&4097u32.to_le_bytes());
    assert_eq!(decode(&huge, 4097, 1), Err(Error::Invalid));
}

#[test]
fn unary_runs_and_escape_symbols_are_bounded() {
    for (depth, word) in [(5, 15u32 << 7), (8, 255u32 << 6)] {
        let input = word.to_le_bytes();
        let mut bits = Bits {
            input: &input,
            position: 0,
        };
        assert_eq!(bits.symbol(0, depth), Err(Error::Invalid));
    }
    let mut bits = Bits {
        input: &[255; 8],
        position: 0,
    };
    assert_eq!(Schedule::new().run(&mut bits, 4096), Err(Error::Invalid));
    let mut bits = Bits {
        input: &[0; 4],
        position: 0,
    };
    assert_eq!(bits.symbol(7, 8), Ok(128));
    let mut bits = Bits {
        input: &[],
        position: 0,
    };
    assert_eq!(bits.take(1), Err(Error::Truncated));
}
