# Prepare a Windows test disk

`prepare.rs` creates a private output directory, a sparse 64 GiB raw disk and a
Windows client or Server answer file. `prepare-nt6.rs` adds that file and the
signed storage and network drivers to the installation ISO. Installation runs
directly in Firecracker. No QEMU binary or library is required. Existing output
directories are rejected so preparation cannot overwrite a previous disk.

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

The
[upstream developer tool](../../CONTRIBUTING.md#contribution-quality-standards)
provides the normal build and style checks. Each tool's README gives its build
commands and additional dependencies.

## Prepare an unattended installation

Prerequisites: the repository's Rustup toolchain, `7z`, `genisoimage`,
`sha256sum`, the matching pinned Windows ISO and virtio-win ISO, and the
[graphical firmware](../../docs/windows-console.md). Check the selected image
index against `sources/install.wim` before generating an answer file. Choose
`server` for Server Core or Desktop Experience, or `client` for Windows clients.

```sh
mkdir -p build/windows
rustc --edition 2024 -D warnings tools/windows/prepare.rs -o build/windows/prepare
rustc --edition 2024 -D warnings tools/windows/prepare-nt6.rs -o build/windows/prepare-nt6
build/windows/prepare server build/windows/server-core 1
build/windows/prepare-nt6 server2025 windows-server-2025.iso virtio-win.iso \
  build/windows/server-core-media build/windows/server-core/Autounattend.xml
```

For standard Windows 11 Enterprise Evaluation, use the matching client image
index. Its secure firmware and media policy must match the image; preparing an
answer file does not supply that platform:

```sh
build/windows/prepare client build/windows/windows11-enterprise 1
build/windows/prepare-nt6 win11-enterprise-26h2 windows11-enterprise.iso virtio-win.iso \
  build/windows/windows11-enterprise-media build/windows/windows11-enterprise/Autounattend.xml
```

The client profile creates the local `Firecracker` account in Administrators.
The server profile sets the built-in `Administrator` password. Neither profile
enables automatic logon. These are Microsoft's supported
[unattended account settings](https://learn.microsoft.com/en-us/windows-hardware/customize/desktop/unattend/microsoft-windows-shell-setup-useraccounts-localaccounts-localaccount);
the client profile does not bypass Windows hardware requirements.

Attach the generated `server2025-viostor.iso` or selected client ISO as the
console's optical image and the corresponding `windows.raw` as the writable
virtio disk, following the
[direct installation guide](../../docs/windows-console.md). Setup reads the
answer file from the same DVD as the drivers. The answer file erases disk 0,
creates GPT partitions, installs the selected image and signed drivers, sets
UTC, disables hibernation and enables EMS after installing the fallback EFI
path. Use only the new disposable disk. Firecracker reset exits the process;
restart with the same disk and stores to continue installation, then eject the
ISO.

The generated account password is in `account-password` and the answer file. The
output directory is mode 0700 and those files are mode 0600. The rebuilt answer
ISO also contains the password; keep its private output directory and do not
distribute it.

For graphical installations, omit the answer-file argument and choose the
matching profile documented in the console guide. Both media types contain the
same signed storage and network packages under `$WinPEDriver$`. Setup searches
this directory, loads the matching PE drivers and adds them to the installed
image. This uses Microsoft's
[automatic driver discovery](https://learn.microsoft.com/en-us/troubleshoot/windows-client/setup-upgrade-and-drivers/limitations-dollar-sign-winpedriver-dollar-sign).
No answer-file driver paths or configuration set is required. The ISO builder
preserves source media and verifies every prepared file after re-extraction.
Verify actual PE binding and installed-driver persistence during each new
installation qualification.

Test the NT6 media layout from the repository root with the pinned toolchain:

```sh
mkdir -p build/windows
rustc --edition 2024 -D warnings --test tools/windows/prepare-nt6.rs \
  -o build/windows/prepare-nt6-tests
build/windows/prepare-nt6-tests --test-threads=1
```

The four tests include synthetic `7z` driver archives. They need `7z` and
`sha256sum`, but no Windows ISO. They check selected driver bytes, automatic
storage/network paths with and without an answer file, profile pins and output
cleanup. These standalone Rust tests are separate from the Cargo workspace
checks. They do not qualify Windows Setup or installed-driver persistence.

Test XP media readback from the repository root with the same pinned toolchain:

```sh
mkdir -p build/windows
rustc --edition 2024 -D warnings --test tools/windows/prepare-xp.rs \
  -o build/windows/prepare-xp-tests
build/windows/prepare-xp-tests --test-threads=1
```

The six tests need `xorriso` and `sha256sum`, but no Windows ISO. They include a
small synthetic ISO with the XP filename conventions, payload readback, and
rejection of changed or missing OEM driver files. The existing loader, boot
catalogue, directory bound and output cleanup checks remain. These checks do not
qualify a fresh XP installation.

A prepared disk must be fully shut down before offline access. Disable
hibernation with `powercfg.exe /hibernate off`, then run `shutdown /s /t 0`.
Never force offline writes to an NTFS volume reported as hibernated or unclean.

The earlier QEMU-based preparation evidence remains historical. The revised
single-DVD unattended procedures need fresh installation qualification for each
selected edition; generating an answer file is not an installation result.
