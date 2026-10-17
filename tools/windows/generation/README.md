# Windows generation-counter probe

`probe.rs` reads the guest's actual Windows generation-counter device. It can
also wait for a generation change on supported guests. It uses Rust `core` and
the existing Windows probe compatibility inputs. It does not install a driver or
provide a replacement generation counter.

## Build

Select the Windows 7 x64 profile and prepare `windows_rustflags` with the
[shared VC-LTL5/YY-Thunks link procedure](../README.md#windows-probe-linking).
From the repository root, in that same Bash session:

```sh
mkdir -p build/windows/generation
rustc --edition 2024 "${windows_rustflags[@]}" -D warnings \
  tools/windows/generation/probe.rs -o build/windows/generation/win7-x64.exe
```

For XP SP3 x86, select the shared procedure's XP x86 profile, regenerate the
flags, and build instead:

```sh
mkdir -p build/windows/generation
rustc --edition 2024 "${windows_rustflags[@]}" -D warnings \
  tools/windows/generation/probe.rs -o build/windows/generation/xp-x86.exe
```

Keep `counter.rs`, `polling.rs`, `egress/win32.rs` and `egress/device.rs` at
their repository paths. The probe and XP egress readers share the same polling
implementation; the standalone probe retains its notification interface. No new
crate, C implementation or generated import library is needed. Check the actual
PE architecture, subsystem version and imports before using a rebuilt
executable. A successful link does not qualify a guest driver or a restore.

## Interface and result

The definitions follow Microsoft's published SDK headers
[vmgenerationcounter.h](https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/shared/vmgenerationcounter.h)
and
[devioctl.h](https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/shared/devioctl.h).
`IOCTL_VMGENCOUNTER_READ` is `0x0032c004`. Its output is the 16-byte
`VM_GENCOUNTER`: two unsigned 64-bit fields, `GenerationCount` then
`GenerationCountHigh`. The Rust layout and size are checked at compile time.

Run the matching executable as Administrator or SYSTEM. With no arguments, it
opens `\\.\VmGenerationCounter` for read/write access, issues the polling
request with no input buffer, and requires exactly 16 output bytes. The owned
device handle closes on success and failure. A successful invocation exits zero
and prints one line:

```text
VMGENID=0123456789abcdef0123456789abcdef
```

The line contains the high field followed by the low field, each as 16 lowercase
hex digits. The example is not a recorded guest result. API failures print the
operation and Win32 error in hex. Error 5 also reports the elevation
requirement. An unexpected output size fails. Exit 1 indicates a query, wait or
argument failure, 2 indicates an output failure, and 99 indicates a Rust panic.

Microsoft documents the
[polling interface and guest prerequisites](https://learn.microsoft.com/en-us/windows/win32/hyperv_v2/virtual-machine-generation-identifier).
Win7 SP1 and XP SP3 need compatible integration-service drivers; compiling for
those OS versions does not install them. Polling is supported by that documented
legacy interface. Event-driven requests require Vista SP2/Server 2008 SP2 or
later; the documented interface returns `ERROR_NOT_SUPPORTED` (50) on earlier
systems, including XP. XP qualification uses polling, not notification.

## Prearmed notification

On Win7, supply the current ID from a successful poll, in the same high-then-low
hex order. The ID must be exactly 32 hex digits and nonzero. The timeout must be
1 to 600000 milliseconds, with one to six decimal digits. The option order is
fixed; missing, extra or invalid arguments fail before opening the device.
Windows `CommandLineToArgvW` handles a quoted executable path. For example, in
an elevated command prompt, substitute the actual current ID:

```bat
"C:\Users\Public\vmgenid.exe" --wait 0123456789abcdef0123456789abcdef --timeout-ms 300000
```

The probe opens an overlapped handle and submits `IOCTL_VMGENCOUNTER_READ` with
that **16-byte current ID as input** and a separate 16-byte output buffer.
Microsoft documents that this request completes when the generation differs. A
zero-length input would poll and cannot establish notification. Only a
submission that returns `ERROR_IO_PENDING` (997) prints:

```text
ARMED VMGENID=0123456789abcdef0123456789abcdef
```

An immediate completion fails without `ARMED`: the input could already be stale.
A pending request can complete before the marker is observed, so the marker
alone is insufficient. Require the process to remain pending until the planned
snapshot/restore boundary. After notification, success requires exactly 16
output bytes and a different ID, prints `VMGENID=<new-ID>`, and exits zero.
Compare that ID with fresh polls in the restored guest.

On timeout or wait failure, the issuing thread calls the XP-compatible
[`CancelIo`](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-cancelio)
and allows up to five further seconds for completion. It checks the result with
[`GetOverlappedResult`](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-getoverlappedresult)
without an unbounded wait. Timeout remains a failure even if completion races
with cancellation. Device and event handles close after completion or drained
cancellation. If cancellation cannot be drained, the probe reports failure and
calls `ExitProcess` without unwinding, keeping request storage live until
process teardown. Output failure and panic also terminate the process directly.
Neither a timeout nor process termination qualifies notification.

Argument tests follow the existing standalone probe test pattern. From the
repository root:

```sh
mkdir -p build/windows/generation
rustc +1.98.1 --edition 2024 --test -D warnings tools/windows/generation/probe.rs \
  -o build/windows/generation/probe-tests
build/windows/generation/probe-tests
```

These tests cover option parsing, layout, field order and the pure XP polling
state checks. They do not execute Windows I/O or qualify driver completion,
cancellation or SCI delivery.

## Win7 driver servicing

Use a private Win7 SP1 x64 test image. First complete the
[Windows 7 signing prerequisites](../../../docs/windows.md#windows-7-signed-drivers),
including the normal restart and installed-state/trust checks. Shut the guest
down cleanly before offline staging. The reviewed Microsoft
[KB3063109 package](https://support.microsoft.com/en-us/topic/hyper-v-integration-components-update-for-windows-virtual-machines-that-are-running-on-a-windows-10-or-windows-server-2016-based-host-bd22f4f6-feec-89f4-8a1d-405076ff4222)
is `windows6.x-hypervintegrationservices-x64.cab`, SHA256
`19d07b22f24e73e5256fbe2526c48af818f26e80386a11166252f9fa20234046`. Its
generation-counter component is version 6.3.9600.18080. Keep the CAB, Microsoft
catalogue and servicing manifests intact. The extracted INF/SYS pair is not an
independently signed driver package.

Stage the unchanged CAB in `C:\Users\Public` using the existing closed disk
preparation route. Copy `win7-x64.exe` as `C:\Users\Public\vmgenid.exe`. For
Win7 package servicing, first inspect applicability in an elevated native
Windows command prompt:

```bat
dism.exe /Online /Get-PackageInfo /PackagePath:C:\Users\Public\windows6.x-hypervintegrationservices-x64.cab
```

If the package is applicable, install it without an automatic restart:

```bat
dism.exe /Online /Add-Package /PackagePath:C:\Users\Public\windows6.x-hypervintegrationservices-x64.cab /NoRestart /LogPath:C:\Users\Public\fc-gencounter-servicing.log
echo %ERRORLEVEL%
```

Inspect the actual exit and servicing results. Complete any required reboot
before qualification, then record the installed package, actual bound
generation-counter INF/version, running `gencounter` service and device error
code. Preserve the servicing log. Do not use `/IgnoreCheck`, edit the INF or
disable signature checks. Microsoft documents the
[package servicing commands](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/dism-operating-system-package-servicing-command-line-options).
This Win7 x64 CAB is not the XP x86 driver payload.

Keep guest transcripts and the actual servicing exit/result. After normal
shutdown, collect them from the stopped disk read-only. A servicing exit of 3010
requires a restart; it is not a driver-binding pass. After the next cold boot,
record the complete `HardwareID` and `CompatibleIDs`, device Code 0, selected
signed INF and installed SYS hashes/version, running `gencounter` service and
actual KMDF files/runtime. A Code 0 result alone is insufficient.

## XP driver prerequisites

The Win7 x64 CAB is not an XP x86 package. XP SP3 x86 needs the original
Microsoft x86 generation-counter INF/SYS/CAT package and compatible KMDF. Keep
all three original names and bytes so the catalogue still covers the package.
The reviewed XP driver is version 6.3.9600.17831 and requests KMDF 1.9; finding
framework files on disk does not establish registered/running framework services
or trust.

Use `xp-x86.exe` for this guest. Use normal Windows driver installation,
preserve its actual error/restart result and signature warnings, and complete a
normal restart before checking the selected signed package, device Code 0 and
running service. Do not edit an INF, substitute a catalogue or disable signing
checks. Neither the Win7 CAB nor a correct MSI reconstruction proves XP binding.
The installed XP SP3 x86 reference has separate
[recorded cold-query evidence](../../../docs/windows-validation.md#recorded-windows-xp-reference).
That evidence does not qualify a rebuilt egress reader or restore matrix.

## Cold, full and differential checks

Use the [maintained TAP workflow](../egress/README.md#tap) with a private Win7
SP1 x64 image whose signed generation-counter driver has passed the servicing
checks above. Build the matching probe first. On the Linux controller host,
record its actual digest before staging:

```sh
sha256sum build/windows/generation/win7-x64.exe
```

Keep that executable, digest, Firecracker and snapshot-editor hashes, firmware
hash and snapshot format with the results. With the guest shut down, use the
existing closed-disk staging route to copy the executable as
`C:\Users\Public\vmgenid.exe`, the maintained `egress/tap.ps1` as
`C:\Users\Public\fc-network.ps1`, and the HTTPS probe beside it. Configure
exactly one elevated startup action as described by the TAP workflow. Supply the
recorded 64-digit SHA-256 value; do not compute the expected value from the
guest copy. For example, with the actual lab addresses and digest assigned:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File C:\Users\Public\fc-network.ps1 `
  -HostAddress $hostAddress -DnsAddress $dnsAddress -Label WIN7 `
  -GenerationChecks -GenerationProbeSha256 $probeSha256
```

`-GenerationChecks` is optional. It requires `-GenerationProbeSha256`; supplying
the digest alone also fails. Before transcript or network changes, the script
checks these flags, the staged executable's digest and the absence of
`C:\Users\Public\fc-gencounter-events`. Generation mode also refuses an existing
`C:\Users\Public\fc-network.log`; it creates the event directory only after
starting a fresh transcript. A preflight or transcript-start failure prints its
error on the console and requests S5 without opening a report connection;
earlier evidence remains intact. Ordinary TAP runs need neither the generation
probe nor these flags.

Before staging a fresh disposable clone, preserve its earlier transcript and
event directory through the closed, read-only collection route. Remove both
`fc-network.log` and `fc-gencounter-events` from the fresh clone while it is
shut down. Keep the original evidence unchanged. Do not use append or overwrite
to combine separate runs.

For each stage, the script polls twice and requires the same nonzero current ID.
Before the cold and full PASS reports, it starts one pending `--wait` request
with that ID, checks its exact `ARMED` output and requires that same child
process to remain alive. The existing TAP controller pauses on each PASS and
captures that pending request in the full or differential snapshot. After
restore, the original child must exit zero with a different ID that agrees with
two fresh polls. Starting a replacement waiter after restore cannot satisfy
these checks. The three stage IDs must differ.

Run the existing `tap-check.sh` with the documented environment and outer
timeout. No new controller or host network setting is needed. After the final S5
and Firecracker exit zero, enable the optional check in the existing offline
verifier. Use the same absolute base, run, tag, disk and partition offset as the
TAP run; Win7's transcript encoding is UTF-16LE:

```sh
export WINDOWS_GENERATION_CHECKS=1
export WINDOWS_TRANSCRIPT_ENCODING=UTF-16LE
sudo --preserve-env=WINDOWS_CHECK_BASE,WINDOWS_CHECK_RUN,WINDOWS_GUEST_TAG \
  --preserve-env=WINDOWS_CHECK_DISK,WINDOWS_PARTITION_OFFSET,WINDOWS_GUEST_ADDRESS \
  --preserve-env=WINDOWS_TRANSCRIPT_ENCODING,WINDOWS_GENERATION_CHECKS \
  timeout --signal=TERM --kill-after=10s 120 \
  unshare --mount --propagation private \
  bash tools/windows/egress/tap-verify.sh
```

The optional verifier needs Python 3.10 or later. The existing read-only mount
supplies exactly five bounded regular files: `reports.log` and stdout/stderr for
each of the two child requests. The guest closes each UTF-8 report append before
sending that same message to the host. These raw records do not depend on
PowerShell transcript width or encoding. Collection creates a new `events/`
directory in the run directory; an existing directory fails instead of replacing
evidence. The verifier requires exact guest/host report equality, received
timestamps, event order, child PIDs and output, and controller phase watermarks.
It writes `generation-verification.json` beside the ordinary TAP results and
records hashes of the parsed bytes. It does not replace the TAP verifier's
independent egress, storage, shutdown and exit checks.

Keep `echo.log`, `stages.log`, the five collected files and both verification
results. Recheck the copied generation evidence without remounting or replacing
it:

```sh
python3 tools/windows/generation/verify.py "$WINDOWS_CHECK_BASE/${WINDOWS_CHECK_RUN:-run1}" \
  --tag "${WINDOWS_GUEST_TAG:-WIN7}"
```

The verifier's pure evidence tests use the existing pytest suite:

```sh
tools/devtool -y test -- integration_tests/build/test_windows_generation.py
```

The maintained opt-in workflow passes cold boot, full restore and differential
restore on the installed Win7 SP1 x64 reference with PowerShell 2 and format-33
snapshots. The public verifier accepts seven matching reports, three distinct
IDs and both original child completions. See the
[recorded Win7 validation](../../../docs/windows-validation.md#recorded-windows-7-reference)
for the tested rejection cases and scope. Other Windows versions still need
runtime qualification. Polling qualifies identity changes separately from
prearmed notification. XP supports polling only and cannot use the PowerShell
notification workflow above. Its separate CMD option is described next. Neither
a changed ID nor a completed request proves entropy reseeding or application
recovery. Driver servicing, real guest queries and the restore matrix remain
separate from compile/link and pure-verifier checks.

## XP polling during egress

The XP TAP and CONNECT readers have an optional `GenerationChecks SHA256`
argument in their maintained CMD launchers. Use the SHA-256 of the actual
rebuilt `restore-probe.exe` or `xp-connect.exe`. This identifies the reader
which performs the inline IOCTLs; it is not the standalone generation probe's
digest. Keep earlier probe binaries and evidence unchanged.

Each stage makes two synchronous reads through the shared `counter.rs` API. Both
must return the same nonzero 16-byte ID. The previous stage's ID stays in the
snapshotted CMD or Rust process memory, so the full and differential stages must
observe new IDs. The raw log records both reads and their stage number. This
mode creates no waiter and makes no notification claim on XP.

Follow the [XP TAP instructions](../egress/README.md#xp-generation-polling) or
[XP CONNECT instructions](../legacy-vsock/README.md#xp-generation-polling). The
public controller hashes the selected reader on the closed OS volume before
starting a VM or network helper. Offline verification hashes it again, checks
all six reads and three distinct stage IDs, and retains the raw evidence hashes
in `generation-verification.json`. Ordinary egress, storage, shutdown and
proxy-accounting checks still run independently.

A successful source test or link does not qualify a new reader build. The
[validation record](../../../docs/windows-validation.md#recorded-windows-xp-reference)
separates the recorded XP TAP and CONNECT polling results from open gates.
Repeat the matching guest and snapshot-format matrix for changed inputs before
claiming their restore coverage. Rebuilding the standalone Win7 probe also needs
a new prearmed notification regression; previous Win7 results identify the
earlier binary.
