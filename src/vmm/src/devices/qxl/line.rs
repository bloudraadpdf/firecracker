// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! One-pixel cosmetic lines with Q28.4 endpoints and GDI endpoint ownership.

use std::cmp::Ordering;

use super::protocol::DecodeError;

#[derive(Clone, Copy, Debug)]
struct Parameter {
    numerator: i64,
    denominator: i64,
}

impl Parameter {
    const ZERO: Self = Self {
        numerator: 0,
        denominator: 1,
    };
    const ONE: Self = Self {
        numerator: 1,
        denominator: 1,
    };

    fn new(numerator: i64, denominator: i64) -> Self {
        if denominator < 0 {
            Self {
                numerator: -numerator,
                denominator: -denominator,
            }
        } else {
            Self {
                numerator,
                denominator,
            }
        }
    }

    fn compare(self, other: Self) -> Ordering {
        (i128::from(self.numerator) * i128::from(other.denominator))
            .cmp(&(i128::from(other.numerator) * i128::from(self.denominator)))
    }
}

#[derive(Debug)]
struct Line {
    start: [i64; 2],
    end: [i64; 2],
    delta: [i64; 2],
}

impl Line {
    fn new(start: [i32; 2], end: [i32; 2]) -> Self {
        let start = start.map(i64::from);
        let end = end.map(i64::from);
        Self {
            start,
            end,
            delta: [end[0] - start[0], end[1] - start[1]],
        }
    }

    fn corner(&self, point: [i64; 2]) -> Option<Parameter> {
        let [dx, dy] = self.delta;
        let [x, y] = [point[0] - self.start[0], point[1] - self.start[1]];
        if i128::from(x) * i128::from(dy) != i128::from(y) * i128::from(dx) {
            return None;
        }
        let parameter = if dx == 0 {
            Parameter::new(y, dy)
        } else {
            Parameter::new(x, dx)
        };
        (parameter.compare(Parameter::ZERO) != Ordering::Less
            && parameter.compare(Parameter::ONE) == Ordering::Less)
            .then_some(parameter)
    }

    fn covers(&self, x: u32, y: u32) -> bool {
        let [dx, dy] = self.delta;
        let [x, y] = [i64::from(x) * 16, i64::from(y) * 16];
        if let Some(parameter) = self.corner([x + 8, y]) {
            let outgoing = dx + dy.abs();
            if (outgoing > 0 || outgoing == 0 && dy < 0)
                && (dy.abs() > dx.abs()
                    || dy.abs() < dx.abs() && parameter.compare(Parameter::ZERO) == Ordering::Equal)
            {
                return true;
            }
        }
        if dy + dx.abs() > 0 && self.corner([x, y + 8]).is_some() {
            return true;
        }

        let mut entry = (Parameter::ZERO, true);
        let mut exit: Option<(Parameter, bool)> = None;
        // In diagonal coordinates each pixel diamond becomes two intervals.
        for (origin, delta, center, minimum_included, maximum_included) in [
            (self.start[0] + self.start[1], dx + dy, x + y, false, true),
            (self.start[0] - self.start[1], dx - dy, x - y, true, false),
        ] {
            let minimum = center - 8;
            let maximum = center + 8;
            if delta == 0 {
                if origin < minimum
                    || origin > maximum
                    || origin == minimum && !minimum_included
                    || origin == maximum && !maximum_included
                {
                    return false;
                }
                continue;
            }
            let mut lower = (Parameter::new(minimum - origin, delta), minimum_included);
            let mut upper = (Parameter::new(maximum - origin, delta), maximum_included);
            if delta < 0 {
                std::mem::swap(&mut lower, &mut upper);
            }
            match lower.0.compare(entry.0) {
                Ordering::Greater => entry = lower,
                Ordering::Equal => entry.1 &= lower.1,
                Ordering::Less => (),
            }
            match exit.as_mut() {
                Some(exit) => match upper.0.compare(exit.0) {
                    Ordering::Less => *exit = upper,
                    Ordering::Equal => exit.1 &= upper.1,
                    Ordering::Greater => (),
                },
                None => exit = Some(upper),
            }
        }
        let Some(exit) = exit else {
            return false;
        };
        if exit.0.compare(Parameter::ZERO) != Ordering::Greater
            || exit.0.compare(Parameter::ONE) == Ordering::Greater
        {
            return false;
        }
        // GDI excludes the final right/bottom corner; diagonal ties also exclude
        // the final left corner.
        if exit.0.compare(Parameter::ONE) == Ordering::Equal
            && (self.end == [x + 8, y]
                || self.end == [x, y + 8]
                || dx.abs() == dy.abs() && self.end == [x - 8, y])
        {
            return false;
        }
        match entry.0.compare(exit.0) {
            Ordering::Less => true,
            Ordering::Equal => entry.1 && exit.1,
            Ordering::Greater => false,
        }
    }
}

/// Rasterise a cosmetic line into a bounded surface without rounding endpoints.
/// Returned pixels are unique and ordered along the positive major axis.
pub fn rasterize(
    start: [i32; 2],
    end: [i32; 2],
    width: u32,
    height: u32,
) -> Result<Vec<[u32; 2]>, DecodeError> {
    if width > 4096 || height > 4096 {
        return Err(DecodeError::Parameter);
    }
    let line = Line::new(start, end);
    if line.delta == [0, 0] || width == 0 || height == 0 {
        return Ok(Vec::new());
    }
    let major_axis = usize::from(line.delta[1].abs() > line.delta[0].abs());
    let minor_axis = 1 - major_axis;
    let dimensions = [width, height];
    let denominator = i128::from(line.delta[major_axis]) * 16;
    let mut pixels = Vec::new();
    for major in 0..dimensions[major_axis] {
        let numerator = i128::from(line.start[minor_axis]) * i128::from(line.delta[major_axis])
            + (i128::from(major) * 16 - i128::from(line.start[major_axis]))
                * i128::from(line.delta[minor_axis]);
        let minor = if denominator < 0 {
            (-numerator).div_euclid(-denominator)
        } else {
            numerator.div_euclid(denominator)
        };
        // A major-axis step intersects at most two candidate pixel diamonds.
        for minor in [minor, minor + 1] {
            let Ok(minor) = u32::try_from(minor) else {
                continue;
            };
            if minor < dimensions[minor_axis] {
                let pixel = if major_axis == 0 {
                    [major, minor]
                } else {
                    [minor, major]
                };
                if line.covers(pixel[0], pixel[1]) {
                    pixels.push(pixel);
                }
            }
        }
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Reference {
        start: [i32; 2],
        end: [i32; 2],
        rows: [u16; 16],
    }

    #[test]
    fn matches_native_gdi_fractional_endpoints() {
        let cases: Vec<Reference> =
            serde_json::from_str(include_str!("../../../../../tests/data/qxl/gdi-lines.json"))
                .unwrap();
        assert_eq!(cases.len(), 778);
        for case in cases {
            let mut rows = [0u16; 16];
            for [x, y] in rasterize(case.start, case.end, 16, 16).unwrap() {
                rows[y as usize] |= 1 << x;
            }
            assert_eq!(rows, case.rows, "{:?} -> {:?}", case.start, case.end);
        }
    }

    #[test]
    fn clips_extreme_coordinates_without_overflow() {
        assert_eq!(
            rasterize([i32::MIN; 2], [i32::MAX; 2], 4, 4).unwrap(),
            vec![[0, 0], [1, 1], [2, 2], [3, 3]]
        );
        assert!(
            rasterize([i32::MAX, i32::MIN], [i32::MAX - 1, i32::MAX], 4, 4)
                .unwrap()
                .is_empty()
        );
        assert!(rasterize([8, 8], [8, 8], 4, 4).unwrap().is_empty());
        assert_eq!(
            rasterize([0, 0], [16, 16], 4097, 1).unwrap_err(),
            DecodeError::Parameter
        );
    }
}
