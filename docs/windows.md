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
storage drivers. This headless profile does not provide an installer display or
installation media.
