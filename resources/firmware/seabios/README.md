# SeaBIOS PVH handoff

This x86 firmware boots BIOS disks through Firecracker's PVH entry. The Rust
handoff supplies SeaBIOS with coreboot memory records and the Firecracker
ACPI root. It reserves its own 1-2 MiB range and the legacy BIOS area, loads
the embedded SeaBIOS ELF segments below 1 MiB, then enters SeaBIOS.

Use `pci_transport: "transitional"` and `--enable-pci`. This path was tested
with Windows XP SP3 x86, one vCPU and 2 GiB. It does not provide VGA or an
interactive display. See the [guest results](../../../docs/windows.md#legacy-windows-validation).

The source is SeaBIOS 1.17.0, commit
`b52ca86e094d19b58e2304417787e96b940e39c6`. The patch writes ACKNOWLEDGE before
DRIVER during virtio initialisation. The configuration builds SeaBIOS as a
coreboot payload with serial output. Keep the configuration and patch with
the firmware; an unpatched upstream build is not the tested platform.

## Build on AMD64 Linux

Install GCC with 32-bit compilation support, binutils, GNU make, Python 3,
Rust 1.98.1 and its `i686-unknown-linux-gnu` target. From the Firecracker root,
use a new output directory:

```sh
mkdir -p build/windows
mkdir build/windows/seabios-pvh
git clone --depth 1 --branch rel-1.17.0 https://git.seabios.org/seabios.git \
  build/windows/seabios-pvh/source
test "$(git -C build/windows/seabios-pvh/source rev-parse HEAD)" = \
  b52ca86e094d19b58e2304417787e96b940e39c6
git -C build/windows/seabios-pvh/source apply \
  "$PWD/resources/firmware/seabios/seabios-ordered-status.patch"
cp resources/firmware/seabios/seabios.config build/windows/seabios-pvh/source/.config
mise exec -- make -C build/windows/seabios-pvh/source -j4 PYTHON=python3
cp resources/firmware/seabios/main.rs resources/firmware/seabios/link.ld \
  build/windows/seabios-pvh/
cp build/windows/seabios-pvh/source/out/bios.bin.elf build/windows/seabios-pvh/bios.elf
mise exec -- "$HOME/.cargo/bin/rustup" run 1.98.1 rustc --edition 2024 --target i686-unknown-linux-gnu \
  -D warnings -O -C panic=abort -C relocation-model=static -C linker=gcc \
  -C link-arg=-nostdlib -C link-arg=-static \
  -C link-arg=-Wl,-T,build/windows/seabios-pvh/link.ld \
  build/windows/seabios-pvh/main.rs -o build/windows/seabios-pvh/firmware32.elf
mise exec -- objcopy -O elf64-x86-64 build/windows/seabios-pvh/firmware32.elf \
  build/windows/seabios-pvh/firmware.elf
sha256sum build/windows/seabios-pvh/firmware.elf
```

Set `boot-source.kernel_image_path` to the resulting `firmware.elf`. The
ELF64 container carries a 32-bit Xen PVH entry; the wrapper does not start in
long mode. The linker rejects an image that overlaps the ACPI handoff.

SeaBIOS embeds build metadata, so digests can differ between builds. The
original XP validation firmware has SHA-256
`c5b4bb8a04bb2835eec685e80063f0e7dd55f736de432fbc4430b9518c73c959`.
The preserved Rust source, linker script and SeaBIOS configuration match
that test's inputs. A fresh build from these instructions produces
`2c97c0e13ec572dd641a037c695c0b1bd9611ab6cd4601f7fa21187707fe4512`.
XP passes cold boot, storage, HTTP/TCP egress, full and differential restore,
and S5 with that build. Evidence is under `build/windows/seabios-pvh/run2`.

SeaBIOS carries its upstream licences in `COPYING`, `COPYING.LESSER` and
source headers. The combined firmware embeds SeaBIOS; retain those licences
and its corresponding source when distributing it.
