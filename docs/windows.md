# Windows guests (AMD64, experimental)

This branch runs AMD64 Windows Server 2025 Core, Windows 10 and Windows 11 on
Linux/KVM with virtio PCI storage, networking and full/differential snapshots.
XP x86 and Windows 7 x64 also pass the legacy checks below. Support is
experimental. It does not add a Windows host backend. See the
[ARM64 guide](windows-arm64.md) for the separate firmware path and its
validation status.

## Boot configuration

All CloudHv firmware profiles require the
[PVH memory-map correction](../resources/firmware/pvh-memory-map.md). Earlier
firmware could expose platform device pages as usable RAM and corrupt Windows
page tables on restore. Cold boot corrected firmware before creating new
snapshots; loading an old snapshot does not update its guest memory map.

The intended boot path is EDK II CloudHv PVH firmware, a GPT disk with an EFI
system partition, and modern virtio PCI storage and networking. Start
Firecracker with `--enable-pci`. Use the firmware ELF as the boot source; do not
pass the Windows installation ISO as a kernel or root disk.

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

Replace the example UUID with a unique UUID for the VM and retain it across cold
boots and restores. The explicit `cloud-hv` profile publishes a bounded SMBIOS
Type 1 system identity through the CloudHv firmware ABI. It reserves one page at
`0xF0000` and rejects overlapping ELF load segments. This is a minimal identity
table, not a complete SMBIOS hardware inventory. The profile requires AMD64 PVH
firmware and no initrd. Omit it for SeaBIOS and Linux kernels; the VMM does not
infer the firmware ABI from a filename or PVH note. The UUID is separate from VM
Generation ID and does not change on snapshot restore.

The terminal is the guest serial console. Once SAC reports that CMD is
available, enter `cmd`, wait for the new channel, then enter `ch -si 1`. Press
Enter and follow the login prompts individually.

`hyperv` defaults to `false`. PUT `/machine-config` resets an omitted `hyperv`
field to `false`; PATCH preserves an omitted field. The field is available only
on x86-64. It installs a fixed set of KVM Hyper-V enlightenments after applying
the CPU template. Unsupported host capabilities produce an error before the
guest starts.

The firmware receives the ACPI root through the PVH start information. The
platform supplies a 32-bit ACPI PM timer at `0x608`, PM1 event and control
blocks at `0x602` and `0x606`, and the CloudHv reset/S5 port at `0x600`. SCI
uses interrupt 3, with a level/low MADT override. Reset and S5 terminate the
Firecracker process, like the existing i8042 reset operation. An external
supervisor must start a new process for a guest reboot.

## Prepare the disk

1. Download the AMD64 ISO from the
   [Microsoft Windows Server 2025 Evaluation Center](https://www.microsoft.com/en-us/evalcenter/download-windows-server-2025).

1. Obtain the
   [virtio Windows drivers](https://fedorapeople.org/groups/virt/virtio-win/direct-downloads/stable-virtio/)
   and build the
   [Firecracker CloudHv firmware](../resources/firmware/README.md). Record
   resolved versions and SHA-256 digests with the test results.

1. Install Server Core under QEMU with OVMF, a raw disk, and a modern
   `virtio-blk-pci` device (`disable-legacy=on`). Load the AMD64 `viostor`
   driver during installation. Install `NetKVM` for `virtio-net-pci`.

1. Provide the removable-media EFI boot path on the system partition. QEMU's
   persistent UEFI boot variables are not transferred with the disk. Run
   `bcdboot` before configuring EMS: it replaces existing boot settings.

1. Enable the Windows Special Administration Console from an elevated command
   prompt:

   ```cmd
   bcdedit /emssettings emsport:1 emsbaudrate:115200
   bcdedit /ems {default} on
   bcdedit /bootems {bootmgr} on
   ```

1. Shut down QEMU before giving Firecracker writable access to the disk.

To install directly in Firecracker, use the
[optional installation console](windows-console.md), which supplies VGA, PS/2
keyboard, USB HID pointer and ATAPI hardware. That guide records the tested
editions and limits. An
[unattended preparation tool](../tools/windows/README.md) generates the answer
media and a separate raw disk without starting the installer. See the
[Cloud Hypervisor Windows guide](https://github.com/cloud-hypervisor/cloud-hypervisor/blob/main/docs/windows.md)
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
address for the host network. Firecracker does not provide DHCP or NAT. In SAC,
`i` lists interfaces and `i -?` describes address configuration.

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
`VirtioSocketWSP` service and both Winsock provider DLLs. Check the service with
`sc query VirtioSocketWSP`. The network interface is not required.

The host uses Firecracker's [Unix socket protocol](vsock.md): connect to
`windows.vsock` and send `CONNECT <guest-port>\n`, or listen on
`windows.vsock_<host-port>` for a guest connection to CID 2.

Windows programs must use the installed provider's address family, obtained
through `ViosockGetAF`, and its `SOCKADDR_VM` layout. Do not assume Linux's
numeric `AF_VSOCK` value. The upstream
[socket header](https://github.com/virtio-win/kvm-guest-drivers-windows/blob/master/viosock/inc/vio_sockets.h)
and `viosocklib-test.exe` describe the interface.

Existing vsock connections do not survive snapshot restore. The guest agent must
handle transport reset, reconnect and complete a new readiness handshake.

The device accepts both event-queue notification and the required CID reread as
reset acknowledgement. This supports Windows, which replenishes its event
buffers without notifying that queue. The device supports indirect buffers on
the receive, transmit and event queues.

### Tokoloshe egress

Tokoloshe's socket-grant proxy accepts HTTP CONNECT on host CID 2, port 4750. A
Windows client opens a provider socket to that endpoint, sends
`CONNECT <host>:<port> HTTP/1.1` with a matching `Host` header and an empty
line, then waits for status 200 before sending tunnel data. Bind the granted
host Unix socket at `windows.vsock_4750` in the Firecracker namespace.

The [validation matrix](windows-validation.md) records native Firecracker tests
with the actual `tokoloshe-egress` sidecar, allowed transfers, policy denials,
independent byte accounting and offline storage verification after cold boot and
both restore types. The tested Windows clients use the native Winsock provider.
Tokoloshe's Windows job runtime is outside this scope. HTTPS certificate
validation and proxy configuration remain application responsibilities.

## Conventional network egress

The TAP tests use NetKVM, a separate host network namespace, a veth route and
source NAT on the host uplink. They verify static IPv4 configuration, name
resolution and a byte-checked 2 MiB TCP echo transfer. Modern guest probes also
verify HTTPS certificates with proxies disabled; XP uses HTTP. The results do
not establish DHCP or IPv6 support. See the
[egress tools](../tools/windows/egress/README.md) for setup and offline checks,
and the separate [ARM64 validation](windows-arm64.md) for that architecture.

## Snapshots

This branch uses snapshot format 28.0.0. It saves the selected PCI transport,
firmware profile and system UUID, CET/Hyper-V processor state, ACPI timer/power
registers, CMOS/RTC state and optional console, TPM and authenticated-variable
checkpoints. Restore rejects incompatible formats; use the matching earlier VMM
for older snapshots. The optical media path is part of console configuration and
is not duplicated in the optical device checkpoint. See
[the validation record](windows-validation.md) and
[the secure platform](windows-secure-boot.md).

The RTC follows host UTC plus the saved guest adjustment, including elapsed time
while the VM is stopped. It provides periodic, alarm and update interrupts on
reserved IRQ 8. Register C reads acknowledge pending flags.

The VMClock device publishes generation and disruption counters. Its
`clock_status` remains `UNKNOWN` and its `counter_id` remains `INVALID`; this
implementation does not advertise a usable precision clock. AWS provides a
[null driver package](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/other-windows-device-drivers.html)
that lets Windows recognise the device without adding clock functionality. AWS
documents support for Windows Server 2016, 2019, 2022 and 2025. Windows 10
recognised `AMZNC10C` as `AWS VMClock Device` with device problem code 0 after
installation of the unmodified signed INF/catalog, without reboot. This verifies
device recognition, not counter consumption. No functional Windows VMClock
driver has been verified. Installing the null package does not resolve guest
time lag; clock accuracy requires separate validation.

The guest CPUID records whether Hyper-V is enabled. Restore derives the machine
configuration from that state and checks destination-host support. The vCPU
snapshot includes Hyper-V control MSRs, SynIC state, and synthetic timer
configuration and counts. SynIC2 avoids clearing restored guest message and
event pages. The reference clock is restored through the VM clock state, rather
than writing its read-only counter MSR.

The x86 FADT publishes conventional ACPI fixed hardware and an aligned FACS. The
PM timer resumes from its saved count and signals SCI when an enabled
timer-status event is pending. ARM64 retains hardware-reduced ACPI; see its
separate [validation results](windows-arm64.md#egress-and-validation).

Snapshots do not include the disk. Preserve a matching disk when retaining or
copying a memory snapshot. Do not restore old memory against a disk that has
since changed. Only one VM may write the disk at a time.

## Guest clock policy

Firecracker's virtual CMOS clock uses UTC. Prepare modern Windows images with a
matching RTC policy: use the UTC timezone (`tzutil /s "UTC"`), or retain the
chosen display timezone and set the DWORD `RealTimeIsUniversal` to `1` under
`HKLM\SYSTEM\CurrentControlSet\Control\TimeZoneInformation`, then reboot.
Microsoft documents
[tzutil](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/tzutil);
AWS documents the
[UTC RTC policy for Windows guests](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/change-time-zone-of-instance.html).
These are image configuration choices; Firecracker does not change the guest's
timezone or registry.

Windows images with a local RTC policy can report a timezone offset after cold
boot. Network time synchronisation can later change that clock. Use host
timestamps to identify validation phases independently of guest wall time.

The Windows 10 clock check distinguishes pause from restore: UTC and the
performance counter advance during pause. With `clock_realtime: false`, restore
omits the stopped interval; with `clock_realtime: true`, it compensates that
interval. These short measurements do not establish long-term accuracy. No VMBus
time synchronisation service is provided. Cross-host migration and
firmware-stage snapshots remain unvalidated.

## Legacy Windows

Use the [validation matrix](windows-validation.md) for exact tested builds. XP
SP3 x86 uses SeaBIOS, transitional PCI and one vCPU with its uniprocessor HAL.
Its checks do not establish SMP or HTTPS support. Windows 7 SP1 x64 uses a
GPT/EFI disk with the corrected CloudHv firmware. Both require drivers matched
to their OS version. Contemporary virtio-win media has no compatible Viosock
package; the tested historical drivers require the documented receive, reset and
DMA fixes. These packages are test-signed. Build and installation instructions
are in [legacy-vsock](../tools/windows/legacy-vsock/README.md).

Windows 7's TLS test requires a current trusted root. The tested
`www.microsoft.com` chain terminates at DigiCert Global Root G2. Download the
[official DER certificate](https://cacerts.digicert.com/DigiCertGlobalRootG2.crt)
and require SHA-256
`cb3ccbb76031e5e0138f8dd39a23f9de47ffc35e43c1144cea27d46a5ab1cb5f` before
copying it into the isolated guest. In an elevated command prompt, run
`certutil -addstore -f Root C:\DigiCertGlobalRootG2.crt`.
[DigiCert's root catalogue](https://knowledge.digicert.com/general-information/digicert-trusted-root-authority-certificates)
identifies this root. Certificate validation remains enabled. A changed server
chain can require a different trusted root; this guest trust-store prerequisite
is separate from Firecracker support.

The [egress regression tools](../tools/windows/egress/README.md) open fresh
connections after each restore; they do not test preservation of live TCP
sessions. XP also verifies a flushed 65,536-byte patterned file. CONNECT runs
omit the guest NIC and check allowed byte transfers, zero-payload denials and
flushed storage. Read-only disk checks and host accounting verify the guest
reports. These platform results do not establish that Tokoloshe can replace
Toets' QEMU backend for Windows jobs.

### Transitional PCI platform

On x86, `"pci_transport": "transitional"` in `machine-config` selects the 32-bit
PCI memory aperture. It requires `--enable-pci`; ARM64 rejects this selection.
The allocator persists the selection and uses the same window for BAR
allocation, relocation and release. The ACPI PCI root omits the unused 64-bit
aperture. Default configurations retain modern PCI.

The transport supplies legacy PCI IDs, I/O registers, queue page-frame addresses
and INTx, alongside modern capabilities. It preserves those states across
snapshots. The XP test uses the
[SeaBIOS PVH handoff](../resources/firmware/seabios/README.md); selecting this
option alone does not supply BIOS firmware or install guest drivers.
