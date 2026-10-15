# Windows guests (AMD64, experimental)

This branch runs AMD64 Windows Server 2025 Core, Windows 10 and Windows 11
on Linux/KVM with virtio PCI storage, networking and full/differential
snapshots. XP x86 and Windows 7 x64 also pass the legacy checks below.
Support is experimental. It does not add a Windows host backend. See the
[ARM64 guide](windows-arm64.md) for the separate firmware
path and its validation status.

## Boot configuration

All CloudHv firmware profiles require the [PVH memory-map correction](../resources/firmware/pvh-memory-map.md).
Earlier firmware could expose platform device pages as usable RAM and corrupt
Windows page tables on restore. Cold boot corrected firmware before creating
new snapshots; loading an old snapshot does not update its guest memory map.

The intended boot path is EDK II CloudHv PVH firmware, a GPT disk with an
EFI system partition, and modern virtio PCI storage and networking. Start
Firecracker with `--enable-pci`. Use the firmware ELF as the boot source;
do not pass the Windows installation ISO as a kernel or root disk.

Example configuration for a prepared disk:

```json
{
  "boot-source": {
    "kernel_image_path": "CLOUDHV.fd",
    "boot_args": "",
    "firmware": {
      "type": "cloud-hv",
      "system_uuid": "11223344-5566-7788-99aa-bbccddeeff01"
    }
  },
  "machine-config": {
    "vcpu_count": 2,
    "mem_size_mib": 4096,
    "hyperv": true
  },
  "drives": [{
    "drive_id": "windows",
    "path_on_host": "windows.raw",
    "is_root_device": false,
    "is_read_only": false
  }]
}
```

Save the configuration as `windows.json`, then run:

```sh
firecracker --enable-pci --api-sock windows.sock --config-file windows.json
```

Replace the example UUID with a unique UUID for the VM and retain it across
cold boots and restores. The explicit `cloud-hv` profile publishes a bounded
SMBIOS Type 1 system identity through the CloudHv firmware ABI. It reserves
one page at `0xF0000` and rejects overlapping ELF load segments. This is a
minimal identity table, not a complete SMBIOS hardware inventory. The profile
requires AMD64 PVH firmware and no initrd. Omit it for SeaBIOS and Linux kernels;
the VMM does not infer the firmware ABI from a filename or PVH note. The UUID
is separate from VM Generation ID and does not change on snapshot restore.

The terminal is the guest serial console. Once SAC reports that CMD is
available, enter `cmd`, wait for the new channel, then enter `ch -si 1`.
Press Enter and follow the login prompts individually.

`hyperv` defaults to `false`. PUT `/machine-config` resets an omitted
`hyperv` field to `false`; PATCH preserves an omitted field. The field is
available only on x86-64. It installs a fixed set of KVM Hyper-V
enlightenments after applying the CPU template. Unsupported host
capabilities produce an error before the guest starts.

The firmware receives the ACPI root through the PVH start information.
The platform supplies a 32-bit ACPI PM timer at `0x608`, PM1 event and
control blocks at `0x602` and `0x606`, and the CloudHv reset/S5 port at
`0x600`. SCI uses interrupt 3, with a level/low MADT override. Reset and S5
terminate the Firecracker process, like the existing i8042 reset operation. An external supervisor must start a
new process for a guest reboot.

## Prepare the disk

1. Download the AMD64 ISO from the
   [Microsoft Windows Server 2025 Evaluation Center](https://www.microsoft.com/en-us/evalcenter/download-windows-server-2025).
2. Obtain the [virtio Windows drivers](https://fedorapeople.org/groups/virt/virtio-win/direct-downloads/stable-virtio/)
   and build the [Firecracker CloudHv firmware](../resources/firmware/README.md).
   Record resolved versions and SHA-256 digests with the test results.
3. Install Server Core under QEMU with OVMF, a raw disk, and a modern
   `virtio-blk-pci` device (`disable-legacy=on`). Load the AMD64 `viostor`
   driver during installation. Install `NetKVM` for `virtio-net-pci`.
4. Provide the removable-media EFI boot path on the system partition.
   QEMU's persistent UEFI boot variables are not transferred with the disk.
   Run `bcdboot` before configuring EMS: it replaces existing boot settings.
5. Enable the Windows Special Administration Console from an elevated
   command prompt:

   ```cmd
   bcdedit /emssettings emsport:1 emsbaudrate:115200
   bcdedit /ems {default} on
   bcdedit /bootems {bootmgr} on
   ```

6. Shut down QEMU before giving Firecracker writable access to the disk.

To install directly in Firecracker, use the
[optional installation console](windows-console.md), which supplies VGA,
PS/2 keyboard, USB HID pointer and ATAPI hardware. That guide records the tested editions and limits.
An [unattended preparation tool](../tools/windows/README.md) generates the
answer media and a separate raw disk without starting the installer.
See the [Cloud Hypervisor Windows guide](https://github.com/cloud-hypervisor/cloud-hypervisor/blob/main/docs/windows.md)
for the upstream preparation procedure.

## Networking

Add a network interface backed by an existing host TAP:

```json
"network-interfaces": [{
  "iface_id": "net0",
  "host_dev_name": "tap0",
  "guest_mac": "06:00:ac:1e:fc:02"
}]
```

The NetKVM driver must be installed in the guest. Configure DHCP or a static
address for the host network. Firecracker does not provide DHCP or NAT.
In SAC, `i` lists interfaces and `i -?` describes address configuration.

## Vsock

Windows can use virtio-vsock through the virtio-win `viosock` driver and its
Winsock provider. Add this device to the boot configuration:

```json
"vsock": {
  "guest_cid": 3,
  "uds_path": "windows.vsock"
}
```

Install `viosock/2k25/amd64/viosock.inf` from the virtio-win ISO with
`pnputil /add-driver <path> /install`. The package installs the
`VirtioSocketWSP` service and both Winsock provider DLLs. Check the service
with `sc query VirtioSocketWSP`. The network interface is not required.

The host uses Firecracker's [Unix socket protocol](vsock.md): connect to
`windows.vsock` and send `CONNECT <guest-port>\n`, or listen on
`windows.vsock_<host-port>` for a guest connection to CID 2.

Windows programs must use the installed provider's address family, obtained
through `ViosockGetAF`, and its `SOCKADDR_VM` layout. Do not assume Linux's
numeric `AF_VSOCK` value. The upstream
[socket header](https://github.com/virtio-win/kvm-guest-drivers-windows/blob/master/viosock/inc/vio_sockets.h)
and `viosocklib-test.exe` describe the interface.

Existing vsock connections do not survive snapshot restore. The guest agent
must handle transport reset, reconnect and complete a new readiness handshake.

The device accepts both event-queue notification and the required CID reread
as reset acknowledgement. This supports Windows, which replenishes its event
buffers without notifying that queue. The device supports indirect buffers
on the receive, transmit and event queues.

### Tokoloshe egress

Tokoloshe's socket-grant proxy accepts HTTP CONNECT on host CID 2, port 4750.
A Windows client opens a provider socket to that endpoint, sends
`CONNECT <host>:<port> HTTP/1.1` with a matching `Host` header and an empty line,
then waits for status 200 before sending tunnel data. Bind the granted host
Unix socket at `windows.vsock_4750` in the Firecracker namespace.

The format-19 AMD64 test uses the actual `tokoloshe-egress` sidecar and a grant
for one host loopback echo endpoint. Windows writes and flushes 2,097,152
random bytes, reads them back, then checks every byte through CONNECT.
Another port returns 403. These checks pass after cold boot, full restore and
differential restore, without a guest network device. Sealed proxy accounting
records three connected tunnels, each with 2,097,152 bytes in each direction,
and three policy denials with zero payload bytes. After S5, read-only disk
inspection confirms the final file's recorded hash. Evidence is retained in
`build/windows/server19/`.

This validates the Windows transport and proxy protocol. The test used a
native Winsock probe; the full Tokoloshe guest agent is not ported to Windows.
HTTPS certificate validation and proxy configuration in Windows applications
remain application responsibilities.

## Conventional network egress validation

Windows Server 2025 also passes a conventional IPv4 egress test with
NetKVM, one Firecracker TAP interface and no vsock device. A separate host
network namespace routes guest traffic over a veth pair, with source NAT
on the host uplink. The test verifies DNS resolution, HTTPS with certificate
validation and proxies disabled, and byte equality for a 2 MiB bidirectional
TCP echo transfer. Independent server accounting confirms the byte count.
Windows then requests ACPI S5 and Firecracker exits successfully.

This result covers static IPv4 configuration. It does not establish DHCP,
IPv6 or legacy Windows compatibility. Windows ARM64 also passes DNS,
HTTPS and the same 2 MiB bidirectional TCP test through a TAP interface;
see [ARM64 validation](windows-arm64.md).

## Snapshots

This branch uses snapshot format 27.0.0, which persists the optional CloudHv
firmware profile and system UUID. Format 26.0.0 added the physical-presence mailbox.
Format 25.0.0 added optional TPM and authenticated firmware variable state; see
[the secure platform](windows-secure-boot.md).
Format 23.0.0 added USB controller/HID state, keyboard lock tracking and pending
input releases.
Format 21.0.0 introduced the console devices. Console and headless revalidation
for the current format is tracked in the
[console plan](windows-console-plan.md). The preceding reviewed
format 20.0.0 added CET control and shadow-stack state for guests that expose
those CPU features. A native vCPU regression
verifies nonzero registers across two fresh restores. Server 2025, Windows 10
and Windows 11 pass the
format-20 cold/full/differential TAP and real Tokoloshe CONNECT regressions.
Windows 7 passes storage, DNS, validated HTTPS and TAP; XP passes storage,
HTTP and TAP. Both pass cold/full/differential restore and clean shutdown.
ARM64 passes the same format-20 TAP and CONNECT checks, including clean
shutdown. The format preserves modern and transitional
PCI transport state, including the legacy network I/O BAR size. Earlier
snapshot formats are incompatible. It saves the ACPI PM timer,
PM1 status, enable and sleep-type registers, CMOS registers, RTC divider
phase, pending interrupt flags and guest wall-clock adjustment.
The RTC follows host UTC plus the saved guest adjustment, including elapsed
time while the VM is stopped. It provides periodic, alarm and update
interrupts on reserved IRQ 8. Register C reads acknowledge pending flags.

The VMClock device publishes generation and disruption counters. Its
`clock_status` remains `UNKNOWN` and its `counter_id` remains `INVALID`; this
implementation does not advertise a usable precision clock. AWS provides a
[null driver package](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/other-windows-device-drivers.html)
that lets Windows recognise the device without adding clock functionality.
AWS documents support for Windows Server 2016, 2019, 2022 and 2025. Windows 10
recognised `AMZNC10C` as `AWS VMClock Device` with device problem code 0 after
installation of the unmodified signed INF/catalog, without reboot. This verifies
device recognition, not counter consumption. No functional Windows VMClock
driver has been verified. Installing the null package does not resolve guest time lag;
clock accuracy requires separate validation.

The guest CPUID records whether Hyper-V is enabled. Restore derives the
machine configuration from that state and checks destination-host support.
The vCPU snapshot includes Hyper-V control MSRs, SynIC state, and synthetic
timer configuration and counts. SynIC2 avoids clearing restored guest
message and event pages. The reference clock is restored through the VM
clock state, rather than writing its read-only counter MSR.

The x86 FADT publishes conventional ACPI fixed hardware and an aligned
FACS. The PM timer resumes from its saved count and signals SCI when an
enabled timer-status event is pending. Register/event restore tests and
Windows Server 2025 full restore pass. The restored authenticated session
reads its saved file and requests S5. Server 2025 also passes format-19 cold,
full and differential workloads for storage, TAP and Tokoloshe CONNECT, then
requests S5. ARM64 retains hardware-reduced ACPI; see its separate
[validation results](windows-arm64.md#egress-and-validation).

## Validated behaviour

Windows Server 2025 Standard Evaluation Core, build 26100.32230, was tested
with two vCPUs and 4 GiB RAM on an AMD64 Linux/KVM host:

- Boot and authenticated commands through SAC.
- Boot with synchronous and asynchronous block I/O; file writes and reads
  through the authenticated command session.
- NetKVM networking through an isolated TAP, with guest-to-host ping.
- Full snapshot restore and differential snapshot rebase/restore, preserving
  the command session, stored file and network traffic.
- A new full snapshot after differential restore.
- ACPI S5 shutdown and a guest-requested reboot followed by a new process.

Linux 6.18.38 also passed direct boot, full restore and differential restore
with `hyperv: false`. Focused unit tests, Clippy and the musl release build
pass. The full upstream integration suite has not been run.

Snapshots do not include the disk. Preserve a matching disk when retaining
or copying a memory snapshot. Do not restore old memory against a disk that
has since changed. Only one VM may write the disk at a time.

Firecracker's virtual CMOS clock uses UTC. Prepare modern Windows images with a
matching RTC policy: use the UTC timezone (`tzutil /s "UTC"`), or retain the
chosen display timezone and set the DWORD `RealTimeIsUniversal` to `1` under
`HKLM\SYSTEM\CurrentControlSet\Control\TimeZoneInformation`, then reboot.
Microsoft documents [tzutil](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/tzutil);
AWS documents the [UTC RTC policy for Windows guests](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/change-time-zone-of-instance.html).
These are image configuration choices; Firecracker does not change the guest's
timezone or registry.

The final Server test had Pacific time selected and no `RealTimeIsUniversal`
value. Its cold UTC was seven hours ahead of the host. Windows System event 1
records a correction of -25,194,836 ms, and Time-Service event 35 identifies
`time.windows.com` as the NTP source. This explains the apparent seven-hour
change during the TAP run. The subsequent no-NIC cold boot used Local CMOS Clock
and reproduced the initial offset. Host timestamps, rather than the guest wall
clock, identify each network-test phase. The diagnostic did not change guest time
settings.

A controlled Windows 10 AMD64 check with Hyper-V enabled and the format-23
`firecracker-usb-fifth` binary distinguishes pause from restore. During a
30.014-second pause, UTC and the performance counter both advanced. Full restore
with the default `clock_realtime: false` omitted the 33.977-second stopped
interval. Differential restore with `clock_realtime: true` compensated its
31.018-second stopped interval in both clocks. The performance-counter frequency
was 10 MHz. Evidence is retained in `build/windows/acceptance23/clock`.

That guest reported `Pacific Standard Time` and had no `RealTimeIsUniversal`
registry value; its initial UTC offset of seven hours was separate from restore
accounting. The test did not change the timezone. These short measurements do
not establish long-running clock accuracy or functional VMClock-driver use.
No VMBus time synchronisation service is provided. Cross-host migration and
firmware-stage snapshots remain unvalidated.

These Toets AMD64 goldens pass with four vCPUs, 8 GiB, SeaBIOS and transitional
PCI:

| Guest | Build |
|---|---|
| Windows 10 Enterprise LTSC 2021 | 19044.7725 |
| Windows 11 IoT Enterprise LTSC 2024 | 26100.1742 |

Cold boot, full restore
and differential restore each pass flushed storage, DNS, certificate-validated
TLS 1.2 HTTPS and a byte-checked 2 MiB TAP transfer. A separate NIC-free run
passes the real Tokoloshe CONNECT proxy over Viosock in all three stages:
three allowed 2 MiB transfers each way and three policy denials with zero
bytes. Each run ends with guest S5 and Firecracker exit zero. Offline checks
verify guest transcripts, absence of hibernation and the final disk hashes.

This boot requires preserving PCI MSI-X configuration across virtio resets;
reset clears virtio event assignments and pending state, while retaining the
MSI-X table, mask and enable bits. No guest driver or kernel patch is used.

Progress and test evidence are tracked in [the implementation plan](windows-support-plan.md).

## Legacy Windows validation

Fresh Windows XP SP3 32-bit and Windows 7 Enterprise SP1 64-bit installations
made with Toets' pinned media pass Firecracker cold boot, storage, TAP egress,
full and differential restore, real Tokoloshe CONNECT over vsock, and ACPI
S5 shutdown with format 20 and default
seccomp:

| Guest | Firecracker result | Remaining platform requirements |
|---|---|---|
| XP SP3 x86 | Passes with SeaBIOS, transitional PCI and one vCPU using its UP HAL. | HTTPS and SMP are not established by this test. Viosock uses a custom test-signed driver. |
| Windows 7 SP1 x64 | Boots, writes files, passes TAP egress after cold boot and full/differential restores, and shuts down through ACPI S5. | Viosock uses a custom test-signed driver. |

Windows 7 is tested on a separate GPT/EFI copy of its installed disk. The
original `0xc0000225` loader error occurs with QEMU and OVMF 2025.11, both
with and without VGA. A debugger trace isolates it to a 512 KiB allocation
at address zero: firmware advertises that page as free but rejects the
allocation. The CloudHv firmware patch now reserves the page. The original,
unmodified Windows 7 loader passes that stage in Firecracker. Windows Server
2025 also passes boot, console login, file write/read and S5 shutdown with
the changed firmware. Restoring UGA in an earlier experiment did not resolve
the allocation failure.

The kernel's next failure is `MISMATCHED_HAL` (`0x79`, parameter 6): its HAL
requires the MADT `PCAT_COMPAT` flag. Firecracker now declares the dual 8259
controllers that it already creates in KVM. That check passes. The conventional FADT now publishes the PM timer, and
unmodified Windows reads port `0x608`. It next stops with
`HAL_INITIALIZATION_FAILED` (`0x5c`, parameters `0x10b, 1, 0, 0`) while
waiting for an RTC clock interrupt. The RTC implementation passes this stage.
The next `0x7b` failure comes from the PCI root bus failing resource
allocation: its I/O window overlaps the active RTC. The PCI aperture now
starts at `0x1000`, above the fixed platform ports. ECAM is reserved by a
separate PNP0C02 motherboard device with a unique ID.
A separate KVM regression also found and corrected lost
legacy interrupt routes when installing device/MSI routes.

The production build with default seccomp boots Windows 7 without guest
kernel patches. A startup command records Windows version `6.1.7601`, the
`win7\build` identity, the process list and a file-write marker. It then
requests shutdown; Firecracker records ACPI S5 and exits zero. The proof
file is recovered from the stopped disk with a read-only mount. Evidence
is in `build/windows/legacy/win7-storage-boot/userspace-proof.txt` and the
matching `pci-io-vmm.log`.

Windows 7 also passes DNS resolution, direct TLS 1.2 HTTPS with certificate
validation and a 2 MiB TCP payload-equality test through NetKVM and TAP.
Fresh SP1 media needs a current trust root for this HTTPS test. The tested
`www.microsoft.com` chain terminates at DigiCert Global Root G2. Download the
[official DER certificate](https://cacerts.digicert.com/DigiCertGlobalRootG2.crt)
and require SHA-256
`cb3ccbb76031e5e0138f8dd39a23f9de47ffc35e43c1144cea27d46a5ab1cb5f`
before copying it into the isolated guest. In an elevated command prompt,
run `certutil -addstore -f Root C:\DigiCertGlobalRootG2.crt`.
[DigiCert's root catalogue](https://knowledge.digicert.com/general-information/digicert-trusted-root-authority-certificates)
identifies this root. Certificate validation remains enabled; this guest
trust-store prerequisite is separate from Firecracker support. A changed
server certificate chain can require a different trusted root.
The [egress regression tools](../tools/windows/egress/README.md) preserve the
TAP probes, bounded snapshot controller and offline result checks.
The checks pass after cold boot, a full format-20 snapshot restored in a new
Firecracker process, and a differential snapshot rebased onto the full memory
file and restored in another process. Each stage opens fresh connections;
this does not test preserving live TCP sessions. The host independently
records each complete echo transfer. Evidence is retained under
`build/windows/win7-reset20/run1`.

XP checks a flushed 65,536-byte patterned file, direct HTTP 200 and a 2 MiB
TCP echo after cold boot and both restore types. Read-only inspection checks
the final file after S5. Evidence is in `build/windows/xp-reset20/run2`.
Both legacy guests also pass the real Tokoloshe HTTP CONNECT proxy test
without a guest NIC, after cold boot and both restore types. Each stage
verifies 2 MiB in each direction, policy denial with zero payload, and
flushed storage. Sealed proxy accounting and offline disk inspection confirm
the guest results. The current driver ISO has no compatible package; these
tests use custom builds from historical Viosock source with receive, reset
event and DMA completion fixes, plus XP build support. The packages are
test-signed, not production-signed. Reproducible patches and build instructions
are in [legacy-vsock](../tools/windows/legacy-vsock/README.md); runtime evidence
is in the [validation record](windows-support-plan.md#reopened-xp-and-windows-7-vsock-validation).
These results do not establish that Tokoloshe can replace Toets' QEMU
backend for its legacy Windows jobs.

### Transitional PCI platform

On x86, `"pci_transport": "transitional"` in `machine-config` selects the
32-bit PCI memory aperture. It requires `--enable-pci`; ARM64 rejects this
selection. The allocator persists the selection and uses the same window for
BAR allocation, relocation and release. The ACPI PCI root omits the unused
64-bit aperture. Default configurations retain modern PCI.

The transport supplies legacy PCI IDs, I/O registers, queue page-frame
addresses and INTx, alongside modern capabilities. It preserves those states
across snapshots. The XP test uses the [SeaBIOS PVH handoff](../resources/firmware/seabios/README.md);
selecting this option alone does not supply BIOS
firmware or install guest drivers.
