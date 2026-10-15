# Windows test disks

Use an authorised, already-installed raw disk with matching virtio drivers. This
revision boots prepared disks; it does not provide graphical installation. Keep
an original copy, and fully shut Windows down before copying its disk.

## Build prerequisites

Run the commands from the Firecracker repository root on Linux. Use upstream's
Rustup toolchain from `rust-toolchain.toml`; it currently selects Rust 1.98.1.
Put the Rustup proxies first on `PATH`, including when another Rust installation
is present:

```sh
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
rustup show active-toolchain
```

For the standalone crates' formatting and Clippy checks, install the components:

```sh
rustup component add rustfmt clippy
```

For Windows probe builds, also install their target libraries:

```sh
rustup target add i686-pc-windows-msvc x86_64-pc-windows-msvc
```

The
[upstream developer tool](../../CONTRIBUTING.md#contribution-quality-standards)
provides the normal build and style checks. Each tool's README gives its build
commands and additional dependencies.
