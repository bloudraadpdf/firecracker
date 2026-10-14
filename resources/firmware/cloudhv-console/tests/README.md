# Firmware absolute-pointer probe

`PointerProbe` connects the firmware controllers and reads
`EFI_ABSOLUTE_POINTER_PROTOCOL` for 120 seconds. It writes coordinates, buttons
and axis limits to the firmware console and serial port. No guest OS or USB
mouse driver is required.

Build with the console EDK II tree and its normal toolchain environment:

```sh
cp PointerProbe.c PointerProbe.inf "$EDK2/MdeModulePkg/Application/HelloWorld/"
cd "$EDK2"
cp OvmfPkg/CloudHv/CloudHvConsoleX64.dsc OvmfPkg/CloudHv/CloudHvPointerProbeX64.dsc
printf '\n[Components]\n  MdeModulePkg/Application/HelloWorld/PointerProbe.inf\n' \
  >> OvmfPkg/CloudHv/CloudHvPointerProbeX64.dsc
build -p OvmfPkg/CloudHv/CloudHvPointerProbeX64.dsc -a X64 -t GCC -b RELEASE \
  -m MdeModulePkg/Application/HelloWorld/PointerProbe.inf
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

| Injected X, Y | EFI X, Y | Button |
|---|---|---|
| 1234, 2345 | 2468, 4690 | None |
| 15000, 28000 | 30000, 56001 | Left |
| 32767, 0 | 65536, 0 | None |
| 0, 32767 | 0, 65536 | Right |
| 16384, 16384 | 32769, 32769 | None |

This checks the firmware protocol, not just USB enumeration. Stop the probe VMM
and remove its generated FAT image after collecting the serial log.
