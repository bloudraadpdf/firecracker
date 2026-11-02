# Graphical CloudHv firmware

This opt-in AMD64 profile adds the existing EDK II Bochs/QEMU graphics output
driver, PS/2 keyboard and USB absolute-pointer drivers to CloudHv firmware. It
uses the same pinned EDK II revision as the [headless firmware](../README.md):
`97eeb7b0901da9f7b5d98a2e6a5e8694978298e9` (`ch-97eeb7b09`).

Apply the existing `cloudhv-firecracker.patch` first. Then apply
`cloudhv-console.patch` from this directory. Build the new
`OvmfPkg/CloudHv/CloudHvConsoleX64.dsc` profile. It includes the original DSC
with a console flag; the existing headless profile keeps its original component
selection and output directory.

```text
PCI VGA 1234:1111 -> QemuVideoDxe -> Graphics Output Protocol
                                             |
                                      GraphicsConsoleDxe
                                             |
                                          ConSplitter

PCI ISA bridge -> SioBusDxe -> PS/2 keyboard -> text input

PCI UHCI -> UhciDxe -> UsbBusDxe -> HID tablet -> absolute pointer
```

For explicit `cloud-hv` firmware, omit `console.vga_rom_path`. QemuVideoDxe uses
BAR0 for the framebuffer. Standard VGA and virtio VGA use BAR2 for the Bochs
registers. QXL uses Bochs I/O ports and reads the drawable framebuffer size from
its QXL ROM data in BAR2. This data BAR is part of the QXL device; it is
separate from the executable VGA option ROM.

The GOP driver does not execute SeaVGABIOS. The display remains attached without
a PCI expansion-ROM resource. SeaBIOS requires its separate VGA option ROM. QXL
guest validation currently uses SeaBIOS; the firmware's QXL driver match alone
does not establish a tested CloudHv/QXL boot.

CloudHv already contains GraphicsConsoleDxe, ConSplitter and the ATA/ATAPI
storage stack. The patch adds QemuVideoDxe, SioBusDxe and Ps2KeyboardDxe. The
SIO keyboard child is `PNP0303`; no PS/2 mouse is exposed.

The USB path adds UhciDxe, UsbBusDxe and UsbMouseAbsolutePointerDxe. Upstream's
absolute-pointer driver accepts only relative boot-mouse reports. The console
patch also accepts the fork's `1d0f:fc03` nonboot tablet, checks its complete
HID report descriptor, and reads its six-byte absolute reports directly. Its
axes are `0..32767`; EFI ConSplitter normalises them to `0..65536`. The tablet
has left/right buttons and no pressure axis. The USB HID wheel remains available
to Windows; EFI absolute-pointer input does not expose a scroll wheel. The
[firmware probe](tests/README.md) checks coordinates and buttons before an OS
starts.

The VM must expose its legacy devices through an enabled PCI ISA bridge (class
`0601`, I/O and memory decode enabled). Upstream SioBusDxe matches that class
without requiring a PIIX device ID. The platform boot manager then adds the
keyboard to `ConIn` and the VGA device to `ConOut`.

The console profile exposes PCI I/O ports `0x1000..0xffff` to the firmware
allocator (`PcdPciIoBase=0x1000`, `PcdPciIoSize=0xf000`). This matches
Firecracker's PCI I/O allocator and ACPI root aperture, leaving legacy ports and
PCI configuration ports outside the allocation window. EDK II's root bridge I/O
validation separately permits legacy VGA, ISA and IDE accesses below `0x1000`
through the corresponding root attributes. Firecracker assigns BAR addresses
before boot; the shared CloudHv patch marks those resources as already assigned.
The headless profile retains its zero-size I/O default. The console compile flag
also selects this aperture in `PlatformInitLib`, which creates the resource HOB
used by `PlatformPei`. Setting DSC defaults alone is insufficient: the upstream
platform initialiser replaces them with `0xc000..0xffff`, which rejects native
IDE BARs assigned near `0x1000`. The headless platform initialiser remains
unchanged.

## Build

Use a separate checkout to preserve the headless firmware and its build
products. From the Firecracker root, with the EDK II build prerequisites
(including NASM and IASL on `PATH`, or `NASM_PREFIX` and `IASL_PREFIX` set):

```sh
git clone --depth 1 --branch ch-97eeb7b09 \
  https://github.com/cloud-hypervisor/edk2.git build/windows/console/edk2
git -C build/windows/console/edk2 submodule update --init --recursive
git -C build/windows/console/edk2 apply --ignore-whitespace \
  "$PWD/resources/firmware/cloudhv-firecracker.patch"
git -C build/windows/console/edk2 apply --ignore-whitespace \
  "$PWD/resources/firmware/cloudhv-console/cloudhv-console.patch"
git -C build/windows/console/edk2 apply --ignore-whitespace \
  "$PWD/resources/firmware/cloudhv-pvh-memory-map.patch"
cd build/windows/console/edk2
. edksetup.sh
make -C BaseTools -j2
build -p OvmfPkg/CloudHv/CloudHvConsoleX64.dsc \
  -a X64 -t GCC -b RELEASE -n 2
```

The output is `Build/CloudHvConsoleX64/RELEASE_GCC/FV/CLOUDHV.fd`. Record its
digest before guest validation. A successful firmware build does not establish a
successful Windows installer boot.

The firmware patch follows the upstream `BSD-2-Clause-Patent` licence. The Rust
VGA device is independent of QEMU's device implementation. UEFI uses
QemuVideoDxe directly; the SeaVGABIOS option ROM is for the BIOS path.

## Build evidence

The AMD64 release profile built with two jobs and produced a 4 MiB image with
SHA-256 `730de0fca7791d7623336b5c87d53e076c941e53f1118cb3b26d91445cbf7a16`. The
corrected image passes the UEFI memory-map probe and the recorded Windows
installation/restore matrix. The USB profile also passes the no-OS
absolute-pointer probe. Keep firmware images immutable while their guests or
snapshots exist.

The console platform initialiser was validated with a DEBUG serial build: the
root bridge reported I/O `0x1000..0xffff`, EDK II identified and booted the
native IDE optical drive, and Windows 11 started the generic IDE controller,
both channels and the CD-ROM driver. WinPE read the prepared ISO's
`Drivers\viostor` directory. Windows Setup displayed at 800 by 600 pixels;
remote input accepted the CD boot prompt, opened Shift+F10, ran commands and
moved the pointer. Completed installation is tracked in the
[validation record](../../../docs/windows-validation.md).

The optical controller's single-session READ TOC format 1 response also passes
an unchanged Windows 11 ISO test: WinPE mounts UDF and reads the full
4,332,322,774-byte installation image. The earlier CDFS fallback was an ATAPI
command omission, independent of firmware and image preparation. No registry
modification or WIM splitting is required.
