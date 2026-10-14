# Prepare a Windows test disk

`prepare.rs` creates a new, private output directory, a sparse 64 GiB raw
disk, a Windows answer ISO, a copy of the OVMF variable store, and an
installation command. It never starts a VM. Existing output directories
are rejected so preparation cannot overwrite a previous disk.

Prerequisites: Rust, `xorriso`, `qemu-img`, the official English-US Windows
Server 2025 ISO, a virtio-win ISO with `viostor/2k25/amd64` and
`NetKVM/2k25/amd64`, and matching OVMF code and variable-store files.
The Windows image index must select Server Core. Check it against the
ISO's `sources/install.wim` image list before installation.

From the worktree on the AMD64 Linux host:

```sh
mise exec -- rustc --edition 2024 -D warnings tools/windows/prepare.rs \
  -o build/windows/prepare
build/windows/prepare amd64 build/windows/server-core 1 \
  build/windows/media/windows-server-2025.iso \
  build/windows/media/virtio-win.iso \
  /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_VARS_4M.fd
```

The generated administrator password is in
`build/windows/server-core/administrator-password`. It is also present
in the answer media. The parent output directory has mode `0700`; keep
it private and do not commit or distribute the answer media.

When ready for the installation stage, run:

```sh
sh build/windows/server-core/install.sh
```

The installer erases its new virtual disk. Do not rerun this command on
an installed disk. QEMU receives no physical disks. Its three CD-ROMs
are attached in order: Windows (`D:`), virtio drivers (`E:`), answer file
(`F:`). The answer file loads the AMD64 storage and network drivers,
creates GPT partitions, and runs `bcdboot` to install the fallback EFI boot
path independently of QEMU's NVRAM. It then enables EMS/SAC, because
`bcdboot` replaces the existing boot entry. The EMS commands explicitly
target `{default}` and `{bootmgr}`; the old `{current}` identifier is no
longer valid during that Setup pass.

The QEMU monitor is a Unix socket at `monitor.sock` in the output directory.
If the ISO requests a key to boot, send `sendkey ret` through that monitor.
If that prompt expires, select the first QEMU DVD-ROM in the firmware boot
menu and press Enter again when the ISO prompts.
Guest serial output goes to `serial.log`. Windows Setup has no serial UI;
installation errors require inspecting Setup logs or a QEMU display.
Synchronous setup commands are logged in
`C:\Windows\Panther\UnattendGC\setupact.log`, including their exit codes.

Shut down QEMU before opening `windows.raw` in Firecracker. Use the
[Firecracker firmware build](../../resources/firmware/README.md), rather
than the OVMF image used for installation.

The answer file disables hibernation with `powercfg.exe /hibernate off`.
For previously prepared disks, run that command in an elevated Windows
prompt, then run `shutdown /s /t 0` and wait for QEMU to exit. Windows Fast
Startup can retain kernel and filesystem state despite a powered-off VM;
see Microsoft's [power-state reference](https://learn.microsoft.com/en-us/windows/win32/power/system-power-states).
Do not force offline writes to a disk that NTFS reports as hibernated or
unclean. Resume Windows and obtain a full shutdown first.

The generator, XML and shell syntax, and refusal to overwrite existing output
have been checked. A fresh installation with explicit boot-entry identifiers
reaches Server Core login with SAC enabled, without manual BCD repairs.
The resulting disk also boots Firecracker and accepts the generated login.

## Evaluation activation

Before long tests, connect the guest to the Internet and check its evaluation
activation from an elevated command prompt:

```bat
cscript.exe //nologo C:\Windows\System32\slmgr.vbs /xpr
```

If activation is required, run `cscript.exe //nologo
C:\Windows\System32\slmgr.vbs /ato`, then check `/xpr` again. These are
[Microsoft's activation commands](https://learn.microsoft.com/en-us/windows-server/get-started/activation-slmgr-vbs-options).
The preparation tool does not activate the installed guest.

An unactivated or expired Windows 11 evaluation can
[shut down every hour](https://www.microsoft.com/en-us/evalcenter/evaluate-windows-11-enterprise).
The ARM TAP test encountered this shutdown after full restore; its Application
log recorded WLMS events 100 and 101. A clean Firecracker exit is therefore
insufficient evidence of workload success. Check all guest pass markers and
the activation state. The activation-gated retry passed both TAP and CONNECT
checks; see the [final validation record](../../docs/windows-support-plan.md#final-validation-and-cleanup-2026-10-14).

## ARM64 installation

Use `arm64` as the first argument with the English-US Windows 11 IoT
Enterprise LTSC 2024 Evaluation ARM64 ISO and AAVMF code/variable files.
Image 1 in that ISO is ARM64. This mode uses `viostor/w11/arm64` and
`NetKVM/w11/arm64`, USB installation media, and the QEMU `virt` machine.
It creates the local administrator account `FcAdmin` with the generated
password. The preparation test reaches the Windows desktop under QEMU TCG
and shuts down cleanly. The resulting disk boots Windows under Firecracker
on a PMU-capable emulated Linux/KVM host. The generated KVM installation
path has not been validated.

The Windows ARM64 loader reads PMUv3 registers. The installation host must
expose a PMU through KVM. The tested Apple nested-KVM appliance does not;
its loader faults on `mrs pmcr_el0`. QEMU TCG with `-cpu max` reaches Windows
first-run setup on that appliance. See the separate
[Firecracker validation results](../../docs/windows-arm64.md#egress-and-validation).

The installed-system locale is also set in the `oobeSystem` pass, using
[Microsoft-Windows-International-Core](https://learn.microsoft.com/en-us/windows-hardware/customize/desktop/unattend/microsoft-windows-international-core-userlocale).
Windows PE locale settings alone do not configure that pass.

The ARM64 answer file selects `bcdedit /emssettings bios` to obtain serial
settings from the firmware SPCR table; see Microsoft's
[EMS configuration reference](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/bcdedit--emssettings).
EMS configuration alone does not prove that the selected Windows edition
provides a SAC command channel.
