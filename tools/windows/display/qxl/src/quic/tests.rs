// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

extern crate std;
use super::*;
use std::vec;

#[test]
fn upstream_streams_and_round_trips() {
    // Upstream e3371bd2d26473dd42798ae392e0c4cdc6a63a06, default QUIC settings.
    let cases = [
        (1, 1, 1, 0, 28, 0xd6da49cad93c6b68),
        (1, 1, 3, 1, 32, 0x5da36cfe679d0bff),
        (1, 37, 9, 1, 360, 0xf6a300f85bddfcef),
        (1, 2051, 7, 2, 1924, 0x51643d58de9936fc),
        (1, 4096, 5, 3, 9992, 0xd4da3d3a4e21b81a),
        (2, 1, 1, 0, 28, 0x59eefedcef9cfbeb),
        (2, 1, 3, 1, 32, 0x6ae8e0670e234939),
        (2, 37, 9, 1, 652, 0x6663df440bb4a290),
        (2, 2051, 7, 2, 3688, 0xe4a4f62c5316d8fd),
        (2, 4096, 5, 3, 29056, 0xae004c5496e0e1e2),
        (3, 1, 1, 0, 28, 0xcf4b3470211fa46c),
        (3, 1, 3, 1, 36, 0xfd5e8ecd8f4c0dad),
        (3, 37, 9, 1, 1028, 0x8a07f5c844f7f39f),
        (3, 2051, 7, 2, 4888, 0xdcfa1b157ea40902),
        (3, 4096, 5, 3, 29940, 0x9637299ea41576ad),
        (4, 1, 1, 0, 28, 0xd4323aea9ddf1963),
        (4, 1, 3, 1, 36, 0xe05ff8306eb53252),
        (4, 37, 9, 1, 1028, 0x149f1aba452bc6a8),
        (4, 2051, 7, 2, 4888, 0x46e7d455164c7261),
        (4, 4096, 5, 3, 29940, 0x06071d8b8be8ab86),
        (5, 1, 1, 0, 28, 0x8e0b6b5da5d2094a),
        (5, 1, 3, 1, 40, 0xa90b5b81e3d56640),
        (5, 37, 9, 1, 1364, 0xc5c3bbaf1662b1ed),
        (5, 2051, 7, 2, 6776, 0xb9ac0d2a568c7099),
        (5, 4096, 5, 3, 39908, 0x954659ca9e58c13b),
    ];
    for (kind, width, height, pattern, expected_length, expected_hash) in cases {
        let format = match kind {
            1 => Format::Gray,
            2 => Format::Rgb555,
            3 => Format::Bgr24,
            4 => Format::Bgrx32,
            _ => Format::Bgra32,
        };
        let header = Header {
            format,
            width,
            height,
        };
        let mut pixels = vec![0; header.pixel_bytes().unwrap()];
        for y in 0..height {
            for x in 0..width {
                for c in 0..format.bytes() {
                    let value = match pattern {
                        0 => 73 + c * 17,
                        1 => x * 73 + y * 151 + c * 61 + x * y * 7,
                        2 => (x / 23 + y / 3) * 37 + c * 19,
                        _ => x + y * 3 + c * 47,
                    };
                    pixels[(y * width + x) * format.bytes() + c] = value as u8;
                }
            }
        }
        let mut models = core::array::from_fn(|_| Model::new());
        let mut correlations = vec![0; header.correlation_bytes().unwrap()];
        let mut encoded = vec![0; width * height * 16 + 128];
        let length = encode(
            header,
            &pixels,
            &mut encoded,
            &mut correlations,
            &mut models,
        )
        .unwrap();
        let hash = encoded[..length]
            .iter()
            .fold(0xcbf29ce484222325u64, |h, b| {
                (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
            });
        assert_eq!(
            (length, hash),
            (expected_length, expected_hash),
            "{header:?}, pattern {pattern}"
        );
        let mut output = vec![0xff; pixels.len()];
        assert_eq!(
            decode(
                &encoded[..length],
                &mut output,
                &mut correlations,
                &mut models
            )
            .unwrap(),
            header
        );
        for i in 0..output.len() {
            let expected = if format == Format::Bgrx32 && i % 4 == 3 {
                0
            } else if format == Format::Rgb555 && i % 2 == 1 {
                pixels[i] & 127
            } else {
                pixels[i]
            };
            assert_eq!(output[i], expected, "{header:?}, byte {i}");
        }
    }
}

#[test]
fn rejects_invalid_geometry_buffers_and_streams() {
    let mut models = core::array::from_fn(|_| Model::new());
    let mut correlations = [0; 8];
    let mut output = [0; 128];
    let header = Header {
        format: Format::Bgra32,
        width: 1,
        height: 1,
    };
    assert_eq!(
        encode(
            header,
            &[1, 2, 3],
            &mut output,
            &mut correlations,
            &mut models
        ),
        Err(Error::InvalidImage)
    );
    assert_eq!(
        encode(
            header,
            &[1, 2, 3, 4],
            &mut output[..20],
            &mut correlations,
            &mut models
        ),
        Err(Error::BufferTooSmall)
    );
    assert_eq!(
        encode(
            header,
            &[1, 2, 3, 4],
            &mut output,
            &mut correlations[..7],
            &mut models
        ),
        Err(Error::BufferTooSmall)
    );
    let length = encode(
        header,
        &[1, 2, 3, 4],
        &mut output,
        &mut correlations,
        &mut models,
    )
    .unwrap();
    let mut pixels = [0; 4];
    for end in 0..24 {
        assert!(decode(&output[..end], &mut pixels, &mut correlations, &mut models).is_err());
    }
    output[0] ^= 1;
    assert_eq!(
        decode(
            &output[..length],
            &mut pixels,
            &mut correlations,
            &mut models
        ),
        Err(Error::InvalidStream)
    );
    output[0] ^= 1;
    output[8..12].copy_from_slice(&6u32.to_le_bytes());
    assert_eq!(
        decode(
            &output[..length],
            &mut pixels,
            &mut correlations,
            &mut models
        ),
        Err(Error::InvalidStream)
    );
    assert_eq!(
        Header { width: 0, ..header }.pixel_bytes(),
        Err(Error::InvalidImage)
    );
    assert_eq!(
        Header {
            height: 4097,
            ..header
        }
        .pixel_bytes(),
        Err(Error::InvalidImage)
    );
}
