# Firmware absolute-pointer probe

`PointerProbe` connects the firmware controllers and reads
`EFI_ABSOLUTE_POINTER_PROTOCOL` for 120 seconds. It writes coordinates, buttons
and axis limits to the firmware console and serial port. No guest OS or USB
mouse driver is required.

The applications and malformed-map fixtures are supplied as a patch to the
external EDK II tree. From the Firecracker repository root, apply it after the
console firmware patches:

```sh
git -C build/windows/console/edk2 apply \
  "$PWD/resources/firmware/cloudhv-console/tests/firmware-tests.patch"
```

Build with that tree's normal toolchain environment:

```sh
cd build/windows/console/edk2
build -p OvmfPkg/CloudHv/Tests/PointerProbe.dsc -a X64 -t GCC -b RELEASE \
  -m OvmfPkg/CloudHv/Tests/PointerProbe.inf
truncate -s 64M pointer-probe.raw
mformat -i pointer-probe.raw -F ::
mmd -i pointer-probe.raw ::/EFI ::/EFI/BOOT
mcopy -i pointer-probe.raw Build/CloudHvConsoleX64/RELEASE_GCC/X64/PointerProbe.efi \
  ::/EFI/BOOT/BOOTX64.EFI
```

Attach that FAT image as a read-only virtio disk. Boot the console firmware with
one vCPU, 256 MiB RAM and the console enabled. Bound the VMM lifetime with a
180-second timeout. Send absolute pointer requests through the console socket.
The console multiplexer scales the tablet's `0..32767` axes to `0..65536`.

The native Firecracker probe produced these values:

| Injected X, Y | EFI X, Y     | Button |
| ------------- | ------------ | ------ |
| 1234, 2345    | 2468, 4690   | None   |
| 15000, 28000  | 30000, 56001 | Left   |
| 32767, 0      | 65536, 0     | None   |
| 0, 32767      | 0, 65536     | Right  |
| 16384, 16384  | 32769, 32769 | None   |

This checks the firmware protocol, not just USB enumeration. Stop the probe VMM
and remove its generated FAT image after collecting the serial log.
