# SeaBIOS graphical console firmware

Build SeaVGABIOS from SeaBIOS 1.17.0, revision
`b52ca86e094d19b58e2304417787e96b940e39c6`. This is the same source revision as
the [serial PVH firmware](../seabios/README.md). The display option ROM is a
separate build. It uses the Bochs DISPI interface and PCI ID `1234:1111`.
Firmware diagnostics use COM1 (`0x3f8`) at debug level 3; debug I/O-port output
is disabled.

The Firecracker VGA device is an independent Rust implementation. It does not
incorporate QEMU or Bochs device code. SeaVGABIOS supplies the guest font and
BIOS video services. Its source headers specify LGPLv3. Preserve upstream
`COPYING`, `COPYING.LESSER`, source and build configuration with redistributed
firmware.

## Build on AMD64 Linux

Use a separate source/output directory to preserve the serial firmware. The
source requires GCC with 32-bit support, binutils, GNU make and Python 3. From
the Firecracker root:

```sh
mkdir -p build/windows/console
git clone --depth 1 --branch rel-1.17.0 https://git.seabios.org/seabios.git \
  build/windows/console/seavgabios
test "$(git -C build/windows/console/seavgabios rev-parse HEAD)" = \
  b52ca86e094d19b58e2304417787e96b940e39c6
git -C build/windows/console/seavgabios apply \
  "$PWD/resources/firmware/seabios-console/dispi-modes.patch"
cp resources/firmware/seabios-console/vgabios.config \
  build/windows/console/seavgabios/.config
make -C build/windows/console/seavgabios olddefconfig
make -C build/windows/console/seavgabios -j2 PYTHON=python3
```

Use `build/windows/console/seavgabios/out/vgabios.bin` as the PCI expansion ROM.
Continue to use the coreboot SeaBIOS PVH handoff built by the existing
instructions. No VGA source is linked into the Firecracker binary.

SeaBIOS clears `C0000` option-ROM memory before it scans PCI devices. The host
must expose the option ROM through the VGA PCI expansion-ROM BAR; preloading it
at `C0000` does not work. SeaBIOS reads the ROM from that BAR, copies it below 1
MiB and invokes its initialisation entry.

```text
VGA PCI expansion-ROM BAR -> SeaBIOS copies ROM to C0000
                                        |
                                        v
                            SeaVGABIOS initialises VGA
                            and uploads fonts to plane 2
                                        |
                                        v
                            INT 10h text and VBE services
```

## Device model boundaries

The device core has a fixed 16 MiB framebuffer, indexed VGA ports, VGA latches
and write modes, odd/even text addressing, chain-four addressing, planar
rendering, guest-uploaded text fonts, and Bochs DISPI 8/15/16/24/32-bit modes.
The register BAR supplies the documented VGA and DISPI windows. It does not
advertise EDID or QEMU's later endian-control extension. The firmware patch
removes unsupported 4-bit DISPI modes from the VBE mode list. Standard VGA
planar modes remain available through INT 10h.

The display core's serialised state includes registers, palette, latches and
video memory. Restore must call `validate()` before device access. Its host
frame is row-major `0x00RRGGBB`, separate from guest pixel layout. Legacy
aperture accesses cover physical `A0000-BFFFF`; that range must not be mapped as
ordinary KVM RAM. Linear video-memory access through the BAR uses the same
backing bytes as VGA planes.

The implementation does not yet qualify every historical VGA mode. In particular
CGA interleaved modes, programmable character blink, underline, and
pixel-panning behaviour require further coverage. Passing device tests is not
evidence that Windows installation or a firmware boot has passed. See the
[validation record](../../../docs/windows-validation.md) for the guest tests.

## References and build evidence

The register layout follows the
[QEMU standard VGA interface](https://www.qemu.org/docs/master/specs/standard-vga.html)
and the
[Bochs DISPI specification](https://github.com/bochs-emu/VGABIOS/blob/master/vgabios/vbe_display_api.txt).
The compatible UEFI driver is
[EDK II QemuVideoDxe](https://github.com/tianocore/edk2/tree/master/OvmfPkg/QemuVideoDxe).
The [graphical CloudHv profile](../cloudhv-console/README.md) includes that
driver.

The initial native AMD64 build produced a 39,936-byte option ROM, requiring a 64
KiB ROM BAR. Its ROM signature is `55aa`, its checksum is zero, and its PCIR
vendor/device is `1234:1111`. The initial build digest was
`7db37a45944ff051fe3b3f02bff33db5d414dbdc4b6dbc4ba14fb248133a44c7`. The filtered
build digest is
`4290493bd94206f4b886cc3a35716300ca9008f76afa4ddc9f7db2407fa8679d`. The earlier
configuration enabled debug I/O-port output. These historical results do not
qualify the current COM1-only diagnostic configuration. Record new ROM digests
and build provenance, then validate the new firmware. SeaBIOS embeds build
metadata, so a later build can have a different digest. Keep each ROM immutable
while a VM or snapshot refers to it.
