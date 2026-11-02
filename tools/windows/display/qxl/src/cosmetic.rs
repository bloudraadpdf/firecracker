// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Cosmetic-line style conversion from XDDM `GetCosmeticAttr`.

const MAX_SEGMENTS: usize = 100;
const FIXED_ONE: i32 = 16;

#[derive(Clone, Copy, Debug)]
pub enum CosmeticStyle<'a> {
    Solid,
    Alternate { start_gap: bool },
    Custom { lengths: &'a [i32], start_gap: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleError {
    InvalidDensity,
    InvalidLength,
    InvalidCount,
}

#[derive(Debug, PartialEq, Eq)]
pub struct LineStyle {
    lengths: [i32; MAX_SEGMENTS],
    count: u8,
    start_gap: bool,
}

impl LineStyle {
    pub fn new(style: CosmeticStyle<'_>, density: i32) -> Result<Self, StyleError> {
        let mut result = Self {
            lengths: [0; MAX_SEGMENTS],
            count: 0,
            start_gap: false,
        };
        match style {
            CosmeticStyle::Solid => {}
            CosmeticStyle::Alternate { start_gap } => {
                result.lengths[..2].fill(FIXED_ONE);
                result.count = 2;
                result.start_gap = start_gap;
            }
            CosmeticStyle::Custom { lengths, start_gap } => {
                if density <= 0 {
                    return Err(StyleError::InvalidDensity);
                }
                if lengths.is_empty() || lengths.len() > MAX_SEGMENTS {
                    return Err(StyleError::InvalidCount);
                }
                for (output, &length) in result.lengths.iter_mut().zip(lengths) {
                    if length <= 0 {
                        return Err(StyleError::InvalidLength);
                    }
                    *output = length
                        .checked_mul(density)
                        .and_then(|length| length.checked_mul(FIXED_ONE))
                        .ok_or(StyleError::InvalidLength)?;
                }
                result.count = u8::try_from(lengths.len()).map_err(|_| StyleError::InvalidCount)?;
                result.start_gap = start_gap;
            }
        }
        Ok(result)
    }

    /// QXL style lengths in signed Q28.4 pixels.
    pub fn lengths(&self) -> &[i32] {
        &self.lengths[..usize::from(self.count)]
    }

    /// QXL uses the GDI LA_STYLED and LA_STARTGAP flag values.
    pub fn flags(&self) -> u8 {
        if self.count == 0 {
            0
        } else {
            8 | if self.start_gap { 4 } else { 0 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(lengths: &[i32], density: i32) -> Result<LineStyle, StyleError> {
        LineStyle::new(
            CosmeticStyle::Custom {
                lengths,
                start_gap: false,
            },
            density,
        )
    }

    #[test]
    fn gdi_dash_and_dot_density() {
        // GDI advertises denStyleStep=3: PS_DASH is 18/6 pixels, PS_DOT is 3/3.
        assert_eq!(custom(&[6, 2], 3).unwrap().lengths(), &[288, 96]);
        assert_eq!(custom(&[1; 8], 3).unwrap().lengths(), &[48; 8]);
    }

    #[test]
    fn alternate_is_one_pixel_and_preserves_start_gap() {
        let style = LineStyle::new(CosmeticStyle::Alternate { start_gap: true }, 3).unwrap();
        assert_eq!(style.lengths(), &[16, 16]);
        assert_eq!(style.flags(), 12);
        assert_eq!(custom(&[1, 1], 3).unwrap().flags(), 8);
        let solid = LineStyle::new(CosmeticStyle::Solid, 3).unwrap();
        assert!(solid.lengths().is_empty());
        assert_eq!(solid.flags(), 0);
    }

    #[test]
    fn rejects_invalid_styles_without_overflow() {
        for density in [i32::MIN, -1, 0] {
            assert_eq!(custom(&[1], density), Err(StyleError::InvalidDensity));
        }
        for lengths in [&[][..], &[1; 101][..]] {
            assert_eq!(custom(lengths, 3), Err(StyleError::InvalidCount));
        }
        assert_eq!(custom(&[1; 100], 3).unwrap().lengths().len(), 100);
        for length in [i32::MIN, -1, 0, i32::MAX / 48 + 1, i32::MAX] {
            assert_eq!(custom(&[length], 3), Err(StyleError::InvalidLength));
        }
        let limit = i32::MAX / 48;
        assert_eq!(custom(&[limit], 3).unwrap().lengths(), &[limit * 48]);
        assert_eq!(custom(&[1], i32::MAX), Err(StyleError::InvalidLength));
    }
}
