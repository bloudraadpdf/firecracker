// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0
mod engine;
mod policy;
mod ppi;
mod service;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 4 && args.len() != 5 {
        return Err(
            "usage: firecracker-uefi-vars serve STORE SOCKET | provision STORE SEED_JSON | \
             seed-microsoft x64|arm64 SOURCE_DIRECTORY OUTPUT_JSON"
                .into(),
        );
    }
    match args[1].to_str() {
        Some("serve") if args.len() == 4 => service::serve(args[2].as_ref(), args[3].as_ref())?,
        Some("provision") if args.len() == 4 => {
            service::provision(args[2].as_ref(), args[3].as_ref())?
        }
        Some("seed-microsoft") if args.len() == 5 => policy::prepare(
            args[3].as_ref(),
            args[4].as_ref(),
            args[2]
                .to_str()
                .ok_or("architecture is not UTF-8")?
                .parse()?,
        )?,
        _ => return Err("unknown command".into()),
    }
    Ok(())
}
