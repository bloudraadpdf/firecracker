// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Emit the expected cursor-free RGB frame before a mode probe's F1 confirmation.

pub mod model;

use std::io::{self, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [case] = args.as_slice() else {
        return Err("usage: frame CASE_INDEX".into());
    };
    let case: u8 = case.parse()?;
    let view = model::View {
        case,
        phase: model::Phase::Pixels,
        pointers: 0,
        text: 0,
        error: None,
        cleanup_error: false,
    };
    let mode = view.mode().ok_or("case index exceeds the mode table")?;
    let mut output = io::BufWriter::new(io::stdout().lock());
    write!(output, "P6\n{} {}\n255\n", mode.width, mode.height)?;
    for y in 0..mode.height {
        for x in 0..mode.width {
            let pixel = model::pixel(view, x, y).ok_or("invalid pixel coordinates")?;
            output.write_all(&pixel.to_be_bytes()[1..])?;
        }
    }
    output.flush()?;
    Ok(())
}
