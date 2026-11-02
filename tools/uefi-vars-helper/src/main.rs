// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0
mod engine;
mod policy;
mod ppi;
mod service;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 4 {
        return Err(
            "usage: firecracker-uefi-vars serve STORE SOCKET | provision STORE SEED_JSON | seed-microsoft SOURCE_DIRECTORY OUTPUT_JSON".into(),
        );
    }
    match args[1].to_str() {
        Some("serve") => service::serve(args[2].as_ref(), args[3].as_ref())?,
        Some("provision") => service::provision(args[2].as_ref(), args[3].as_ref())?,
        Some("seed-microsoft") => policy::prepare(args[2].as_ref(), args[3].as_ref())?,
        _ => return Err("unknown command".into()),
    }
    Ok(())
}
