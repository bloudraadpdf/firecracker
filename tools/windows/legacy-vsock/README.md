# Legacy Windows Viosock validation

These sources reproduce the Windows XP SP3 x86 and Windows 7 SP1 x64 guest
Viosock work. They do not change Firecracker. Use disposable guest disks. The
drivers use a local test certificate; they are not production-signed.

## Pinned inputs

- [virtio-win/kvm-guest-drivers-windows](https://github.com/virtio-win/kvm-guest-drivers-windows),
  revision `8833d0a1c2d6431d58300a00b402708c3628b29f`.
- [EWDK 19041](https://software-download.microsoft.com/download/pr/EWDK_vb_release_svc_prod1_19041_201201-2105.iso),
  SHA-256 `1bd011404cc16c912463fa103cedc1793f9b55f9977ab355ce98ae89c075d841`.
- [WDK 7.1](https://download.microsoft.com/download/4/a/2/4a25c7d5-efbe-4182-b6a9-ae6850409a78/GRMWDK_EN_7600_1.ISO),
  SHA-256 `5edc723b50ea28a070cad361dd0927df402b7a861a036bbcf11d27ebba77657d`.
- Rust 1.99.0 with the `i686-pc-windows-msvc` and `x86_64-pc-windows-msvc`
  targets for the standalone test programs.

Apply the files in `patches/series`, in order, to the pinned source. The first
four patches retain upstream authorship and commit references:

| Upstream commit                            | Fix                                                            |
| ------------------------------------------ | -------------------------------------------------------------- |
| `661fc34f1c64e442e459eeecef4cf132ed4a89cb` | Unlock an empty receive queue                                  |
| `a48f19c69e47f1200cbb8d867b69b6a1db227a20` | Advance a partial-read buffer before clearing its length       |
| `ed92ff4314d01ccf8c6f52707803c363899ee017` | Return receive credit before clearing its length               |
| `124d09620663a9ac62edbf9615091c32ec3a84f8` | Validate the event structure size rather than its pointer size |

Patch 5 releases completed DMA transactions before request completion. Patch 6
adds the XP build configuration, XP pool and annotation compatibility, removes
unused variables rejected by the compiler, and excludes a Vista-only Winsock
operation from XP. The DMA fix prevents KMDF bugcheck 0x10D/3 during restore;
see
[Microsoft's DMA completion requirements](https://learn.microsoft.com/en-us/windows-hardware/drivers/wdf/completing-a-dma-transaction).

## Build and package

In an isolated Windows build VM, mount both toolchain ISOs. Copy this
directory's scripts to `C:\LegacyVsock` and put the patched upstream source at
`C:\LegacyVsock\upstream`. Run `setup-xp-tools.ps1` as administrator to extract
the WDK 7.1 MSI packages. That script shuts down the build VM.

On the next boot, run these commands from an elevated command prompt:

```bat
C:\LegacyVsock\build-win7.cmd
C:\LegacyVsock\build-xp.cmd
C:\LegacyVsock\build-xp-user.cmd
```

Each command must exit zero. The XP kernel build first checks a minimal header
reproducer: the unmodified WDK header must fail with the newer compiler, and the
build with `NONTOSPINTERLOCK` must pass. The provider links the WDK 7.1 static C
runtime. The newer dynamic runtime imports an exception handler which the tested
XP system does not export.

Create the test certificate once in the build VM's machine certificate store,
using elevated PowerShell:

```powershell
New-SelfSignedCertificate -Type CodeSigningCert -Subject 'CN=Firecracker Legacy Vsock Test' -CertStoreLocation Cert:\LocalMachine\My -KeyAlgorithm RSA -KeyLength 2048 -HashAlgorithm SHA256
```

Run `package.ps1 -Target win7` or `package.ps1 -Target xp`. Each invocation
shuts down the builder after writing `package-result.txt`. Require result zero,
inspect the transcript, and preserve the complete output directory. Only the
public certificate is exported. Do not distribute the private key.

## Tokoloshe proxy helper

The preserved `windows_probe.rs` example targets Tokoloshe revision
`dedf6eedef6b612ccd3f4bd0d24d7325756d4e19`. All 95 copied source and lock files
used for the original validation match that revision. The test copy only
narrowed the workspace members to `crates/*`.

In an isolated checkout of that revision, copy `windows_probe.rs` to
`crates/tokoloshe-egress/examples/windows_probe.rs` and build:

```sh
cargo build --locked -p tokoloshe-egress --example windows_probe --bin tokoloshe-egress
```

The helper takes `RUN SIDECAR SOCKET IPV4_ENDPOINT`. Keep the run path short
enough for Linux Unix-domain socket paths, including the proxy suffix
`/jails/egress-sockets/1.sock`. It opens a socket grant for the echo endpoint,
prints `READY`, and waits for a newline on standard input. It then seals and
prints the real proxy accounting, removes its link, and shuts down the sidecar.
It does not replace the proxy implementation.

## Guest probes

The Rust programs have no standard-library dependency. Keep the sibling
`egress/win32.rs` source when copying `connect.rs`; it shares the small Win32
import declarations with the HTTP probes. Build an x86 import library from
`msvcrt.def` with the selected Rust toolchain's `rust-lld`:

```sh
rust-lld -flavor link /lib /machine:x86 /def:msvcrt.def /out:msvcrt.lib
rustc --edition 2024 --target i686-pc-windows-msvc \
  --check-cfg 'cfg(xp_matrix)' -C panic=abort -C opt-level=2 -C lto=fat \
  -C linker=rust-lld -C link-arg=/entry:mainCRTStartup \
  -C link-arg=/subsystem:console,5.01 -C link-arg=/nodefaultlib \
  -C link-arg=msvcrt.lib -D warnings connect.rs -o legacy-connect.exe
```

Add `--cfg xp_matrix` and name the output `xp-connect.exe` for the XP
three-iteration storage and CONNECT probe. Compile `install-driver.rs` with the
same target, entry point, subsystem, linker and panic options, without the CRT
library. For `install-driver-x64.exe`, use the x64 target and subsystem version
6.01. Build the x64 CONNECT client with the same x64 target and subsystem, and
generate its CRT import library with `/machine:x64` instead of `/machine:x86`.
Pass that library with `-C link-arg=...`. The installer targets Firecracker's
Viosock PCI ID `PCI\VEN_1AF4&DEV_1053&SUBSYS_10531AF4&REV_01`.

Copy the complete package to `C:\Drivers\VIOSock`, the installer to `C:\`, and
use `install-xp.cmd` or `install-win7.ps1` at guest startup. XP also needs the
WDK 7.1 x86 `CertMgr.exe` in `C:\`. Require installer result zero or 3010, then
device error code zero after a reboot. Check the installed driver and provider
against the package before probing.

Replace the installer startup entry with `xp-matrix.cmd`, or invoke
`win7-connect.ps1` on Windows 7 with `legacy-connect.exe` in `C:\`. Do not leave
both installer and probe startup entries enabled.

The proxy must expose CID 2, port 4750. Allow `127.0.0.1:40567` to a TCP echo
server and deny `127.0.0.1:40568`. Each iteration transfers and verifies 2 MiB
in each direction, then requires HTTP 403 for the denied destination. There is a
60-second interval between iterations for the host to pause, snapshot and
restore in a fresh Firecracker process: full snapshot first, then differential
snapshot rebased onto the full memory file. Use the same disk in this linear
sequence. Do not restore an older snapshot with a disk modified by a later run.

The preserved `check.sh` runs this sequence. Set `WINDOWS_CONNECT_BASE` to a
fresh test directory containing `config.json`, with absolute firmware and disk
paths, `track_dirty_pages: true`, CID 3, and `uds_path: "vsock.sock"`. Set
`WINDOWS_CONNECT_FC`, `WINDOWS_CONNECT_PROXY` (Tokoloshe `windows_probe`),
`WINDOWS_CONNECT_SIDECAR` (`tokoloshe-egress`), `WINDOWS_CONNECT_PROXY_ROOT`,
and `WINDOWS_CONNECT_REBASE` to their absolute paths. Set `WINDOWS_CONNECT_UID`
and `WINDOWS_CONNECT_GID` to the test user. The controller creates `run/`;
preserve or remove an earlier run before starting another. It executes a
read-only per-run copy of its shell source. For a graphical guest, use the
relative console path `console.sock`; cleanup removes its stale socket only
after the owned VMM has stopped. User and group IDs default to the invoking
user. When changing identity, it initialises that user's supplementary groups.
API requests have time limits. On exit it allows each child ten seconds to stop,
then kills a child that does not exit. The proxy uses a separate process group
so cleanup also stops a stalled sidecar. A forced stop fails the run.

Require three guest pass markers, six sealed proxy accounting entries (three
allowed 2 MiB transfers, three denied requests with zero payload),
guest-requested S5 and Firecracker exit zero. After all guest processes have
exited, mount the disk read-only and verify the final data. On XP, compile
`verify-xp.rs` for the host and pass it the extracted `xp-legacy-data.bin`; it
checks every byte of the final iteration's deterministic 2 MiB file. Windows 7
records its final SHA-256 in the transcript.

`win7-connect.ps1` also accepts `-Prefix` and `-Tag` for newer Windows with
signed Viosock drivers. The [egress harness](../egress/README.md) provides the
shared offline verifier and the separate TAP regression procedure.

The full test history and evidence paths are in the
[Windows support plan](../../../docs/windows.md).
