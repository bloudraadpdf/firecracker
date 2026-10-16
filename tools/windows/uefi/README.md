# Windows UEFI runtime probe

`probe.rs` tests Windows runtime variable calls on an enrolled Secure Boot test
guest. It enables `SeSystemEnvironmentPrivilege`, reads SetupMode=0 and
SecureBoot=1 with attributes 6, attempts same-value writes to each, and requires
rejection and unchanged bytes and attributes. These calls establish
Windows-visible rejection; the runtime attribute rule can also reject the
writes. A Win32 error does not identify a specific EFI status or policy branch.

The probe also creates and updates one private authenticated variable,
`FirecrackerRuntimeTest` in GUID `7bc4c67c-16f4-4df6-a24e-a3caa7904f09`. It
checks exact bytes and attributes after each update and each rejected signature,
owner and timestamp case. It does not write PK, KEK, db or dbx, change Secure
Boot policy, or call the TPM. Use only a disposable matched test store: the
public [fixtures](fixtures/README.md) are test credentials.

## Build and stage

Follow the [Rust prerequisites](../README.md#build-prerequisites) and select the
x64 library profile in the
[shared link procedure](../README.md#windows-probe-linking). Keep `Shell32.lib`
from the same pinned x64 Windows SDK in `windows_sdk_lib`; the shared library
overlay includes it. Use that Bash session from the repository root:

```sh
mkdir -p build/windows/uefi
rustc --edition 2024 "${windows_rustflags[@]}" -D warnings \
  tools/windows/uefi/probe.rs -o build/windows/uefi/uefi-runtime.exe
```

The Get/SetFirmwareEnvironmentVariableExW APIs require Windows 8 or Server 2012
and later. The shared 6.01 loader setting does not qualify Windows 7. Check the
PE imports and headers after building. A link or host fixture pass is not a
Windows execution result.

Copy `uefi-runtime.exe`, `tpm-runtime.exe` and `secure-check.ps1` beside the
guest egress script through read-only media or the guest's normal network path.
Do not mount a live disk. Run with an elevated account. Use the maintained
[secure egress procedure](../egress/README.md) for cold phase 1, full restore
phase 2 and differential restore phase 3. The scripts invoke the probe with `1`,
`2` and `3`; do not run these updates ahead of the controller.

Phase 1 requires the private variable to be absent and creates value 1. Phases 2
and 3 require the preceding value and replace it with values 2 and 3. Each phase
must emit these lines, in this order, with a nonzero decimal error:

```text
UEFI_RUNTIME rejected=SetupMode win32_error=<nonzero decimal>
UEFI_RUNTIME rejected=SecureBoot win32_error=<nonzero decimal>
UEFI_RUNTIME rejected=tampered win32_error=<nonzero decimal>
UEFI_RUNTIME rejected=wrong-owner win32_error=<nonzero decimal>
UEFI_RUNTIME rejected=replay win32_error=<nonzero decimal>
PASS UEFI_RUNTIME phase=<1, 2 or 3>
```

Require exit zero. The offline egress verifier requires all three ordered
blocks, errors in the range 1 through 4294967295, the unchanged UEFI and TPM
baselines, and the controller's fresh phase evidence. Markers alone do not
establish a restore. The tampered fixture has timestamp second 45, later than
all three accepted updates at seconds 10, 20 and 30; its signed content has been
changed. The helper and pre-boot tests provide separate evidence.

## Check a later cold boot

After phase 3, preserve its disk, variable store and TPM state as one matched
set. Arrange a one-shot startup check, shut Windows down cleanly, stop the VMM
and both helpers, then boot that same set from the configured secure firmware
without loading a snapshot. Keep the host start/stop logs and fresh guest log. A
`cold` argument or a three-phase pass does not prove this transition.

For example, put this script at `C:\fc-probes\uefi-cold.ps1`:

```powershell
$ErrorActionPreference = 'Stop'
Disable-ScheduledTask -TaskName 'FirecrackerUefiCold' | Out-Null
$output = @(& 'C:\fc-probes\uefi-runtime.exe' cold)
$code = $LASTEXITCODE
$output | Set-Content -Encoding ASCII 'C:\Users\Public\fc-uefi-cold.txt'
if ($code -ne 0) { throw "UEFI cold probe exit $code" }
shutdown.exe /s /t 0
```

Before the clean shutdown, register it from elevated PowerShell:

```powershell
$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\fc-probes\uefi-cold.ps1'
$trigger = New-ScheduledTaskTrigger -AtStartup
Register-ScheduledTask -TaskName 'FirecrackerUefiCold' -Action $action -Trigger $trigger -User 'SYSTEM' -RunLevel Highest
shutdown.exe /s /t 0
```

The action permits this disposable script with a
[process execution policy](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_execution_policies?view=powershell-5.1);
it does not change the global execution policy, BCD, Secure Boot or driver
integrity settings. The task disables itself before the probe so a later boot
cannot repeat the delete. The explicit ASCII encoding avoids Windows PowerShell
5.1
[redirection's UTF-16LE default](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_character_encoding?view=powershell-5.1).
The log is written before any shutdown; probe failure stops the script before
shutdown. Require exit zero and a fresh `fc-uefi-cold.txt` containing the two
derived-variable rejection lines above followed by
`PASS UEFI_RUNTIME phase=cold`, with no failure line. The probe requires
retained value 3, submits the owner's newer authenticated empty update, then
requires absence. Preserve the log and the final matched state after clean
shutdown; remove the disabled task with
`Unregister-ScheduledTask -TaskName 'FirecrackerUefiCold' -Confirm:$false` when
finished. This cold deletion is separate from the three-phase offline verifier.

Public API references:
[Get](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getfirmwareenvironmentvariableexw),
[Set](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setfirmwareenvironmentvariableexw)
and
[Task Scheduler](https://learn.microsoft.com/en-us/windows/win32/taskschd/schtasks).
