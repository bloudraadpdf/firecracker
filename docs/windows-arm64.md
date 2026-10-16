# Windows guests (ARM64, experimental)

Build with `cargo build --features windows`; firmware execution requires the
optional Windows capability. Default builds reject firmware before loading it.

This headless path loads patched EDK II CloudHv firmware and supplies ACPI for
GICv3/ITS, architectural timers, PCI and the serial port. Use an authorised,
already-installed raw disk for investigation. Direct installation and native
Windows runtime qualification remain required.

## Host requirements

Use an ARM64 Linux host with KVM, GICv3/ITS and PMUv3 support. The Windows
loader reads `PMCR_EL0`. On the tested Apple nested-KVM appliance, that
instruction faults because KVM exposes no PMU. A working Linux guest or EFI
shell on that host does not establish Windows compatibility.

## Firecracker boot configuration

Build the
[headless ARM64 firmware](../resources/firmware/README.md#arm64-firmware). It
loads raw firmware at `0x80200000`, with the FDT at `0x80000000` and the ACPI
root at `0x801f0000`. Do not supply an initrd or an AMD64 firmware profile.

The following configuration limits the 52-bit validation host to 48 bits:

```json
{
  "boot-source": {
    "kernel_image_path": "CLOUDHV_EFI.fd",
    "boot_args": ""
  },
  "machine-config": {"vcpu_count": 2, "mem_size_mib": 4096, "track_dirty_pages": true},
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
the PMU from the saved vCPU feature flags. The AMD64 `hyperv` machine field does
not apply to ARM64.

The register modifier sets `ID_AA64MMFR0_EL1.PARange` to 48 bits. On the 52-bit
validation host, firmware creates five-level translation tables. The tested
Windows loader then selects four levels while retaining that table address and
faults. Exposing 48 bits makes firmware use four levels and allows Windows
kernel execution. On hosts already exposing 48 bits or less, omit this modifier;
it must not increase the host's address width.
