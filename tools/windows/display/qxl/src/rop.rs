// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! ROP2/ROP3 selection from xddm/display/rop.c, without the 256-entry table.

const INV_SRC: u16 = 1;
const INV_BRUSH: u16 = 2;
const INV_DEST: u16 = 4;
const PUT: u16 = 8;
const OR: u16 = 16;
const AND: u16 = 32;
const XOR: u16 = 64;
const BLACK: u16 = 128;
const WHITE: u16 = 256;
const INVERT: u16 = 512;
const INV_RESULT: u16 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Effect {
    Blend = 0,
    Opaque = 1,
    RevertOnDuplicate = 2,
    NopOnDuplicate = 5,
    Nop = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Blackness,
    Whiteness,
    Invert,
    Nop,
    Fill(u16),
    Copy(u16),
    Blend(u16),
    Opaque(u16),
    Ternary(u8),
}

/// GDI binary operation numbers are 1..=16, not ROP3 truth-table bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rop2(u8);

impl Rop2 {
    pub const fn new(number: u8) -> Option<Self> {
        if number >= 1 && number <= 16 {
            Some(Self(number - 1))
        } else {
            None
        }
    }

    pub const fn descriptor(self) -> u16 {
        [
            BLACK,
            OR | INV_RESULT,
            INV_BRUSH | AND,
            INV_BRUSH | PUT,
            INV_DEST | AND,
            INVERT,
            XOR,
            AND | INV_RESULT,
            AND,
            XOR | INV_RESULT,
            0,
            INV_BRUSH | OR,
            PUT,
            INV_DEST | OR,
            OR,
            WHITE,
        ][self.0 as usize]
    }

    pub const fn effect(self) -> Effect {
        match self.0 {
            0 | 3 | 12 | 15 => Effect::Opaque,
            1 | 4 | 7 | 9 | 13 => Effect::Blend,
            2 | 8 | 11 | 14 => Effect::NopOnDuplicate,
            5 | 6 => Effect::RevertOnDuplicate,
            10 => Effect::Nop,
            _ => unreachable!(),
        }
    }

    pub const fn uses_brush(self) -> bool {
        self.0 & 3 != self.0 >> 2
    }

    pub const fn uses_destination(self) -> bool {
        self.0 & 5 != (self.0 >> 1) & 5
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inputs {
    pub destination: bool,
    pub source: bool,
    pub brush: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rop3 {
    method: Method,
    inputs: Inputs,
    effect: Effect,
}

impl Rop3 {
    pub const fn new(truth: u8) -> Self {
        let inputs = Inputs {
            destination: truth & 0x55 != (truth >> 1) & 0x55,
            source: truth & 0x33 != (truth >> 2) & 0x33,
            brush: truth & 0x0f != truth >> 4,
        };
        let (method, effect) = if !inputs.source {
            let binary = Rop2((truth & 3) | ((truth >> 2) & 12));
            let method = match binary.0 {
                0 => Method::Blackness,
                5 => Method::Invert,
                10 => Method::Nop,
                15 => Method::Whiteness,
                _ => Method::Fill(binary.descriptor()),
            };
            (method, binary.effect())
        } else if !inputs.brush {
            let binary = Rop2(truth & 15);
            let descriptor = replace_flag(binary.descriptor(), INV_BRUSH, INV_SRC);
            let method = if inputs.destination {
                Method::Blend(descriptor)
            } else {
                Method::Copy(descriptor)
            };
            (method, binary.effect())
        } else if !inputs.destination {
            let binary =
                Rop2((truth & 1) | ((truth >> 1) & 2) | ((truth >> 2) & 4) | ((truth >> 3) & 8));
            let descriptor = replace_flag(binary.descriptor(), INV_DEST, INV_SRC);
            (Method::Opaque(descriptor), Effect::Opaque)
        } else {
            (Method::Ternary(truth), Effect::Blend)
        };
        Self {
            method,
            inputs,
            effect,
        }
    }

    pub const fn method(self) -> Method {
        self.method
    }

    pub const fn inputs(self) -> Inputs {
        self.inputs
    }

    pub const fn effect(self) -> Effect {
        self.effect
    }

    pub const fn effect_with_mask(self, masked: bool) -> Effect {
        if masked { Effect::Blend } else { self.effect }
    }
}

const fn replace_flag(value: u16, from: u16, to: u16) -> u16 {
    if value & from != 0 {
        (value & !from) | to
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(value: u16, source: bool, destination: bool, brush: bool, operand: bool) -> bool {
        let source = source ^ (value & INV_SRC != 0);
        let brush = brush ^ (value & INV_BRUSH != 0);
        let destination = destination ^ (value & INV_DEST != 0);
        let operand = if operand { brush } else { source };
        let result = match value & (PUT | OR | AND | XOR | BLACK | WHITE | INVERT) {
            PUT => operand,
            OR => operand | destination,
            AND => operand & destination,
            XOR => operand ^ destination,
            BLACK => false,
            WHITE => true,
            INVERT => !destination,
            0 => destination,
            _ => panic!("invalid descriptor"),
        };
        result ^ (value & INV_RESULT != 0)
    }

    fn evaluate(method: Method, index: u8) -> bool {
        let destination = index & 1 != 0;
        let source = index & 2 != 0;
        let brush = index & 4 != 0;
        match method {
            Method::Blackness => false,
            Method::Whiteness => true,
            Method::Invert => !destination,
            Method::Nop => destination,
            Method::Fill(value) => descriptor(value, source, destination, brush, true),
            Method::Copy(value) | Method::Blend(value) => {
                descriptor(value, source, destination, brush, false)
            }
            Method::Opaque(value) => descriptor(
                value & !INV_SRC,
                false,
                source ^ (value & INV_SRC != 0),
                brush,
                true,
            ),
            Method::Ternary(truth) => truth & (1 << index) != 0,
        }
    }

    #[test]
    fn all_ternary_methods_match_the_eight_input_truth_table() {
        for truth in 0..=255 {
            let operation = Rop3::new(truth);
            for index in 0..8 {
                assert_eq!(
                    evaluate(operation.method, index),
                    truth & (1 << index) != 0,
                    "ROP3={truth:02x}, inputs={index}"
                );
                for (bit, required) in [
                    (1, operation.inputs.destination),
                    (2, operation.inputs.source),
                    (4, operation.inputs.brush),
                ] {
                    if !required {
                        assert_eq!(
                            evaluate(operation.method, index),
                            evaluate(operation.method, index ^ bit)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn all_binary_descriptors_match_gdi_numbering() {
        assert_eq!(Rop2::new(0), None);
        assert_eq!(Rop2::new(17), None);
        for truth in 0..16 {
            let operation = Rop2::new(truth + 1).unwrap();
            for index in 0..4 {
                assert_eq!(
                    descriptor(
                        operation.descriptor(),
                        false,
                        index & 1 != 0,
                        index & 2 != 0,
                        true
                    ),
                    truth & (1 << index) != 0
                );
            }
        }
    }

    fn check_effect(effect: Effect, original: bool, first: bool, second: bool) {
        match effect {
            Effect::RevertOnDuplicate => assert_eq!(second, original),
            Effect::NopOnDuplicate => assert_eq!(second, first),
            Effect::Nop => assert_eq!(first, original),
            Effect::Blend | Effect::Opaque => {}
        }
    }

    #[test]
    fn duplicate_hints_hold_for_every_input_combination() {
        for truth in 0..=255 {
            let operation = Rop3::new(truth);
            for index in 0..8 {
                let first = evaluate(operation.method, index);
                let repeated = (index & !1) | u8::from(first);
                check_effect(
                    operation.effect,
                    index & 1 != 0,
                    first,
                    evaluate(operation.method, repeated),
                );
            }
        }
        for number in 1..=16 {
            let operation = Rop2::new(number).unwrap();
            for index in 0..4 {
                let original = index & 1 != 0;
                let brush = index & 2 != 0;
                let first = descriptor(operation.descriptor(), false, original, brush, true);
                let second = descriptor(operation.descriptor(), false, first, brush, true);
                check_effect(operation.effect(), original, first, second);
                if !operation.uses_destination() {
                    assert_eq!(
                        first,
                        descriptor(operation.descriptor(), false, !original, brush, true)
                    );
                }
                if !operation.uses_brush() {
                    assert_eq!(
                        first,
                        descriptor(operation.descriptor(), false, original, !brush, true)
                    );
                }
            }
        }
    }

    #[test]
    fn common_rops_use_the_original_specialised_commands_and_effects() {
        for (truth, method, effect) in [
            (0x00, Method::Blackness, Effect::Opaque),
            (0xff, Method::Whiteness, Effect::Opaque),
            (0x55, Method::Invert, Effect::RevertOnDuplicate),
            (0xaa, Method::Nop, Effect::Nop),
            (0xcc, Method::Copy(PUT), Effect::Opaque),
            (0xf0, Method::Fill(PUT), Effect::Opaque),
            (0x66, Method::Blend(XOR), Effect::RevertOnDuplicate),
            (0x88, Method::Blend(AND), Effect::NopOnDuplicate),
            (0xc0, Method::Opaque(AND), Effect::Opaque),
            (0x96, Method::Ternary(0x96), Effect::Blend),
        ] {
            let operation = Rop3::new(truth);
            assert_eq!((operation.method, operation.effect), (method, effect));
            assert_eq!(operation.effect_with_mask(true), Effect::Blend);
            assert_eq!(operation.effect_with_mask(false), effect);
        }
    }
}
