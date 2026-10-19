// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io;

/// Per-session US-layout modifier state, synchronised with guest PS/2 LEDs.
#[derive(Default)]
pub struct Keyboard {
    explicit_shift: [bool; 2],
    sent_shift: [bool; 2],
    printable: Vec<(u8, u32)>,
    leds: u8,
    locks_pressed: u8,
    shortcut_modifiers: u8,
}

impl Keyboard {
    /// Update authoritative guest LEDs before translating a key.
    pub fn set_leds(&mut self, leds: u8) {
        self.leds = leds & 7;
    }

    fn semantic_shift(&self, keysym: u32) -> Option<bool> {
        let shifted = match keysym {
            0x41..=0x5a => Some(self.leds & 4 == 0),
            0x61..=0x7a => Some(self.leds & 4 != 0),
            0x21..=0x7e => Some(b"!@#$%^&*()_+{}|:\"~<>?".contains(&u8::try_from(keysym).unwrap())),
            _ => None,
        }?;
        Some(if self.shortcut_modifiers != 0 {
            self.explicit_shift.iter().any(|pressed| *pressed)
        } else {
            shifted
        })
    }

    fn shift(
        &mut self,
        desired: [bool; 2],
        emit: &mut impl FnMut(u8, bool, bool) -> io::Result<()>,
    ) -> io::Result<()> {
        for (index, code) in [0x12, 0x59].into_iter().enumerate() {
            if self.sent_shift[index] != desired[index] {
                emit(code, false, desired[index])?;
                self.sent_shift[index] = desired[index];
            }
        }
        Ok(())
    }

    fn desired_shift(&self, shifted: bool) -> [bool; 2] {
        if !shifted {
            [false; 2]
        } else if self.explicit_shift.iter().any(|pressed| *pressed) {
            self.explicit_shift
        } else {
            [true, false]
        }
    }

    /// Emits physical key events while preserving the character carried by an RFB keysym.
    pub fn event(
        &mut self,
        keysym: u32,
        pressed: bool,
        mut emit: impl FnMut(u8, bool, bool) -> io::Result<()>,
    ) -> io::Result<()> {
        let keysym = match keysym {
            0x0100_0020..=0x0100_007e => keysym & 0xff,
            0xffb0..=0xffb9 => keysym - 0xffb0 + u32::from(b'0'),
            0xffae => u32::from(b'.'),
            0xff95..=0xff9d => keysym - 0x45,
            0xff9e => 0xff63,
            0xff9f => 0xffff,
            _ => keysym,
        };
        let Some((code, extended)) = set2(keysym) else {
            return Ok(());
        };
        if matches!(keysym, 0xffe1 | 0xffe2) {
            self.explicit_shift[usize::from(keysym == 0xffe2)] = pressed;
            return self.shift(self.explicit_shift, &mut emit);
        }
        let modifier = match (code, extended) {
            (0x14, false) => 1,
            (0x14, true) => 2,
            (0x11, false) => 4,
            (0x11, true) => 8,
            (0x1f, true) => 16,
            (0x27, true) => 32,
            _ => 0,
        };
        if modifier != 0 {
            self.shift(self.explicit_shift, &mut emit)?;
            emit(code, extended, pressed)?;
            if pressed {
                self.shortcut_modifiers |= modifier;
            } else {
                self.shortcut_modifiers &= !modifier;
            }
            return Ok(());
        }
        let lock = match keysym {
            0xffe5 => 4,
            0xff7f => 2,
            0xff14 => 1,
            _ => 0,
        };
        if lock != 0 {
            if pressed && self.locks_pressed & lock != 0 {
                return Ok(());
            }
            emit(code, extended, pressed)?;
            if pressed {
                self.locks_pressed |= lock;
            } else {
                self.locks_pressed &= !lock;
            }
            return Ok(());
        }
        let semantic_shift = self.semantic_shift(keysym);
        if let Some(shifted) = semantic_shift.filter(|_| pressed) {
            self.shift(self.desired_shift(shifted), &mut emit)?;
            emit(code, extended, true)?;
            self.printable.retain(|(held, _)| *held != code);
            self.printable.push((code, keysym));
        } else {
            emit(code, extended, pressed)?;
            if semantic_shift.is_some() {
                self.printable.retain(|(held, _)| *held != code);
                let desired = self
                    .printable
                    .last()
                    .map_or(self.explicit_shift, |(_, keysym)| {
                        self.desired_shift(self.semantic_shift(*keysym).unwrap())
                    });
                self.shift(desired, &mut emit)?;
            }
        }
        Ok(())
    }
}

/// X11 keysyms to physical US-layout set-2 keys; modifiers remain separate events.
pub fn set2(keysym: u32) -> Option<(u8, bool)> {
    let code = match keysym {
        0x61 | 0x41 => 0x1c,
        0x62 | 0x42 => 0x32,
        0x63 | 0x43 => 0x21,
        0x64 | 0x44 => 0x23,
        0x65 | 0x45 => 0x24,
        0x66 | 0x46 => 0x2b,
        0x67 | 0x47 => 0x34,
        0x68 | 0x48 => 0x33,
        0x69 | 0x49 => 0x43,
        0x6a | 0x4a => 0x3b,
        0x6b | 0x4b => 0x42,
        0x6c | 0x4c => 0x4b,
        0x6d | 0x4d => 0x3a,
        0x6e | 0x4e => 0x31,
        0x6f | 0x4f => 0x44,
        0x70 | 0x50 => 0x4d,
        0x71 | 0x51 => 0x15,
        0x72 | 0x52 => 0x2d,
        0x73 | 0x53 => 0x1b,
        0x74 | 0x54 => 0x2c,
        0x75 | 0x55 => 0x3c,
        0x76 | 0x56 => 0x2a,
        0x77 | 0x57 => 0x1d,
        0x78 | 0x58 => 0x22,
        0x79 | 0x59 => 0x35,
        0x7a | 0x5a => 0x1a,
        0x31 | 0x21 => 0x16,
        0x32 | 0x40 => 0x1e,
        0x33 | 0x23 => 0x26,
        0x34 | 0x24 => 0x25,
        0x35 | 0x25 => 0x2e,
        0x36 | 0x5e => 0x36,
        0x37 | 0x26 => 0x3d,
        0x38 | 0x2a => 0x3e,
        0x39 | 0x28 => 0x46,
        0x30 | 0x29 => 0x45,
        0x2d | 0x5f => 0x4e,
        0x3d | 0x2b => 0x55,
        0x5b | 0x7b => 0x54,
        0x5d | 0x7d => 0x5b,
        0x5c | 0x7c => 0x5d,
        0x3b | 0x3a => 0x4c,
        0x27 | 0x22 => 0x52,
        0x60 | 0x7e => 0x0e,
        0x2c | 0x3c => 0x41,
        0x2e | 0x3e => 0x49,
        0x2f | 0x3f => 0x4a,
        0x20 => 0x29,
        0xff08 => 0x66,
        0xff09 | 0xfe20 => 0x0d,
        0xff0d => 0x5a,
        0xff1b => 0x76,
        0xffe1 => 0x12,
        0xffe2 => 0x59,
        0xffe3 => 0x14,
        0xffe9 | 0xffe7 => 0x11,
        0xffe5 => 0x58,
        0xff7f => 0x77,
        0xff14 => 0x7e,
        0xffbe => 0x05,
        0xffbf => 0x06,
        0xffc0 => 0x04,
        0xffc1 => 0x0c,
        0xffc2 => 0x03,
        0xffc3 => 0x0b,
        0xffc4 => 0x83,
        0xffc5 => 0x0a,
        0xffc6 => 0x01,
        0xffc7 => 0x09,
        0xffc8 => 0x78,
        0xffc9 => 0x07,
        0xffaa => 0x7c,
        0xffab => 0x79,
        0xffad => 0x7b,
        0xff13 => return Some((0xe1, false)),
        0xff61 | 0xff15 => return Some((0xe0, false)),
        _ => {
            return Some((
                match keysym {
                    0xffe4 => 0x14,
                    0xffea | 0xffe8 | 0xfe03 => 0x11,
                    0xffeb => 0x1f,
                    0xffec => 0x27,
                    0xff67 => 0x2f,
                    0xffff => 0x71,
                    0xff63 => 0x70,
                    0xff50 => 0x6c,
                    0xff57 => 0x69,
                    0xff51 => 0x6b,
                    0xff52 => 0x75,
                    0xff53 => 0x74,
                    0xff54 => 0x72,
                    0xff55 => 0x7d,
                    0xff56 => 0x7a,
                    0xff8d => 0x5a,
                    0xffaf => 0x4a,
                    _ => return None,
                },
                true,
            ));
        }
    };
    Some((code, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_caps_lock_reverses_letter_shift_but_not_punctuation() {
        for (leds, keysym, shifted) in [
            (0, 'a', false),
            (0, 'A', true),
            (4, 'a', true),
            (4, 'A', false),
            (0, '!', true),
            (4, '!', true),
            (0, '1', false),
            (4, '1', false),
        ] {
            let mut keyboard = Keyboard::default();
            keyboard.set_leds(leds);
            let mut output = Vec::new();
            for pressed in [true, false] {
                keyboard
                    .event(u32::from(keysym), pressed, |code, extended, down| {
                        output.push((code, extended, down));
                        Ok(())
                    })
                    .unwrap();
            }
            let physical = set2(u32::from(keysym)).unwrap();
            let mut expected = Vec::new();
            if shifted {
                expected.push((0x12, false, true));
            }
            expected.extend([
                (physical.0, physical.1, true),
                (physical.0, physical.1, false),
            ]);
            if shifted {
                expected.push((0x12, false, false));
            }
            assert_eq!(output, expected);
        }
    }

    #[test]
    fn lock_repeats_do_not_retoggle_or_predict_guest_state() {
        let mut keyboard = Keyboard::default();
        let mut output = Vec::new();
        for pressed in [true, true, false] {
            keyboard
                .event(0xffe5, pressed, |code, _, down| {
                    output.push((code, down));
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(output, [(0x58, true), (0x58, false)]);
        assert_eq!(keyboard.semantic_shift(u32::from('a')), Some(false));
        keyboard.set_leds(4);
        assert_eq!(keyboard.semantic_shift(u32::from('a')), Some(true));
        let mut reconnected = Keyboard::default();
        reconnected.set_leds(4);
        assert_eq!(reconnected.semantic_shift(u32::from('a')), Some(true));
    }

    #[test]
    fn keypad_characters_and_navigation_do_not_depend_on_num_lock() {
        for leds in 0..=7 {
            let mut keyboard = Keyboard::default();
            keyboard.set_leds(leds);
            let mut output = Vec::new();
            for keysym in [0xffb1, 0xffae, 0xff9c, 0xff9f] {
                for down in [true, false] {
                    keyboard
                        .event(keysym, down, |code, extended, down| {
                            output.push((code, extended, down));
                            Ok(())
                        })
                        .unwrap();
                }
            }
            assert_eq!(
                output,
                [
                    (0x16, false, true),
                    (0x16, false, false),
                    (0x49, false, true),
                    (0x49, false, false),
                    (0x69, true, true),
                    (0x69, true, false),
                    (0x71, true, true),
                    (0x71, true, false),
                ]
            );
        }
    }

    #[test]
    fn caps_lock_does_not_change_control_or_windows_shortcuts() {
        for leds in [0, 4] {
            let mut keyboard = Keyboard::default();
            keyboard.set_leds(leds);
            let mut output = Vec::new();
            for (keysym, pressed) in [
                (0xffe3, true),
                (0xffe1, true),
                (u32::from('S'), true),
                (u32::from('S'), false),
                (0xffe1, false),
                (0xffe3, false),
                (0xffeb, true),
                (u32::from('r'), true),
                (u32::from('r'), false),
                (0xffeb, false),
            ] {
                keyboard
                    .event(keysym, pressed, |code, extended, down| {
                        output.push((code, extended, down));
                        Ok(())
                    })
                    .unwrap();
            }
            assert_eq!(
                output,
                [
                    (0x14, false, true),
                    (0x12, false, true),
                    (0x1b, false, true),
                    (0x1b, false, false),
                    (0x12, false, false),
                    (0x14, false, false),
                    (0x1f, true, true),
                    (0x2d, false, true),
                    (0x2d, false, false),
                    (0x1f, true, false),
                ]
            );
        }
    }

    fn sequence(events: &[(u32, bool)]) -> Vec<(u8, bool, bool)> {
        let mut keyboard = Keyboard::default();
        let mut output = Vec::new();
        for &(keysym, pressed) in events {
            keyboard
                .event(keysym, pressed, |code, extended, pressed| {
                    output.push((code, extended, pressed));
                    Ok(())
                })
                .unwrap();
        }
        output
    }

    #[test]
    fn printable_keysyms_supply_missing_shift_and_preserve_repeats() {
        assert_eq!(
            sequence(&[
                (u32::from('A'), true),
                (u32::from('A'), true),
                (u32::from('A'), false),
                (u32::from('!'), true),
                (u32::from('!'), false),
                (u32::from(':'), true),
                (u32::from(':'), false),
            ]),
            [
                (0x12, false, true),
                (0x1c, false, true),
                (0x1c, false, true),
                (0x1c, false, false),
                (0x12, false, false),
                (0x12, false, true),
                (0x16, false, true),
                (0x16, false, false),
                (0x12, false, false),
                (0x12, false, true),
                (0x4c, false, true),
                (0x4c, false, false),
                (0x12, false, false),
            ]
        );
    }

    #[test]
    fn explicit_right_shift_is_restored_after_unshifted_character() {
        assert_eq!(
            sequence(&[
                (0xffe2, true),
                (u32::from('a'), true),
                (u32::from('a'), false),
                (u32::from('B'), true),
                (u32::from('B'), false),
                (0xffe2, false),
            ]),
            [
                (0x59, false, true),
                (0x59, false, false),
                (0x1c, false, true),
                (0x1c, false, false),
                (0x59, false, true),
                (0x32, false, true),
                (0x32, false, false),
                (0x59, false, false),
            ]
        );
    }

    #[test]
    fn two_explicit_shifts_remain_independent() {
        assert_eq!(
            sequence(&[
                (0xffe1, true),
                (0xffe2, true),
                (0xffe1, false),
                (u32::from('?'), true),
                (u32::from('?'), false),
                (0xffe2, false),
            ]),
            [
                (0x12, false, true),
                (0x59, false, true),
                (0x12, false, false),
                (0x4a, false, true),
                (0x4a, false, false),
                (0x59, false, false),
            ]
        );
    }

    #[test]
    fn overlapping_characters_keep_shift_until_the_last_release() {
        assert_eq!(
            sequence(&[
                (u32::from('A'), true),
                (u32::from('B'), true),
                (u32::from('a'), false),
                (u32::from('B'), false),
            ]),
            [
                (0x12, false, true),
                (0x1c, false, true),
                (0x32, false, true),
                (0x1c, false, false),
                (0x32, false, false),
                (0x12, false, false),
            ]
        );
    }

    #[test]
    fn unicode_ascii_alias_and_windows_shortcut_preserve_physical_keys() {
        assert_eq!(
            sequence(&[
                (0x0100_0041, true),
                (0x0100_0041, false),
                (0xffeb, true),
                (u32::from('r'), true),
                (u32::from('r'), false),
                (0xffeb, false),
                (0x0100_20ac, true),
                (0x0100_20ac, false),
            ]),
            [
                (0x12, false, true),
                (0x1c, false, true),
                (0x1c, false, false),
                (0x12, false, false),
                (0x1f, true, true),
                (0x2d, false, true),
                (0x2d, false, false),
                (0x1f, true, false),
            ]
        );
    }
    #[test]
    fn text_modifiers_navigation_and_special_keys() {
        assert_eq!(set2(u32::from('A')), set2(u32::from('a')));
        assert_eq!(set2(0xffe4), Some((0x14, true)));
        assert_eq!(set2(0xffff), Some((0x71, true)));
        assert_eq!(set2(0xff13), Some((0xe1, false)));
        assert_eq!(set2(0xff61), Some((0xe0, false)));
        assert_eq!(set2(0x10020ac), None);
        for (keysym, code, extended) in [
            (0xffaa, 0x7c, false),
            (0xffab, 0x79, false),
            (0xffad, 0x7b, false),
            (0xffaf, 0x4a, true),
            (0xff8d, 0x5a, true),
        ] {
            assert_eq!(
                sequence(&[(keysym, true), (keysym, false)]),
                [(code, extended, true), (code, extended, false)]
            );
        }
    }
}
