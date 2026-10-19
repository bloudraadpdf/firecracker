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

#[cfg(test)]
mod tests {
    use super::*;

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
