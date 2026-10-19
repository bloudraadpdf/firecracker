// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::protocol::DecodeError;

#[derive(Clone, Copy, Debug)]
pub enum Operand {
    Source,
    Brush,
}

#[derive(Clone, Copy, Debug)]
pub struct RasterOp {
    descriptor: u16,
    operand: Operand,
}

impl RasterOp {
    pub fn uses_operand(self) -> bool {
        self.descriptor & 0x78 != 0
    }

    pub fn uses_destination(self) -> bool {
        self.descriptor & 0x270 != 0
    }

    pub fn decode(descriptor: u16, operand: Operand) -> Result<Self, DecodeError> {
        if descriptor & !0x7ff != 0 || (descriptor & 0x3f8).count_ones() != 1 {
            return Err(DecodeError::Parameter);
        }
        Ok(Self {
            descriptor,
            operand,
        })
    }

    pub fn apply(self, source: u32, brush: u32, destination: u32) -> u32 {
        let operand = match self.operand {
            Operand::Source => {
                source
                    ^ if self.descriptor & 1 != 0 {
                        u32::MAX
                    } else {
                        0
                    }
            }
            Operand::Brush => {
                brush
                    ^ if self.descriptor & 2 != 0 {
                        u32::MAX
                    } else {
                        0
                    }
            }
        };
        let destination = destination
            ^ if self.descriptor & 4 != 0 {
                u32::MAX
            } else {
                0
            };
        self.evaluate(operand, destination)
    }

    pub fn apply_opaque(self, source: u32, brush: u32) -> u32 {
        let source = source
            ^ if self.descriptor & 1 != 0 {
                u32::MAX
            } else {
                0
            };
        let brush = brush
            ^ if self.descriptor & 2 != 0 {
                u32::MAX
            } else {
                0
            };
        self.evaluate(source, brush)
    }

    fn evaluate(self, operand: u32, destination: u32) -> u32 {
        let result = match self.descriptor & 0x3f8 {
            8 => operand,
            16 => operand | destination,
            32 => operand & destination,
            64 => operand ^ destination,
            128 => 0,
            256 => u32::MAX,
            512 => !destination,
            _ => unreachable!(),
        };
        result
            ^ if self.descriptor & 1024 != 0 {
                u32::MAX
            } else {
                0
            }
    }
}

pub fn ternary(opcode: u8, source: u32, brush: u32, destination: u32) -> u32 {
    let mut result = 0;
    for index in 0..8 {
        if opcode & (1 << index) == 0 {
            continue;
        }
        let source = if index & 2 != 0 { source } else { !source };
        let brush = if index & 4 != 0 { brush } else { !brush };
        let destination = if index & 1 != 0 {
            destination
        } else {
            !destination
        };
        result |= source & brush & destination;
    }
    result
}

pub fn alpha_over(source: u32, destination: u32, alpha: u8, destination_has_alpha: bool) -> u32 {
    let multiply = |value: u32, alpha: u32| (value * alpha + 127) / 255;
    let alpha = u32::from(alpha);
    let source_alpha = multiply(source >> 24, alpha);
    let mut result = if destination_has_alpha {
        (source_alpha + multiply(destination >> 24, 255 - source_alpha)).min(255) << 24
    } else {
        0xff00_0000
    };
    for shift in [0, 8, 16] {
        let source = multiply((source >> shift) & 255, alpha);
        let destination = multiply((destination >> shift) & 255, 255 - source_alpha);
        result |= (source + destination).min(255) << shift;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ternary_operations_cover_every_bit_truth_table() {
        for opcode in 0..=u8::MAX {
            for inputs in 0..8 {
                let word = |set| if set { u32::MAX } else { 0 };
                assert_eq!(
                    ternary(
                        opcode,
                        word(inputs & 2 != 0),
                        word(inputs & 4 != 0),
                        word(inputs & 1 != 0)
                    ),
                    word(opcode & (1 << inputs) != 0)
                );
            }
        }
    }

    #[test]
    fn opaque_operations_use_the_brush_and_its_own_inversion_flag() {
        let source = 0x1234_5678;
        let brush = 0xabcd_0123;
        let op = RasterOp::decode(64 | 1 | 2 | 4 | 1024, Operand::Source).unwrap();
        assert_eq!(op.apply_opaque(source, brush), !(source ^ brush));
        for (descriptor, source_used, brush_used) in [
            (8, true, false),
            (16, true, true),
            (128, false, false),
            (256, false, false),
            (512, false, true),
        ] {
            let op = RasterOp::decode(descriptor, Operand::Source).unwrap();
            assert_eq!(op.uses_operand(), source_used);
            assert_eq!(op.uses_destination(), brush_used);
        }
    }

    #[test]
    fn premultiplied_alpha_combines_pixel_and_constant_alpha() {
        assert_eq!(alpha_over(0x8040_0000, 0x8000_0080, 0, true), 0x8000_0080);
        assert_eq!(alpha_over(0x8040_0000, 0x8000_0080, 128, true), 0xa020_0060);
        assert_eq!(alpha_over(0x8040_0000, 0x8000_0080, 255, true), 0xc040_0040);
        assert_eq!(
            alpha_over(0xff20_4060, 0xff60_4020, 128, false),
            0xff40_4040
        );
        assert_eq!(
            alpha_over(0x8040_0000, 0x8000_0080, 128, false),
            0xff20_0060
        );
    }

    #[test]
    fn descriptors_require_one_operation_and_only_defined_flags() {
        for descriptor in 0..=u16::MAX {
            let valid = descriptor & !0x7ff == 0 && (descriptor & 0x3f8).count_ones() == 1;
            assert_eq!(RasterOp::decode(descriptor, Operand::Source).is_ok(), valid);
        }
    }

    #[test]
    fn all_operation_and_inversion_combinations_match_bit_truth_tables() {
        for brush_operand in [false, true] {
            for operation in 3..=9 {
                for inversion in 0..16 {
                    let descriptor = (1 << operation) | (inversion & 7) | ((inversion & 8) << 7);
                    let operand = if brush_operand {
                        Operand::Brush
                    } else {
                        Operand::Source
                    };
                    let op = RasterOp::decode(descriptor, operand).unwrap();
                    for source in [false, true] {
                        for brush in [false, true] {
                            for destination in [false, true] {
                                let input = if brush_operand {
                                    brush ^ (inversion & 2 != 0)
                                } else {
                                    source ^ (inversion & 1 != 0)
                                };
                                let dest = destination ^ (inversion & 4 != 0);
                                let expected = match operation {
                                    3 => input,
                                    4 => input || dest,
                                    5 => input && dest,
                                    6 => input ^ dest,
                                    7 => false,
                                    8 => true,
                                    9 => !dest,
                                    _ => unreachable!(),
                                } ^ (inversion & 8 != 0);
                                let word = |value| if value { u32::MAX } else { 0 };
                                assert_eq!(
                                    op.apply(word(source), word(brush), word(destination)),
                                    word(expected)
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
