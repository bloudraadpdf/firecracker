// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Physical keyboard codes shared by the console and PS/2 controller.

// IBM enhanced keyboard set-2 make codes paired with their set-1 equivalents.
pub(crate) fn set1_code(code: u8) -> Option<u8> {
    Some(match code {
        0x76 => 0x01,
        0x16 => 0x02,
        0x1e => 0x03,
        0x26 => 0x04,
        0x25 => 0x05,
        0x2e => 0x06,
        0x36 => 0x07,
        0x3d => 0x08,
        0x3e => 0x09,
        0x46 => 0x0a,
        0x45 => 0x0b,
        0x4e => 0x0c,
        0x55 => 0x0d,
        0x66 => 0x0e,
        0x0d => 0x0f,
        0x15 => 0x10,
        0x1d => 0x11,
        0x24 => 0x12,
        0x2d => 0x13,
        0x2c => 0x14,
        0x35 => 0x15,
        0x3c => 0x16,
        0x43 => 0x17,
        0x44 => 0x18,
        0x4d => 0x19,
        0x54 => 0x1a,
        0x5b => 0x1b,
        0x5a => 0x1c,
        0x14 => 0x1d,
        0x1c => 0x1e,
        0x1b => 0x1f,
        0x23 => 0x20,
        0x2b => 0x21,
        0x34 => 0x22,
        0x33 => 0x23,
        0x3b => 0x24,
        0x42 => 0x25,
        0x4b => 0x26,
        0x4c => 0x27,
        0x52 => 0x28,
        0x0e => 0x29,
        0x12 => 0x2a,
        0x5d => 0x2b,
        0x1a => 0x2c,
        0x22 => 0x2d,
        0x21 => 0x2e,
        0x2a => 0x2f,
        0x32 => 0x30,
        0x31 => 0x31,
        0x3a => 0x32,
        0x41 => 0x33,
        0x49 => 0x34,
        0x4a => 0x35,
        0x59 => 0x36,
        0x7c => 0x37,
        0x11 => 0x38,
        0x29 => 0x39,
        0x58 => 0x3a,
        0x05 => 0x3b,
        0x06 => 0x3c,
        0x04 => 0x3d,
        0x0c => 0x3e,
        0x03 => 0x3f,
        0x0b => 0x40,
        0x83 => 0x41,
        0x0a => 0x42,
        0x01 => 0x43,
        0x09 => 0x44,
        0x77 => 0x45,
        0x7e => 0x46,
        0x6c => 0x47,
        0x75 => 0x48,
        0x7d => 0x49,
        0x7b => 0x4a,
        0x6b => 0x4b,
        0x73 => 0x4c,
        0x74 => 0x4d,
        0x79 => 0x4e,
        0x69 => 0x4f,
        0x72 => 0x50,
        0x7a => 0x51,
        0x70 => 0x52,
        0x71 => 0x53,
        0x61 => 0x56,
        0x78 => 0x57,
        0x07 => 0x58,
        0x1f => 0x5b,
        0x27 => 0x5c,
        0x2f => 0x5d,
        _ => return None,
    })
}

pub(crate) fn valid_key(code: u8, extended: bool) -> bool {
    if extended {
        matches!(
            code,
            0x11 | 0x14
                | 0x1f
                | 0x27
                | 0x2f
                | 0x4a
                | 0x5a
                | 0x69
                | 0x6b
                | 0x6c
                | 0x70
                | 0x71
                | 0x72
                | 0x74
                | 0x75
                | 0x7a
                | 0x7d
        )
    } else {
        set1_code(code).is_some() && !matches!(code, 0x1f | 0x27 | 0x2f)
    }
}

#[cfg(any(feature = "windows", test))]
pub(crate) fn evdev_key(code: u8, extended: bool) -> Option<u16> {
    if !extended {
        return match code {
            0xe0 => Some(99),
            0xe1 => Some(119),
            _ if valid_key(code, false) => set1_code(code).map(u16::from),
            _ => None,
        };
    }
    Some(match code {
        0x11 => 100,
        0x14 => 97,
        0x1f => 125,
        0x27 => 126,
        0x2f => 127,
        0x4a => 98,
        0x5a => 96,
        0x69 => 107,
        0x6b => 105,
        0x6c => 102,
        0x70 => 110,
        0x71 => 111,
        0x72 => 108,
        0x74 => 106,
        0x75 => 103,
        0x7a => 109,
        0x7d => 104,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_preserves_all_existing_fcc1_code_pairs() {
        let normal = [
            250, 126, 118, 124, 126, 126, 126, 124, 126, 126, 52, 47, 66, 26, 255, 127, 8, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let extended = [
            0, 0, 18, 128, 128, 128, 0, 0, 0, 4, 0, 4, 0, 26, 55, 36, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0,
        ];
        for code in 0..=u8::MAX {
            for (extended, bitmap) in [(false, normal), (true, extended)] {
                let existing = bitmap[usize::from(code / 8)] & (1 << (code % 8)) != 0;
                assert_eq!(valid_key(code, extended), existing);
                assert_eq!(
                    evdev_key(code, extended).is_some(),
                    existing || (!extended && matches!(code, 0xe0 | 0xe1)),
                );
            }
        }
    }

    #[test]
    fn modifiers_keypad_navigation_and_special_keys_have_distinct_evdev_codes() {
        for (code, extended, expected) in [
            (0x1c, false, 30),
            (0x5a, false, 28),
            (0x14, false, 29),
            (0x14, true, 97),
            (0x11, false, 56),
            (0x11, true, 100),
            (0x1f, true, 125),
            (0x27, true, 126),
            (0x2f, true, 127),
            (0x5a, true, 96),
            (0x4a, true, 98),
            (0x6c, false, 71),
            (0x6c, true, 102),
            (0x69, true, 107),
            (0x6b, true, 105),
            (0x70, true, 110),
            (0x71, true, 111),
            (0x72, true, 108),
            (0x74, true, 106),
            (0x75, true, 103),
            (0x7a, true, 109),
            (0x7d, true, 104),
            (0xe0, false, 99),
            (0xe1, false, 119),
        ] {
            assert_eq!(evdev_key(code, extended), Some(expected));
        }
    }
}
