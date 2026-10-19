// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io;

fn run() -> io::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let address = args.next().ok_or_else(|| {
        io::Error::other("usage: firecracker-viewer ADDRESS [--password-file PATH]")
    })?;
    if address == "--help" || address == "-h" {
        println!(
            "Usage: firecracker-viewer ADDRESS [--password-file PATH]\n\nConnect to a loopback \
             RFB bridge. Use SSH forwarding for remote hosts.\nCtrl+Alt+Home: Windows key; \
             Ctrl+Alt+End: Ctrl+Alt+Del; Ctrl+Alt+Enter: fullscreen.\nClosing the viewer leaves \
             the VM running."
        );
        return Ok(());
    }
    let address = address
        .to_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| io::Error::other("invalid IP:port address"))?;
    let password_file = match args.next() {
        None => None,
        Some(flag) if flag == "--password-file" => Some(
            args.next()
                .ok_or_else(|| io::Error::other("missing password file"))?
                .into(),
        ),
        Some(_) => return Err(io::Error::other("unexpected argument")),
    };
    if args.next().is_some() {
        return Err(io::Error::other("unexpected argument"));
    }
    console::view(console::ViewerOptions {
        address,
        password_file,
    })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Console: {error}");
        std::process::exit(1);
    }
}
