// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

fn main() {
    if let Err(error) = console::run(std::env::args_os().skip(1)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
