# SeaBIOS PVH handoff

This x86 firmware boots BIOS disks through Firecracker's PVH entry. The Rust
handoff supplies SeaBIOS with coreboot memory records and the Firecracker ACPI
root. It reserves its own 1-2 MiB range and the legacy BIOS area, loads the
embedded SeaBIOS ELF segments below 1 MiB, then enters SeaBIOS.

Use `pci_transport: "transitional"` and `--enable-pci`. This path was tested
with Windows XP SP3 x86, one vCPU and 2 GiB. The handoff alone provides serial
output. Add the [installation console](../../../docs/windows-console.md) and
SeaVGABIOS for graphics and input. See the
[guest results](../../../docs/windows.md#legacy-windows).

The source is SeaBIOS 1.17.0, commit `b52ca86e094d19b58e2304417787e96b940e39c6`.
The ordered-status patch writes ACKNOWLEDGE before DRIVER during virtio
initialisation. The disk-first patch boots an installed hard disk before optical
media. A blank disk falls back to the installation CD. This lets Windows boot
from its new disk after Setup restarts, with the CD still attached. F8 then
reaches the Windows boot menu instead of restarting the CD installer. The
configuration builds SeaBIOS as a coreboot payload. Firmware diagnostics use
COM1 (`0x3f8`) at debug level 3; debug I/O-port output is disabled. Keep the
configuration and patches with the firmware; an unpatched upstream build is not
the tested platform.

## Build on AMD64 Linux

Install GCC with 32-bit compilation support, binutils, GNU make, Python 3, Rust
1.98.1 and its `i686-unknown-linux-gnu` target. Follow the
[Rust build prerequisites](../../../tools/windows/README.md#build-prerequisites).
From the Firecracker root, use a new output directory:

```sh
mkdir -p build/windows
mkdir build/windows/seabios-pvh
git clone --depth 1 --branch rel-1.17.0 https://git.seabios.org/seabios.git \
  build/windows/seabios-pvh/source
test "$(git -C build/windows/seabios-pvh/source rev-parse HEAD)" = \
  b52ca86e094d19b58e2304417787e96b940e39c6
git -C build/windows/seabios-pvh/source apply \
  "$PWD/resources/firmware/seabios/seabios-ordered-status.patch"
git -C build/windows/seabios-pvh/source apply \
  "$PWD/resources/firmware/seabios/seabios-disk-first.patch"
cp resources/firmware/seabios/seabios.config build/windows/seabios-pvh/source/.config
make -C build/windows/seabios-pvh/source -j4 PYTHON=python3
cp resources/firmware/seabios/main.rs resources/firmware/seabios/link.ld \
  build/windows/seabios-pvh/
cp build/windows/seabios-pvh/source/out/bios.bin.elf build/windows/seabios-pvh/bios.elf
rustc --edition 2024 --target i686-unknown-linux-gnu \
  -D warnings -O -C panic=abort -C relocation-model=static -C linker=gcc \
  -C link-arg=-nostdlib -C link-arg=-static \
  -C link-arg=-Wl,-T,build/windows/seabios-pvh/link.ld \
  build/windows/seabios-pvh/main.rs -o build/windows/seabios-pvh/firmware32.elf
objcopy -O elf64-x86-64 build/windows/seabios-pvh/firmware32.elf \
  build/windows/seabios-pvh/firmware.elf
sha256sum build/windows/seabios-pvh/firmware.elf
```

Set `boot-source.kernel_image_path` to the resulting `firmware.elf` and select
`"firmware": {"type": "seabios"}` in the boot-source object, including for
headless guests. Omit `system_uuid` or set it to `null`: SeaBIOS has no CloudHv
SMBIOS identity handoff. This profile selects the conventional PC power/timer
and CMOS/RTC devices that SeaBIOS requires; a filename alone does not select
them. The ELF64 container carries a 32-bit Xen PVH entry; the wrapper does not
start in long mode. The linker rejects an image that overlaps the ACPI handoff.

SeaBIOS embeds build metadata, so digests can differ between builds. The
original XP validation firmware has SHA-256
`c5b4bb8a04bb2835eec685e80063f0e7dd55f736de432fbc4430b9518c73c959`. The retained
historical Rust source, linker script and SeaBIOS configuration match that
test's inputs. An earlier build with debug I/O-port output enabled produced
`2c97c0e13ec572dd641a037c695c0b1bd9611ab6cd4601f7fa21187707fe4512`. XP passed
cold boot, storage, HTTP/TCP egress, full and differential restore, and S5 with
that build. Evidence is under `build/windows/seabios-pvh/run2`. These historical
results do not qualify the current COM1-only diagnostic configuration. Record
new firmware digests and build provenance, then validate the new firmware.

A disk-first build from the pinned source, both patches and this configuration
has SHA-256 `540f66160112e7db5d031df1fcb2f83f536fe3a5e111ba1afc1feefae992d7fb`.
It booted an installed XP SP3 disk to its desktop with a bootable Windows 7 DVD
still attached. This checks disk priority; it does not qualify a new Windows 7
installation.

SeaBIOS carries its upstream licences in `COPYING`, `COPYING.LESSER` and source
headers. The combined firmware embeds SeaBIOS; retain those licences and its
corresponding source when distributing it.
