// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io::{Error, ErrorKind};

mod xp_pattern;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("expected data file")?;
    let seed = args.next().ok_or("expected transcript seed")?.parse()?;
    let data = std::fs::read(path)?;
    if data.len() != 65536
        || data
            .iter()
            .enumerate()
            .any(|(offset, byte)| *byte != xp_pattern::storage_pattern(seed, offset))
    {
        return Err(Error::new(ErrorKind::InvalidData, "XP storage pattern mismatch").into());
    }
    println!("Verified all 65536 bytes against transcript seed {seed}");
    Ok(())
}
