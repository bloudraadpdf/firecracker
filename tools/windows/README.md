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

For Windows probe builds, also install their target libraries:

```sh
rustup target add i686-pc-windows-msvc x86_64-pc-windows-msvc
```

The
[upstream developer tool](../../CONTRIBUTING.md#contribution-quality-standards)
provides the normal build and style checks. Each tool's README gives its build
commands and additional dependencies.

## Windows probe linking

The Windows user-mode probes use Rust `core`, VC-LTL5 and YY-Thunks. They do not
use Rust's Windows `std`. The distributed `*-pc-windows-msvc` standard libraries
do not establish XP or Windows 7 support. Keep the compatibility libraries
external; do not replace them with a generated CRT import library. Kernel
drivers retain the separate WDK build and signing procedure.

Download the following pinned inputs from their publishers. Check each SHA-256
before extraction. VC-LTL5 is EPL-2.0; YY-Thunks is MIT. The Microsoft SDK and
compiler libraries have their own Microsoft terms. Preserve the original notices
when preparing or distributing these inputs.

| Input                                                                                                                                                                                                                                                    | SHA-256                                                            |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| [VC-LTL5 v5.3.1, VC-LTL-Binary.7z](https://github.com/Chuyu-Team/VC-LTL5/releases/download/v5.3.1/VC-LTL-Binary.7z)                                                                                                                                      | `7a18799ed3aa84a225610a5447a56bc534c5c98ccb8dec05caba0e3f633431ad` |
| [YY-Thunks v1.2.3, YY-Thunks-Lib.zip](https://github.com/Chuyu-Team/YY-Thunks/releases/download/v1.2.3/YY-Thunks-Lib.zip)                                                                                                                                | `8d2417cea1914a678f766c73fd458bb85dfd6057878f8d1d87ddcfc370a42ef9` |
| [Windows SDK x86 10.0.19041.5](https://api.nuget.org/v3-flatcontainer/microsoft.windows.sdk.cpp.x86/10.0.19041.5/microsoft.windows.sdk.cpp.x86.10.0.19041.5.nupkg)                                                                                       | `7d7213f0a675877437c9b1ca4ab4fa8cbff9f999a3f0121d2763c17a9b26bb42` |
| [Windows SDK x64 10.0.19041.5](https://api.nuget.org/v3-flatcontainer/microsoft.windows.sdk.cpp.x64/10.0.19041.5/microsoft.windows.sdk.cpp.x64.10.0.19041.5.nupkg)                                                                                       | `ee8b7bc43ed58d17be795c8062ad32e355847aadad7dc8096cc04ccd1f2fdc2e` |
| [Microsoft VC x86 Desktop 14.44.35226](https://download.visualstudio.microsoft.com/download/pr/67cf767c-5e71-47c2-a54a-cd5631e28942/e4dc8ef4551d621501295d2175c890788368d8f8a218401c7c96405d31cb8e41/Microsoft.VC.14.44.17.14.CRT.x86.Desktop.base.vsix) | `e4dc8ef4551d621501295d2175c890788368d8f8a218401c7c96405d31cb8e41` |
| [Microsoft VC x64 Desktop 14.44.35226](https://download.visualstudio.microsoft.com/download/pr/67cf767c-5e71-47c2-a54a-cd5631e28942/f01f701a7bcd9587a340898c851424f6a52bb913a70c185ff0d5bf0288c5831a/Microsoft.VC.14.44.17.14.CRT.x64.Desktop.base.vsix) | `f01f701a7bcd9587a340898c851424f6a52bb913a70c185ff0d5bf0288c5831a` |

VC-LTL5's source revision is `cba9ab9bda943d1659b3d89aef9227f53423f0d5`;
YY-Thunks' is `b41fb80b8d316581eec0959600aeb0e7339d0e5e`. Use YY's **library**
archive with LLD. Upstream's object-file integration can produce duplicate
symbols with LLD. The published `vc-ltl` Rust crate fixes its x86/x64 floor at
Vista and does not include the XP libraries, so it does not replace these
inputs.

Extract each archive into the layout below, keeping filenames and case. Use
`7z x ARCHIVE -oDIRECTORY` for the 7z, ZIP, NuGet and VSIX containers. Only the
selected architecture's SDK package and VC package are needed for one build. The
VC package's internal directory is `14.44.35207`.

```text
build/windows/compat/
  vc-ltl/TargetPlatform/<CRT baseline>/lib/<Win32 or x64>/
  yy-thunks/Lib/<API baseline>/<x86 or x64>/
  sdk/<x86 or x64>/c/um/<x86 or x64>/
  msvc/<x86 or x64>/Contents/VC/Tools/MSVC/14.44.35207/lib/<x86 or x64>/
```

YY's archive includes its licence. The VC binary archive omits it; retain the
pinned source's
[licence](https://github.com/Chuyu-Team/VC-LTL5/blob/cba9ab9bda943d1659b3d89aef9227f53423f0d5/LICENSE)
and
[CRT notices](https://github.com/Chuyu-Team/VC-LTL5/tree/cba9ab9bda943d1659b3d89aef9227f53423f0d5/Sources/ucrt/Licenses).
Retain the SDK packages' licence metadata and
[SDK terms](https://aka.ms/WinSDKLicenseURL), and the
[Build Tools terms](https://go.microsoft.com/fwlink/?LinkId=2179911).

Use one Bash session for the following setup and the probe's build command. For
Windows 7 SP1 x64 and later x64 guests, select:

```bash
compat_root="$PWD/build/windows/compat"
windows_arch=x64
windows_target=x86_64-pc-windows-msvc
windows_subsystem=6.01
windows_yy_lib="$compat_root/yy-thunks/Lib/6.1.7600.0/x64"
windows_vc_lib="$compat_root/vc-ltl/TargetPlatform/6.0.6000.0/lib/x64"
windows_sdk_lib="$compat_root/sdk/x64/c/um/x64"
windows_crt_lib="$compat_root/msvc/x64/Contents/VC/Tools/MSVC/14.44.35207/lib/x64"
```

For XP SP3 x86, select these values instead:

```bash
compat_root="$PWD/build/windows/compat"
windows_arch=x86
windows_target=i686-pc-windows-msvc
windows_subsystem=5.01
windows_yy_lib="$compat_root/yy-thunks/Lib/5.1.2600.0/x86"
windows_vc_lib="$compat_root/vc-ltl/TargetPlatform/5.1.2600.0/lib/Win32"
windows_sdk_lib="$compat_root/sdk/x86/c/um/x86"
windows_crt_lib="$compat_root/msvc/x86/Contents/VC/Tools/MSVC/14.44.35207/lib/x86"
```

Prepare LLD's case-insensitive library lookup. Windows library filenames and
embedded `/DEFAULTLIB` directives have inconsistent case; lowercase filenames
alone are insufficient on Linux. This JSON is an LLVM VFS description of the
external files, not a replacement library. Missing directories fail here.

```bash
windows_overlay="$compat_root/libraries-$windows_target.json"
python3 - "$windows_yy_lib" "$windows_vc_lib" "$windows_sdk_lib" "$windows_crt_lib" \
  > "$windows_overlay" <<'PY'
import json
import pathlib
import sys

roots = []
for argument in sys.argv[1:]:
    directory = pathlib.Path(argument).resolve(strict=True)
    libraries = sorted(p for p in directory.iterdir() if p.suffix.lower() == ".lib")
    if not libraries:
        raise SystemExit(f"No import libraries in {directory}")
    roots.append({"type": "directory", "name": str(directory), "contents": [
        {"type": "file", "name": p.name, "external-contents": str(p)} for p in libraries
    ]})
json.dump({"version": 0, "case-sensitive": False, "roots": roots}, sys.stdout)
PY
rust_linker="$(rustc --print sysroot)/lib/rustlib/$(rustc --print host-tuple)/bin/rust-lld"
test -x "$rust_linker"
windows_rustflags=(
  --target "$windows_target"
  -C panic=abort -C opt-level=2 -C lto=fat -C target-feature=+crt-static
  -C "linker=$rust_linker"
  -C link-arg=/entry:mainCRTStartup
  -C "link-arg=/subsystem:console,$windows_subsystem"
  -C "link-arg=/vfsoverlay:$windows_overlay" -C link-arg=/Brepro
  -L "native=$windows_yy_lib" -L "native=$windows_vc_lib"
  -L "native=$windows_sdk_lib" -L "native=$windows_crt_lib"
  -l static=libvcruntime -l static=libucrt -l static=libcpmt
)
```

Ordinary import-library linkage lets YY supply its API fallbacks. Do not use
`raw-dylib` or `/nodefaultlib` with this procedure. The Windows API declarations
keep the system ABI; the executable entry uses the C ABI. Require successful PE
import/header inspection and actual execution on each declared minimum OS after
rebuilding. A successful link does not prove runtime compatibility.

The selected YY binary release has no ARM64 libraries. Its source has an ARM64
build path, but that payload and the native ARM64 probes still need separate
qualification. Do not substitute x64 libraries or call x64 emulation a native
ARM64 result.

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
index and the
[secure firmware/media procedure](../../resources/firmware/cloudhv-secureboot/README.md#current-windows-installation-media):

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
