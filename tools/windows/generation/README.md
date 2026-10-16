# Windows generation-counter probe

`probe.rs` reads the guest's actual Windows generation-counter device. It uses
Rust `core` and the existing Windows probe compatibility inputs. It does not
install a driver or provide a replacement generation counter.

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

The existing `egress/win32.rs` must remain at its repository path. No new crate,
C implementation or generated import library is needed. Check the actual PE
architecture, subsystem version and imports before using a rebuilt executable. A
successful link does not qualify a guest driver or a restore.

## Interface and result

The definitions follow Microsoft's published SDK headers
[vmgenerationcounter.h](https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/shared/vmgenerationcounter.h)
and
[devioctl.h](https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/shared/devioctl.h).
`IOCTL_VMGENCOUNTER_READ` is `0x0032c004`. Its output is the 16-byte
`VM_GENCOUNTER`: two unsigned 64-bit fields, `GenerationCount` then
`GenerationCountHigh`. The Rust layout and size are checked at compile time.

Run the matching executable as Administrator or SYSTEM. It opens
`\\.\VmGenerationCounter` for read/write access, issues the polling request with
no input buffer, and requires exactly 16 output bytes. The owned device handle
closes on success and failure. A successful invocation exits zero and prints one
line:

```text
VMGENID=0123456789abcdef0123456789abcdef
```

The line contains the high field followed by the low field, each as 16 lowercase
hex digits. The example is not a recorded guest result. API failures print the
operation and Win32 error in hex. Error 5 also reports the elevation
requirement. An unexpected output size fails. Exit 1 indicates query failure, 2
indicates an output failure, and 99 indicates a Rust panic.

Microsoft documents the
[polling interface and guest prerequisites](https://learn.microsoft.com/en-us/windows/win32/hyperv_v2/virtual-machine-generation-identifier).
Win7 SP1 and XP SP3 need compatible integration-service drivers; compiling for
those OS versions does not install them. Polling is supported by that documented
legacy interface. This probe does not wait for events or qualify GED delivery.

## Win7 driver servicing

Use a private, closed Win7 SP1 x64 test image. The reviewed Microsoft
[KB3063109 package](https://support.microsoft.com/en-us/topic/hyper-v-integration-components-update-for-windows-virtual-machines-that-are-running-on-a-windows-10-or-windows-server-2016-based-host-bd22f4f6-feec-89f4-8a1d-405076ff4222)
is `windows6.x-hypervintegrationservices-x64.cab`, SHA256
`19d07b22f24e73e5256fbe2526c48af818f26e80386a11166252f9fa20234046`. Its
generation-counter component is version 6.3.9600.18080. Keep the CAB, Microsoft
catalogue and servicing manifests intact. The extracted INF/SYS pair is not an
independently signed driver package.

Stage the unchanged CAB in `C:\Users\Public` using the existing closed disk
preparation route. Copy the selected `win7-x64.exe` or `xp-x86.exe` executable
as `C:\Users\Public\vmgenid.exe`; use only the executable that matches the
guest. For Win7 package servicing, in an elevated native Windows command prompt:

```bat
dism.exe /Online /Get-PackageInfo /PackagePath:C:\Users\Public\windows6.x-hypervintegrationservices-x64.cab
dism.exe /Online /Add-Package /PackagePath:C:\Users\Public\windows6.x-hypervintegrationservices-x64.cab /NoRestart /LogPath:C:\Users\Public\fc-gencounter-servicing.log
```

Inspect applicability and servicing results. Complete any required reboot before
qualification, then record the installed package, actual bound
generation-counter INF/version, running `gencounter` service and device error
code. Preserve the servicing log. Do not use `/IgnoreCheck`, edit the INF or
disable signature checks. Microsoft documents the
[package servicing commands](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/dism-operating-system-package-servicing-command-line-options).
This Win7 x64 CAB is not the XP x86 driver payload.

The existing Win7 preparation uses an elevated PowerShell startup script and
stores transcripts under `C:\Users\Public`, then shuts down cleanly for host
read-only collection. Query the device's complete `HardwareID` and
`CompatibleIDs` registry values as well as its DeviceID; a Code 0 result alone
is insufficient.

## Cold, full and differential checks

Invoke the probe within the existing guest stage action, before its
corresponding pass marker. Preserve the exact exit code and output. For example,
in elevated Windows PowerShell:

```powershell
& C:\Users\Public\vmgenid.exe
if ($LASTEXITCODE -ne 0) { throw 'VMGenID query failed' }
```

Require stable identity for repeated reads within one running stage and a fresh
identity after each full and differential restore. Keep all three actual values
and phase boundaries. Execute through the real serviced guest driver; reading
host snapshot data does not establish Windows polling behaviour.

Only after those checks pass can a polling-only profile claim restore detection
without GED. A changed identity does not by itself prove entropy reseeding or
application recovery. Driver servicing, real legacy queries and the full restore
matrix remain separate from compile/link checks.
