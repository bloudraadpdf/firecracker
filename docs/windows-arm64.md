# Windows guests (ARM64, experimental)

Historical emulated tests used the recorded format-12, format-19 and format-20
binaries. Native installation, console, application, lifecycle, snapshot,
security and CONNECT results are recorded below.

The ARM64 implementation loads patched EDK II CloudHv firmware and supplies ACPI
tables for GICv3/ITS, architectural timers, PCI and the serial port. Firmware
boots from modern virtio PCI storage. Earlier tests installed Windows and booted
its desktop under QEMU. Historical Firecracker binaries reached Windows service
startup with release firmware and default seccomp. A SYSTEM startup probe
verified 2 MiB payloads through the real Tokoloshe CONNECT proxy without a guest
NIC, including after full, differential and subsequent full snapshot restore.
Adjacent-port requests receive HTTP 403. An in-flight connection interrupted by
restore fails; fresh connections succeed. This validates the transport, not a
complete Windows port of the Tokoloshe guest agent. The emulated cold boot takes
about 30 minutes to reach the probe and reports several service-start timeouts.

## Build

On the ARM64 Linux host, follow [the build prerequisites](getting-started.md)
and build the VMM, jailer and matching snapshot editor with the pinned Rust
toolchain:

```sh
cargo build --release --locked --target aarch64-unknown-linux-musl \
  -p firecracker -p jailer -p snapshot-editor \
  --features firecracker/windows,snapshot-editor/windows
```

Use the VMM and editor from the same build, in
`build/cargo_target/aarch64-unknown-linux-musl/release/`. ARM64 firmware
execution requires the optional `windows` capability. Default builds reject
firmware before loading it; an ordinary direct Linux kernel boot does not
require this capability. The guest still requires the Linux/KVM host below; this
feature does not add a macOS or Windows host backend.

## Host requirements

Use an ARM64 Linux host with KVM, GICv3/ITS and PMUv3 support. The Windows
loader reads the PMU registers. Apple's default nested VM configuration exposes
no PMU. The outer launcher must enable Apple's PMU emulation before Linux boots.
The Tokoloshe launcher and Toets qualification launcher now set
`_setPerformanceMonitoringUnitEmulationEnabled:` on
`VZGenericPlatformConfiguration`, with a selector check before use. Apple does
not document this method; repeat qualification after a macOS update.

On 3 November 2026, an M5 Max with macOS 26.7.1 and Ubuntu kernels
`6.8.0-142-generic` and `7.0.0-38-generic` reported `KVM_CAP_ARM_PMU_V3=1`. All
60 ARM64 CPU tests passed on each kernel, with PMU initialisation and state
restoration exercised. Restoring the PMU clear aliases now clears all bits and
writes the saved state through their set aliases. This handles both older KVM
write-one-to-clear/set semantics and newer direct assignment. A working Linux
guest or EFI shell does not establish Windows compatibility.

The historical PMU-capable validation environment was an isolated Tokoloshe
Linux appliance under QEMU TCG with virtualisation and PMUv3 exposed.
Firecracker ran inside its Linux KVM. This emulated environment does not
establish hardware performance or a QEMU dependency for native deployment. The
original Tokoloshe appliance was not modified.

## Secure firmware and TPM

The secure profile uses the ARM64 secure CloudHv firmware, a private variable
helper and swtpm. Seed the helper with `seed-microsoft arm64 SOURCE OUTPUT`;
the pinned Microsoft policy uses architecture-specific revocations. Follow
[the secure profile](windows-secure-boot.md) for helper isolation and matched
checkpoint state.

The strict seed trusts Windows UEFI CA 2023 and revokes the older PCA 2011
bootloader. An old installation can therefore fail before Windows starts. On
the isolated Windows 11 IoT Enterprise LTSC 2024 test disk, servicing through
KB5060842 and running [BCDBoot](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/bcdboot-command-line-options-techref-di?view=windows-11)
with `C:\Windows /s S: /f UEFI /bootex` installed the
CA 2023 loader in both Microsoft and fallback ESP paths. Verify the ESP files
against `C:\Windows\Boot\EFI_EX\bootmgfw.efi`. PowerShell's
[`Get-AuthenticodeSignature`](https://learn.microsoft.com/powershell/module/microsoft.powershell.security/get-authenticodesignature)
can report a catalogue signature instead of the
embedded PE signature; its issuer alone does not prove which bootloader ran.

Windows reads the TPM CRB registers with `LDP W1, W8, [X10, #24]`. ARM KVM
cannot supply a decoded MMIO syndrome for this load pair. The Windows-enabled
VMM enables `KVM_CAP_ARM_NISV_TO_USER`, reads the guest instruction through the
existing kernel page-table walker, and handles the signed-offset 32-bit load
pair through the existing MMIO bus. It checks the operand against KVM's fault
address, zero-extends both results and advances the PC. Other instruction forms
fail with an error. This requires no host or guest kernel patch and does not
replace the host PMUv3 requirement. The release seccomp filter permits the
register writeback. Snapshot format remains 47.

The variable helper accepts EDK II's lifecycle event payload: one zero byte.
EndOfDxe, ReadyToBoot and ExitBootServices set monotonic stage flags; malformed
payloads fail without changing state. These flags enforce policy locks and
runtime visibility and must be retained with a checkpoint.

### Native security qualification

On 4 November 2026, Windows 11 IoT Enterprise LTSC 2024 build `26100.4349`
passed the cold security probe on the Apple/Linux 7.0 host above. The secure
firmware had SHA-256
`140c5e13ebf868d159a615705316272658fcc347c7ef89e6685ead553e6b8998`.
The ARM64 seed contained a 2,991-byte `dbx`. Secure Boot was enabled and all
four Windows TPM readiness flags were true. Public TBS commands read random
bytes, PCR7 and a persistent RSA public key/name. Authenticated UEFI updates
passed; derived-state writes, changed signatures, wrong owners and replayed
updates were rejected.

A full restore retained those exact TPM and firmware values and passed phase 2
of the runtime UEFI probe. Normal execution used `Anonymous` memory with
`track_dirty_pages: false`, as used by Toets. The first restore with tracking
enabled consumed keyboard events and advanced guest time, but its desktop did
not respond during the recorded interval. Restoring that same retained guest
without tracking recovered the desktop and passed the security probe.

ARM KVM dirty logging uses 4 KiB stage-2 mappings; see the
[upstream block-transition test](https://linux.googlesource.com/linux/kernel/git/torvalds/linux/+/7bb6284aa7b3c369b41e7f33fcbe193161f008e7/tools/testing/selftests/kvm/arm64/stage2_block_transitions.c).
Host `AnonHugePages` alone therefore does not establish large KVM mappings.
The differential check restored the passed phase-2 state with dirty tracking
enabled, ran it for 20 seconds, then captured a 72 MiB RAM delta. Rebasing that
delta advanced the guest tick count from 74,738 to 76,065. The differential
state loaded with tracking disabled and passed phase 3 of the complete
security probe. An independent transcript verifier confirmed exact TPM and
firmware state across all three phases, plus each authenticated update and
rejection. The full and differential matched checkpoints were archived with
verified checksums before further disk changes. Windows then completed its
pending update work and shut down through PSCI. Firecracker exited with code 0
and both helpers stopped.

On 5 November 2026, the unchanged `210c7565c` executable passed tracked
write, capture and restore checks on both the experimental 16 KiB host and
the standard `7.0.0-38-generic` host with 4 KiB pages. The standard-host check
used four vCPUs, 8 GiB RAM, `Anonymous` memory, `Transparent` huge pages and
`kvm-arm.vgic_v3_group1_trap=1`. A baseline differential capture cleared the
dirty bitmap; the same VM then changed a RAM value and a disk value from 6
to 7. The next capture wrote a 116 MiB delta. Its trace recorded successful
`KVM_GET_DIRTY_LOG` and no `mincore` call. Restoring with dirty logging still
enabled returned RAM 7, disk 7 and the original PowerShell PID 3548.

These checks establish state retention after writes following bitmap clear.
They used no TPM, protected variables or guest network and did not repeat
the security or CONNECT tests. They establish neither a performance fix nor
an Apple defect: host load changed and included periods with no idle CPU.
No additional kernel or Firecracker patch was needed. Toets retains dirty
logging disabled for its full-checkpoint path.

## RAM framebuffer

### Native console and media

On 3 November 2026, Windows 11 IoT Enterprise LTSC 2024 ARM64 reached its
800x600 desktop under native Firecracker on the nested Apple host above, with
Linux `7.0.0-38-generic`, four vCPUs, 8 GiB RAM and `huge_pages: "Transparent"`.
Signed ARM64 virtio-win 0.1.302 packages supplied viostor, vioscsi, vioserial,
vioinput and NetKVM. Pointer input opened the desktop context menu at the
requested position. Keyboard input launched a command and returned
`ARM64_KEYBOARD_SERIAL_PASS` through `com.toets.0`. After a clean shutdown and
cold boot, a command read from the optical drive returned
`ARM64_OPTICAL_SERIAL_INPUT_PASS` through the same port.

Configure `console.serial_port` as `com.toets.0` and `console.optical_drives`
for the two read-only media slots. These use standard virtio PCI console and
SCSI devices. The SCSI transport reuses the AMD64 optical command
implementation. It advertises address limits 255/16383; absent targets and LUNs
return `BAD_TARGET`. Older vioscsi packages treat these limits as counts, so a
zero maximum LUN prevents all optical requests. The
[upstream driver correction](https://github.com/virtio-win/kvm-guest-drivers-windows/commit/ec5e45074bb0)
postdates the pinned driver package.

Snapshot format 47 adds serial handshake and optical transport state. It cannot
load older snapshots. Create new states from cold boots. Unit tests cover the
handshake, pending queues, optical commands, media identity and PCI
save/restore. The desktop and command results above do not establish Windows
snapshot qualification.

Toets completed an unattended installation from an empty 64 GiB disk on the same
host on 3 November 2026. The run used Firecracker `bc0ecd98e`, Toets `997301b`,
the prepared OEM ISO below and KB5126052. Setup installed the signed drivers and
update, completed all reboots, removed recovery files and zeroed free space. It
reported `BUILD OK` and shut down through PSCI; Firecracker returned exit code
0\. Toets published a read-only golden disk with 6.2 GiB of allocated storage.
The run took just under two hours, without operator input.

Toets qualification with `1729b4cca` remains incomplete. One cold run reached
the desktop but timed out before its serial `READY` marker. A second received
`READY` and passed desktop and Run-dialog checks, then timed out waiting for the
device-preparation PowerShell command. Its execution script and empty output
file were present on the retained disk. No state or application pass is
established by these runs.

A third cold run, `18e4a78d27e2d220-24413`, passed preparation, another boot and
both token exit-code checks. This run used host `khugepaged` settings
`pages_to_scan=65536`, `scan_sleep_millisecs=10` and
`alloc_sleep_millisecs=100`; they have not established a fix. Its restored run,
`18e4a89e6f11dbaf-24413`, loaded the snapshot but timed out opening the Start
menu after 60 seconds.

A retained-state diagnostic found advancing PMU counters, timer interrupts,
input queue consumption, disk requests and Windows process accounting after
restore. The requested shell windows did not open, and an ACPI power request did
not shut down within two minutes. These observations ruled out a fully stopped
guest, but did not identify the cause.

Further diagnostics compared memory backing for the same retained state on
this nested host. Copying its RAM into anonymous 2 MiB pages through the
existing UFFD handler restored command execution and clean ACPI shutdown. A full
copy into 4 KiB pages still failed to return a command result within two
minutes. Transparent huge pages also restored commands after promotion. These
diagnostic runs used manual input and did not control host CPU load. They
are not unattended qualification results and do not isolate the cause of
the timing difference.

Revision `b725627e8` adds `mem_backend.backend_type: "Anonymous"` to
`/snapshot/load`. It copies snapshot RAM into anonymous memory before vCPUs
resume, using the snapshot's huge-page setting. No external UFFD handler or
kernel patch is needed. Use `huge_pages: "Transparent"` for this host and
`memory_backend = "Anonymous"` in the Toets worker guest profile. Private
file-backed restore can leave guest writes on small copy-on-write pages. The
anonymous backend requires physical memory for the full guest RAM size.

A direct restore of the original failed state loaded 8 GiB in 1.55 seconds,
returned `ANONYMOUS_RESTORE_PASS`, and shut down through PSCI with exit code 0.
This was a manual diagnostic with the settings above. The default `File` backend
remains available; AMD64 XP and Windows 11 passed cold and restored token tests
with that backend on `b725627e8`.

Toets `84a943b` then passed Windows ARM64 cold token run
`18e4acc805d1c097-29879` and restored token run `18e4adacc4e6ca7e-29879` with
the final `b725627e8` build and `Anonymous` backend. Both ran without operator
input against the newly installed image. The restored test completed in 50
seconds and checked commands with exit codes 0 and 1.

On 4 November 2026, lifecycle run `18e4adfa569ff94c-30177` also passed
checkpoint creation, network isolation, reboot and retained-file checks on the
same runtime and host configuration. Run `18e4aed1bfc56e0a-30394` then replayed
that checkpoint and passed the retained-file, network-isolation and reboot
checks again.

The first ARM64 application run passed two of four tests. The other two timed
out at the initial Start-menu check. Inspection of the UAC snapshot's RAM
framebuffer showed that Start was already open. Toets clicked Start again, which
could close it, then waited for it to open. Toets revision `56d3362` accepts an
open Start menu before closing it. Input hold times and test time limits remain
unchanged.

Application run `18e4b395ad848230-32662` then passed all four ARM64 cases with
Toets `56d3362` and Firecracker `b725627e8`: EXE and MSI, each through the
installer UI and silently. It used the original UAC profile snapshot, the
`Anonymous` memory backend and default test time limits. The first desktop check
accepted the already-open menu and completed in 0.21 seconds.

For firmware guests, `SendCtrlAltDel` signals the ACPI control-method power
button `PNP0C0C` through a Generic Event Device. It uses the existing power
interrupt. Direct Linux boots retain the PL061 GPIO input path. The
[ACPI power-button contract](https://uefi.org/specs/ACPI/6.5/04_ACPI_Hardware_Specification.html#power-button)
defines the `Notify(PWRB, 0x80)` event; guest policy determines the response. On
the native Windows ARM64 guest above, revision `1729b4cca` accepted this API
action and received PSCI shutdown after 16 seconds. Firecracker exited zero.

ARM64 GED resources use the architectural interrupt number: the KVM GSI plus 32.
The shared event helper applies this conversion to the power button, VMGenID and
VMClock. The regression tests execute the power-button AML and check interrupt
delivery without a GPIO driver.

### Framebuffer implementation

The ARM console uses EDK II's `QemuRamfbDxe` and firmware-reserved guest RAM.
The `fw_cfg` MMIO/DMA interface at `0x10002000` advertises `etc/ramfb`. It
implements the
[documented firmware interface](https://www.qemu.org/docs/master/specs/fw_cfg.html);
no QEMU executable is used. DMA transfers are limited to 1 MiB and guest RAM.
Scanout accepts `XRGB8888`, zero flags, dimensions from 16 to 2048 pixels and a
buffer of at most 16 MiB. An unconfigured or invalid mode produces a blank
800x600 frame, so direct Linux boot can connect to the console without GOP.

There is no ARM display PCI BAR. Windows PCI enumeration cannot remove the boot
framebuffer. AMD64 retains its PCI display. Since format 46, snapshots store the
framebuffer configuration and `fw_cfg` registers; pixels use the ordinary guest
memory snapshot. Cold boot requires the matching RAM framebuffer firmware.

On 3 November 2026, native Linux `6.8.0-142-generic` passed 40 device-manager
tests, four FDT tests and the maximum device-state snapshot test. AMD64 passed
all 66 console tests and the maximum device-state snapshot test. ARM default and
Windows-enabled lint checks passed, as did AMD64 Windows lint. The ARM console
DEBUG/RELEASE and secure RELEASE firmware builds passed.

On Linux `6.8.0-142-generic`, an installed Windows ARM64 image passed the
previous PCI framebuffer fault and reached userspace. A full 4 GiB memory
snapshot was created and restored. These checks do not establish desktop or
input qualification: the desktop did not appear during 27 minutes of
observation, the display became black during startup, and the captured input
device was inactive. The existing golden image was not changed.

On Linux `7.0.0-38-generic`, VMM revision `d89011f97` reached the `build`
account's `Welcome` display after 30 minutes. An earlier memory capture had 37
processes, with `winlogon.exe`, `LogonUI.exe` and `dwm.exe`, and
`BasicDisplay.sys` and `BasicRender.sys` loaded. The black display before
sign-in did not establish a display failure. The virtio input device was
inactive. The disk had no `vioinput` package in its driver store and no
`viohidkmdf.sys` in its driver directory. Signed ARM64 input-driver installation
and keyboard/pointer qualification had not passed in that run. The format-47
results above include the installed input driver.

The format-46 VMM created and restored a full snapshot on Linux 7.0; the
restored display pixels matched the snapshot frame. The manual restore first
failed on the stopped VMM's stale console socket, then passed after its removal.
This does not establish Windows interaction or guest state acceptance. The test
stopped without a desktop result.

The Ubuntu 24.04 ARM64 image passed cold and restored token tests with
`d89011f97`, format 46, on Linux `7.0.0-38-generic`. On Linux
`6.8.0-142-generic`, the cold test passed, but the restored token test timed out
after the VMM loaded the snapshot. These results apply to this nested Apple host
and image; they do not identify the restore timeout's cause.

## Prepare an ARM64 disk

Obtain the English-US ARM64 edition of
[Windows 11 IoT Enterprise LTSC 2024 Evaluation](https://www.microsoft.com/en-us/evalcenter/download-windows-11-iot-enterprise-ltsc-eval)
and the virtio-win driver ISO. Image 1 of the tested Windows ISO is ARM64. Its
SHA-256 is:

```text
3dcdba9c9c0aa0430d4332b60c9afcb3cd613d648a49cbba2d4ef7b5978f32e8
```

The current preparer has a separate `win11-iot-arm64` staging profile for the
Toets OEM ARM64 ISO, whose hash differs from the historical Evaluation ISO
above. It creates an extracted payload tree and stages ARM64 drivers; it does
not service `boot.wim` or build a bootable installer. See the
[ARM64 staging procedure](../tools/windows/README.md#arm64-installation).

The earlier disk was prepared under QEMU/AAVMF. That result is historical; the
QEMU preparation generator has been removed. The native installation above used
the prepared ISO with SHA-256
`7a42985e9b2df2921e09fcc73bd0f743b64d4a98093c5fee6bbffb9edcba0b22`. Both WinPE
images contain signed ARM64 viostor, vioscsi, vioserial, vioinput and NetKVM
drivers. Toets supplies the installed OS drivers through the answer's
`offlineServicing` pass. Follow the linked staging procedure, then the
[Toets image-build procedure](https://git.bloudraad.io/bloudraad/toets/src/branch/main/docs/how-to/build-image.md).
ARM64 uses its RAM framebuffer and combined virtio input profile; see
[console configuration](windows-console.md#arm64). The AMD64 option ROM and
ATAPI preparation commands do not apply.

## Firecracker boot configuration

Build the
[ARM64 CloudHv profile](../resources/firmware/cloudhv-secureboot/arm64/README.md)
that matches the configured devices: `ArmVirtCloudHvConsole.dsc` for graphical
input, or `ArmVirtCloudHvSecure.dsc` with TPM and protected variables. The
original `ArmVirtCloudHv.dsc` is headless; its earlier build is described in the
[firmware index](../resources/firmware/README.md#arm64-firmware). Each produces
a raw `CLOUDHV_EFI.fd` in its own output directory. Use that image as the boot
source. Do not supply an initrd. Unlike AMD64 PVH boot, this path loads raw
firmware at `0x80200000`, with the FDT at `0x80000000` and the ACPI root at
`0x801f0000`.

Declare the CloudHv ABI with `firmware.type: "cloud-hv"` and a stable
`firmware.system_uuid`. Firecracker reserves one 4 KiB page at `0x801e0000` and
publishes the SMBIOS system UUID there before constructing ACPI tables. Use the
same UUID for the guest throughout its lifetime. Snapshot restore retains the
saved identity and RAM. Without this declaration, Firecracker does not reserve
or write the SMBIOS page. SeaBIOS is supported only on AMD64. This identity path
does not qualify ARM Secure Boot or a Windows installation.

The following configuration limits the 52-bit validation host to 48 bits:

```json
{
  "boot-source": {
    "kernel_image_path": "CLOUDHV_EFI.fd",
    "boot_args": "",
    "firmware": {
      "type": "cloud-hv",
      "system_uuid": "11223344-5566-7788-99aa-bbccddeeff01"
    }
  },
  "machine-config": {"vcpu_count": 2, "mem_size_mib": 4096, "track_dirty_pages": true, "vmclock": false},
  "cpu-config": {
    "vcpu_features": [{"index": 0, "bitmap": "0b1xxx"}],
    "reg_modifiers": [{"addr": "0x603000000013c038", "bitmap": "0b0101"}]
  },
  "drives": [{
    "drive_id": "windows",
    "path_on_host": "windows.raw",
    "is_root_device": false,
    "is_read_only": false
  }],
  "vsock": {"guest_cid": 3, "uds_path": "vsock.sock"}
}
```

Start with
`firecracker --enable-pci --api-sock windows.sock --config-file windows.json`.
CPU-template bit 3 enables `KVM_ARM_VCPU_PMU_V3`. Firecracker assigns PPI 23 and
initialises the PMU after creating the interrupt controller. Restore recreates
the PMU from the saved vCPU feature flags. VMClock is explicitly disabled in
this example; its default remains enabled. Restore retains the saved clock
presence, independently of VM Generation ID. The AMD64 `hyperv` machine field
does not apply to ARM64.

The register modifier sets `ID_AA64MMFR0_EL1.PARange` to 48 bits. On the 52-bit
validation host, firmware creates five-level translation tables. The tested
Windows loader then selects four levels while retaining that table address and
faults. Exposing 48 bits makes firmware use four levels and allows Windows
kernel execution. On hosts already exposing 48 bits or less, omit this modifier;
it must not increase the host's address width.

## Optional security devices

The ARM64 security path uses the same private TPM and variable-helper sockets as
AMD64. Its addresses differ because AMD64's security addresses can overlap ARM
RAM. The ARM pages are outside dynamic MMIO and PCI apertures and below the GIC
regions. They leave ordinary Linux allocations unchanged. Restore rejects saved
device mappings that overlap a selected security page:

| Device                                    | ARM64 address | Size  |
| ----------------------------------------- | ------------- | ----- |
| TPM CRB locality zero                     | `0x10000000`  | 4 KiB |
| Protected variables and physical presence | `0x10001000`  | 4 KiB |

ACPI describes the TPM with start method 7 and control area `0x10000040`. The
variable page also has the platform-bus device-tree binding consumed by the
external firmware. These firmware ABI names do not add a QEMU runtime
dependency. ARM retains its hardware-reduced FADT and PSCI shutdown path.

A secure guest requires the secure ARM firmware profile and dedicated helpers;
the ordinary ARM image does not establish Secure Boot or TPM initialisation.
Preserve the matched disk, TPM, variable store and memory checkpoint as
described in the [Secure Boot guide](windows-secure-boot.md). A snapshot with a
security device requires its corresponding destination helper and explicit
variable-store generation. Failed restoration must discard the isolated staging
copies.

The native security qualification above covers the recorded firmware and
Windows runtime. A fresh secure installation and CONNECT with the protected
firmware profile remain separate checks. The earlier emulated egress results
below do not qualify this security path.

## Egress and validation

Signed ARM64 NetKVM drivers are required for TAP networking. The vsock CONNECT
path needs the ARM64 viosock driver and Winsock provider. Match every driver and
boot image to the selected Windows edition and architecture.

On 4 November 2026, native TAP egress passed on the Apple/Linux 7.0 worker with
runtime `b725627e8` and Toets `56d3362`. A cold Windows ARM64 boot received HTTP
200 from `http://example.com/` and HTTPS 200 from `https://www.microsoft.com/`
through WinHTTP with proxies disabled and certificate validation enabled. The
probe asserted native `ARM64` execution. Toets cancellation returned exit code
3, and checks found no retained namespace, bridge or forwarding rules. The log
is `run-1793793393768090646-1468.log` in the qualification store.

### Native CONNECT

On 4 November 2026, the native ARM64 Windows guest passed cold, full and
differential CONNECT checks on the same Apple/Linux 7.0 host. The VMM source was
`210c7565c` with the `CLOUDHV_RAMFB` firmware profile and no guest NIC. The
virtio-win 0.1.302 source ISO had SHA-256
`303f7ae40dad495d6ae474fdc571df58958a4dbc5c37a522d80f9a203867949d`.
The driver catalogue validated as Microsoft Windows Hardware Compatibility
Publisher. Driver installation returned zero and the device reported error
code zero. The driver, Winsock provider, provider service and Rust probe all
had native ARM64 PE headers. The installed provider and service matched the
source file hashes before each transfer. No test-signing mode was enabled.

The test used the existing native Tokoloshe proxy helper and sidecar from
`dedf6eedef6b612ccd3f4bd0d24d7325756d4e19`, running as UID/GID 1000. The guest
connected to host CID 2, port 4750. Each phase compared all 2,097,152 bytes of
an allowed echo through a socket grant to `127.0.0.1:40567`. Requests to port
40568 received HTTP 403 with no payload. A retained disk marker checked phase
order across restores. Each phase opened fresh connections after the preceding
checkpoint. Sealed sidecar accounting contained exactly six entries: three
`Connected` entries with 2,097,152 bytes in each direction and three
`PolicyDenied` entries with zero bytes. After normal PSCI shutdown, Firecracker
and the proxy both exited zero. Read-only offline verification found phase 3
on disk and no hibernation file. The verification loop and mount were removed.

Normal execution and restored probes used `Anonymous` memory with dirty logging
disabled. The differential checkpoint captured a 20-second tracked interval
and wrote a 95 MiB RAM delta. Rebase changed the base memory hash, then the
differential state loaded with tracking disabled and passed its transfer and
provider checks. Later tracked state checks above did not repeat CONNECT. These
CONNECT checks used the ordinary firmware profile; they do not qualify the
combined protected-variable/TPM and CONNECT profile.

### Historical emulated results

The ARM64 virtio-win ISO contains `viosock/w11/ARM64`. The preparation test
staged that signed driver and a native ARM64 Winsock probe. The initial SYSTEM
task incorrectly required an interactive token. Its replacement uses
`ServiceAccount`; its transcript confirms SYSTEM execution before login. The
probe uses host CID 2, port 4750 and HTTP CONNECT, as described in the
[AMD64 transport guide](windows.md#vsock). The actual Tokoloshe ARM64 Linux
sidecar passes allowed 2 MiB echo requests and denies the adjacent port. Guest
byte comparisons and sealed sidecar accounting confirm these results before and
after full, differential and subsequent full snapshot restore. No guest network
device is configured. Applications must reconnect after restore; a connection
captured during a transfer fails, then the probe establishes a fresh connection
successfully.

Conventional virtio-net/TAP egress also passes in Firecracker with default
seccomp and no vsock device. The SYSTEM probe configures NetKVM at
`172.30.253.2/30`, with TAP gateway `172.30.253.1` and DNS `10.0.2.3`. The Linux
appliance enables forwarding and masquerades that subnet through `enp0s3`. The
guest resolves `www.microsoft.com` and receives HTTPS 200 with certificate
validation enabled and proxies disabled. It then checks every byte of a
2,097,152-byte TCP echo exchange with the LAN host. Independent host accounting
confirms the same count. The transcript completes at 17:39:59 UTC on 2026-10-13;
metrics contain no seccomp faults, VMM panics or vCPU failures. Test processes,
disposable disk and network resources are removed after evidence collection.
Logs and the PowerShell probe are under `build/windows/arm64-host` in the
validation worktree.

Completed checks include firmware PCI disk discovery, musl/seccomp firmware
boot, PMU initialisation and register restore, firmware-loader rejection tests,
and Linux direct boot with full and differential snapshot restore. ARM firmware
boot rejects an initrd before copying it into guest memory. Snapshots contain
memory and device state, not the disk. Keep a disk copy from the same paused
state and restore that copy with the memory snapshot. Only one VM may write a
disk at a time.

The complete evidence and remaining work are in the
[validation record](windows-validation.md).

The historical format-20 source rejected earlier snapshots. The current snapshot
contract is documented in [snapshot versioning](snapshotting/versioning.md).
ARM64 accepts only modern PCI. The format-19 binary
`068748352323dd47b17949862c9497cade8623a0bf4fc230db18dd0077530b7c` passes cold
boot, full restore and differential restore with a SYSTEM probe that flushes and
compares 2 MiB of random disk data, verifies an allowed Tokoloshe CONNECT echo
and checks a denied destination. Sealed accounting records exactly three allowed
2 MiB transfers in each direction and three denied requests with no payload.
After the final probe, Windows requests shutdown and Firecracker exits zero.
Read-only disk inspection verifies the final file against its recorded hash and
confirms that hibernation is disabled. Evidence is in
`build/windows/arm19/run/`; see the [format-19 result](windows-validation.md).

The earlier TAP and subsequent-full results above used format 12. The later
format-19 TAP run passes cold, full and differential storage, DNS, validated
HTTPS and byte-checked 2 MiB TCP workloads with the PCI reset and eventfd
seccomp fixes. Offline disk verification passes, but the controller stops the VM
after its five-minute shutdown limit; a clean shutdown is not established.
Standard online activation succeeds and the guest reports licensed state before
these workloads. Earlier retries ended in evaluation-licence shutdowns. The
format-20 TAP rerun passes all three workloads and clean shutdown. Firecracker
receives the guest shutdown event and exits zero; offline verification confirms
three guest passes, three complete TCP echoes and the final storage hash.
Evidence is in `build/windows/arm20-tap/run/`. The final format-20 CONNECT run
also passes cold, full and differential storage and proxy checks, followed by
guest shutdown and Firecracker exit zero. Offline verification confirms SYSTEM
identity, three guest passes, six sealed accounting entries and the final 2 MiB
storage hash. Each allowed tunnel transfers 2 MiB in each direction; each denied
request transfers no payload. Evidence is in `build/windows/arm-connect20/run/`.
Both final runs use binary SHA-256
`f7f80394d1a8ebe3d6319107862b63ad63780b8e4b75793f601c49e2ecc879a4`. These tests
do not establish native ARM performance. The same format-20 binary passes Linux
direct boot and full/differential restore with a counter probe. Each restore
continues the counter without repeating its boot marker; evidence is in
`build/windows/arm-linux20/`. See the validation record for the tested scope.
