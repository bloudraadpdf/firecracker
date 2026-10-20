// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Display-driver mode filtering and DEVMODEW output for DrvGetModes.

use crate::modes::VideoModeInformation;

pub const DEVMODE_BYTES: usize = 220;
const DISPLAY_FIELDS: u32 = 0x007c0080;

pub fn supported(mode: &VideoModeInformation) -> bool {
    mode.length >= 80
        && mode.planes == 1
        && mode.attributes & 1 != 0
        && matches!(mode.bits_per_plane, 16 | 32)
        && mode.width > 0
        && mode.height > 0
        && mode.width <= i32::MAX as u32
        && mode.height <= i32::MAX as u32
        && mode.driver_attributes <= 3
        && mode
            .width
            .checked_mul(mode.bits_per_plane / 8)
            .is_some_and(|minimum| {
                mode.stride >= minimum
                    && mode.stride <= i32::MAX as u32
                    && mode.stride.is_multiple_of(4)
            })
        && mode.framebuffer_bytes().is_some()
}

/// Write one complete DEVMODEW. Invalid modes and short buffers are unchanged.
pub fn encode(mode: &VideoModeInformation, output: &mut [u8]) -> Option<usize> {
    if !supported(mode) {
        return None;
    }
    let output = output.get_mut(..DEVMODE_BYTES)?;
    output.fill(0);
    for (offset, character) in "qxldd".encode_utf16().enumerate() {
        output[offset * 2..offset * 2 + 2].copy_from_slice(&character.to_le_bytes());
    }
    for (offset, value) in [(64, 0x0401u16), (66, 0x0401), (68, DEVMODE_BYTES as u16)] {
        output[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [
        (72, DISPLAY_FIELDS),
        (84, mode.driver_attributes),
        (168, mode.bits_per_plane),
        (172, mode.width),
        (176, mode.height),
        (184, mode.frequency),
    ] {
        output[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    Some(DEVMODE_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devmode_encodes_dimensions_depth_frequency_and_rotation() {
        let mut mode = VideoModeInformation::custom(7, 1920, 1080, 32).unwrap();
        mode.driver_attributes = 3;
        let mut output = [0xa5; DEVMODE_BYTES + 4];
        assert_eq!(encode(&mode, &mut output), Some(DEVMODE_BYTES));
        assert_eq!(&output[..12], b"q\0x\0l\0d\0d\0\0\0");
        assert_eq!(&output[64..72], &[1, 4, 1, 4, 220, 0, 0, 0]);
        for (offset, value) in [
            (72, DISPLAY_FIELDS),
            (84, 3),
            (168, 32),
            (172, 1920),
            (176, 1080),
            (184, 60),
        ] {
            assert_eq!(
                u32::from_le_bytes(output[offset..offset + 4].try_into().unwrap()),
                value
            );
        }
        assert!(output[12..64].iter().all(|byte| *byte == 0));
        assert!(output[88..168].iter().all(|byte| *byte == 0));
        assert!(output[188..220].iter().all(|byte| *byte == 0));
        assert_eq!(&output[220..], &[0xa5; 4]);
    }

    #[test]
    fn invalid_modes_and_short_buffers_do_not_write() {
        let valid = VideoModeInformation::custom(0, 800, 600, 16).unwrap();
        let mut output = [0xa5; DEVMODE_BYTES];
        assert_eq!(encode(&valid, &mut output[..DEVMODE_BYTES - 1]), None);
        for mode in [
            VideoModeInformation {
                length: 79,
                ..valid
            },
            VideoModeInformation { planes: 2, ..valid },
            VideoModeInformation {
                attributes: 0,
                ..valid
            },
            VideoModeInformation {
                bits_per_plane: 24,
                ..valid
            },
            VideoModeInformation {
                driver_attributes: 4,
                ..valid
            },
            VideoModeInformation { width: 0, ..valid },
            VideoModeInformation {
                height: u32::MAX,
                ..valid
            },
            VideoModeInformation {
                stride: 1601,
                ..valid
            },
        ] {
            assert!(!supported(&mode));
            assert_eq!(encode(&mode, &mut output), None);
        }
        assert_eq!(output, [0xa5; DEVMODE_BYTES]);
    }
}
