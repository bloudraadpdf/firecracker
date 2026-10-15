# Prepare a Windows test disk

`prepare.rs` creates a private output directory, a sparse 64 GiB raw disk and a
Server Core answer file. `prepare-nt6.rs` adds that file and the signed storage
and network drivers to the installation ISO. Installation runs directly in
Firecracker. No QEMU binary or library is required. Existing output directories
are rejected so preparation cannot overwrite a previous disk.

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

## Prepare Server Core

Prerequisites: the repository's Rustup toolchain, `7z`, `genisoimage`,
`sha256sum`, the pinned Windows Server 2025 ISO and virtio-win ISO, and the
[graphical firmware](../../docs/windows-console.md). Check the Server Core image
index against `sources/install.wim` before generating an answer file.

```sh
mkdir -p build/windows
rustc --edition 2024 -D warnings tools/windows/prepare.rs -o build/windows/prepare
rustc --edition 2024 -D warnings tools/windows/prepare-nt6.rs -o build/windows/prepare-nt6
build/windows/prepare build/windows/server-core 1
build/windows/prepare-nt6 server2025 windows-server-2025.iso virtio-win.iso \
  build/windows/server-core-media build/windows/server-core/Autounattend.xml
```

Attach the generated `server2025-viostor.iso` as the console's optical image and
`server-core/windows.raw` as the writable virtio disk, following the
[direct installation guide](../../docs/windows-console.md). Setup reads the
answer file from the same DVD as the drivers. The answer file erases disk 0,
creates GPT partitions, installs the selected image and signed drivers, sets
UTC, disables hibernation and enables EMS/SAC after installing the fallback EFI
path. Use only the new disposable disk. Firecracker reset exits the process;
restart with the same disk and stores to continue installation, then eject the
ISO.

The generated administrator password is in `administrator-password` and the
answer file. The output directory is mode 0700 and those files are mode 0600.
The rebuilt answer ISO also contains the password; keep its private output
directory and do not distribute it.

For graphical installations, omit the answer-file argument and choose the
matching profile documented in the console guide. The ISO builder preserves
source media and verifies every prepared file after re-extraction.

A prepared disk must be fully shut down before offline access. Disable
hibernation with `powercfg.exe /hibernate off`, then run `shutdown /s /t 0`.
Never force offline writes to an NTFS volume reported as hibernated or unclean.

The earlier QEMU-based preparation evidence remains historical. The revised
single-DVD unattended procedure needs a fresh installation qualification.

## Evaluation activation

Before long tests, connect the guest to the Internet and check its evaluation
activation from an elevated command prompt:

```bat
cscript.exe //nologo C:\Windows\System32\slmgr.vbs /xpr
```

If activation is required, run
`cscript.exe //nologo C:\Windows\System32\slmgr.vbs /ato`, then check `/xpr`
again. These are
[Microsoft's activation commands](https://learn.microsoft.com/en-us/windows-server/get-started/activation-slmgr-vbs-options).
The preparation tool does not activate the installed guest.

An unactivated or expired Windows 11 evaluation can
[shut down every hour](https://www.microsoft.com/en-us/evalcenter/evaluate-windows-11-enterprise).
The ARM TAP test encountered this shutdown after full restore; its Application
log recorded WLMS events 100 and 101. A clean Firecracker exit is therefore
insufficient evidence of workload success. Check all guest pass markers and the
activation state. The activation-gated retry passed both TAP and CONNECT checks;
see the [validation record](../../docs/windows.md).

## ARM64 installation

The former QEMU installation generator has been removed. The separate
[ARM64 guide](../../docs/windows-arm64.md) retains the provenance of earlier
prepared-disk tests. A direct Firecracker ARM64 installer is not qualified; the
AMD64 console and installation procedure do not establish ARM64 support.
