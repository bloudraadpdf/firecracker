# Windows guests (AMD64, experimental)

Build with `cargo build --features windows` for the optional Windows platform.
Default builds retain the API schema and reject Windows-only configurations
before they open boot files or attach devices. Saved state has the same data
layout in both builds; default builds reject saved Windows devices explicitly.

This firmware path requires Linux/KVM. It adds no Windows host backend.

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
table, not a complete SMBIOS hardware inventory. Both firmware profiles require
AMD64 PVH firmware and no initrd. For SeaBIOS, set
`"firmware": {"type": "seabios"}`; omit `system_uuid` or set it to `null`.
SeaBIOS does not receive a CloudHv SMBIOS handoff. Omit the firmware object for
direct Linux kernel boot. The VMM does not infer the firmware ABI from a
filename or PVH note. The CloudHv UUID is separate from VM Generation ID and
does not change on snapshot restore.

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

An explicit firmware profile selects this conventional PC power/timer hardware
and the CMOS/RTC on IRQ 8. A direct Linux kernel retains hardware-reduced ACPI,
without these devices, their event descriptors or the IRQ 8 reservation. PCI and
Hyper-V settings do not select PC timers.

Use a prepared GPT disk with an EFI system partition and matching signed virtio
storage drivers. For an installer display and read-only installation media,
select the optional [AMD64 console](windows-console.md). The headless profile
does not provide them.

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

### Windows 7 TLS trust

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

## Legacy Windows

XP SP3 x86 uses SeaBIOS, transitional PCI and one vCPU with its uniprocessor
HAL. These device choices do not establish SMP or HTTPS support. Windows 7 SP1
x64 uses GPT/EFI with matching signed storage and network drivers. Contemporary
virtio-win media has no compatible Viosock package for these guests. The
[test-driver procedure](../tools/windows/legacy-vsock/README.md) retains the
required receive, reset and DMA fixes and uses test signing. A test-driver build
does not qualify an installed guest. No production-signed package is supplied.

### Tokoloshe egress

Tokoloshe's socket-grant proxy accepts HTTP CONNECT on host CID 2, port 4750. A
Windows client opens a provider socket to that endpoint, sends
`CONNECT <host>:<port> HTTP/1.1` with a matching `Host` header and an empty
line, then waits for status 200 before sending tunnel data. Bind the granted
host Unix socket at `windows.vsock_4750` in the Firecracker namespace.

Use the [egress controllers](../tools/windows/egress/README.md) for separate TAP
and CONNECT cold/full/differential checks. They stage matched state, reject open
disks, verify guest and host byte counts and inspect storage read-only after
clean shutdown. HTTPS validation and proxy configuration remain application
responsibilities. Tokoloshe's Windows job runtime is outside this device scope.
