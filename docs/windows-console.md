# Windows console (experimental)

The optional AMD64 console provides a private display/input endpoint without
guest networking or vsock. It adds VGA/VBE graphics, a PS/2 keyboard, a USB HID
pointer and optional read-only ATAPI installation media. This is a device and
installation interface; each Windows edition still requires guest qualification.

The AMD64 device path is:

```text
external console client
             |
     SSH tunnel / HTTPS gateway
             |
     external console bridge
             |
      private Unix socket
             |
       Firecracker event loop
        |       |       |       |
       VGA     PS/2   USB HID  ATAPI -- read-only ISO
        |       |       |       |
        +-------+---+---+-------+
                    |
                 Windows
                    |
                virtio disk
```

## Configure

### AMD64

For SeaBIOS, build
[SeaVGABIOS](../resources/firmware/seabios-console/README.md). For UEFI, build
the
[graphical CloudHv profile](../resources/firmware/cloudhv-console/README.md).
The BIOS path uses the existing SeaBIOS PVH firmware and its VGA option ROM. Add
this object to the startup JSON configuration:

```json
{
  "console": {
    "socket_path": "console.sock",
    "vga_rom_path": "/srv/firmware/vgabios.bin",
    "optical_image_path": "/srv/media/windows.iso"
  }
}
```

This is a configuration fragment. The complete file also needs the boot source,
machine configuration and drives. Select the matching explicit firmware profile
in the boot source: `{"type": "seabios"}` for SeaBIOS, or `cloud-hv` with its
stable system UUID for CloudHv. SeaBIOS does not use that UUID or SMBIOS
handoff. With explicit `cloud-hv` firmware, omit `vga_rom_path`: its GOP driver
uses the display's framebuffer and register BARs directly. This creates no ROM
mapping or expansion-ROM BAR. SeaBIOS and configurations without an explicit
firmware profile require the ROM. The console also selects conventional PC power
and RTC hardware; removing the console does not remove them when a firmware
profile remains selected.

Use `--enable-pci`. The AMD64 console requires at least 64 MiB RAM and ordinary
pages; huge pages cannot represent its legacy VGA memory aperture. There is no
separate console configuration API endpoint. `/vm/config` reports the configured
console.

Use a private working directory for each VM. The Unix socket is mode 0600; its
parent directory must also have appropriate access restrictions. Use absolute
firmware and media paths when the supervisor changes directories. The ISO is
immutable for the lifetime of a VM and its snapshots. Restore verifies its
SHA-256 digest and the VGA ROM contents when a ROM is configured.

Follow the [console bridge instructions](../tools/windows/console/README.md) for
SSH, RFB, browser access, screenshots and keyboard input. The bridge runs
outside the VMM. TCP listeners accept only loopback addresses; a remote
deployment supplies authentication and TLS through its gateway or SSH.

## AMD64 installation and reboot

The optical device is the primary IDE master. It supports read-only PIO ATAPI
transfers, eject and reload of the same medium. It does not provide DMA, audio,
recording or a second optical drive. Both channels start in PCI native mode:
BAR0/BAR1 address the primary command/control registers and BAR2/BAR3 address
the empty secondary channel. Native interrupts use PCI INTA through the shared
GSI16 level interrupt. Each channel can switch to compatibility mode; the
primary then uses ports `0x1f0` and `0x3f6` with IRQ14. The PCI I/O-enable bit
gates all ports in either mode.

BAR size probing, relocation and mode switching follow the
[PCI IDE 1.0 rules](https://www.isdaman.com/alsos/hardware/hdc/pciide.pdf).
Snapshots retain the selected modes, actual port mappings, PCI registers,
pending interrupt and in-flight transfer. Restore validates the allocator
reservations and media digest before attaching the controller. The image is one
data track in one complete session; TOC formats 0 and 1 report that layout,
including the last-session query used by Windows optical storage. The session
response follows
[MMC-1 section 5.1.13.3](https://13thmonkey.org/documentation/SCSI/mmc1r09.pdf).

Installation media must contain the correct virtio storage driver for the target
Windows version. The preparation tools preserve the source media and require a
new output directory. Follow the
[build prerequisites](../tools/windows/README.md#build-prerequisites), then run
from the repository root:

```sh
mkdir -p build/windows/console
rustc --edition 2024 tools/windows/prepare-xp.rs \
  -o build/windows/console/prepare-xp
rustc --edition 2024 tools/windows/prepare-nt6.rs \
  -o build/windows/console/prepare-nt6
```

The XP tool adds the pinned legacy viostor text-mode driver. It needs `tar`,
`xorriso` and `sha256sum`:

```sh
build/windows/console/prepare-xp windows-xp-sp3.iso virtio-win-1.9.61.tar.xz \
  build/windows/console/xp-media
```

It preserves the original boot image and ISO filename conventions. It also
checks that the `I386` directory fits below VGA memory when XP's boot loader
loads it. Rock Ridge metadata is disabled: it can enlarge that directory beyond
the loader's buffer. Before publishing the ISO, it re-extracts the prepared
text-mode, answer and OEM driver files and compares their bytes. The output
includes `files.sha256-sizes` and `image.sha256`. The tool does not supply a
product key. This media check does not qualify Windows Setup or the installed
driver's signature policy.

The NT6 tool takes an explicit `win7`, `win10`, `win11`, `win11-enterprise-26h2`
or `server2025` profile, a matching pinned AMD64 Windows ISO and
`virtio-win-0.1.302.iso`:

```sh
build/windows/console/prepare-nt6 win7 windows7.iso virtio-win-0.1.302.iso \
  build/windows/console/windows7-media
```

It needs `7z`, `genisoimage` and `sha256sum`. It extracts the full UDF tree,
preserves the original BIOS and EFI boot images, adds the signed storage and
network packages under `$WinPEDriver$`, then re-extracts the result and checks
every file's size and SHA-256 digest. It removes staging files after
verification. Normal errors remove the newly created output directory, including
partial images. Existing output directories are refused. Keep `media.rs` beside
both standalone tool sources; it supplies shared checksum, boot-catalogue and
cleanup helpers.

The accepted source digests are in
[`profile_config`](../tools/windows/prepare-nt6.rs); the tool rejects other
images. Preparing installation media does not establish a successful
installation or Windows hardware-requirement qualification.

For unattended installation, the
[preparation tool](../tools/windows/README.md#prepare-an-unattended-installation)
creates a new raw disk and an answer file. Select `server` for Server Core or
Desktop Experience, or `client` for a Windows client. The server account is
`Administrator`; the client account is `Firecracker`, a local administrator.
Pass the answer file as the optional final argument to the matching
`prepare-nt6` profile; the rebuilt DVD then contains Setup, the answer file and
both storage/network drivers. Keep the generated DVD private because it contains
the account password. Each revised single-DVD unattended path requires a fresh
installation qualification for its selected edition.

Setup searches `$WinPEDriver$` for matching drivers in both graphical and
unattended installations. It loads them into PE and adds them to the installed
image. The answer file does not need driver paths or a configuration set. See
[Microsoft's driver-discovery contract](https://learn.microsoft.com/en-us/troubleshoot/windows-client/setup-upgrade-and-drivers/limitations-dollar-sign-winpedriver-dollar-sign).
The profile selects the corresponding AMD64 viostor and NetKVM packages (`w7`,
`w10`, `w11` or `2k25`). The tool rejects different source digests.

The Server 2025 evaluation profile uses the Microsoft image with SHA-256
`7b052573ba7894c9924e3e87ba732ccd354d18cb75a883efa9b900ea125bfd51`. Its source
is the
[Microsoft Evaluation Center](https://www.microsoft.com/en-us/evalcenter/download-windows-server-2025).
Select a Desktop Experience edition in Setup when validating the installed
graphical desktop. Server Core is a separate guest profile.

A prepared ISO is not proof of successful driver binding. If Setup has no disk,
preserve the PE PnP device IDs, driver inventory and Panther logs before a
retry. Verify the signed driver in PE and in the installed image, then verify
disk boot. The earlier manual driver-selection results do not qualify automatic
installation. Do not modify the signed INF or catalogue.

`drvload` loads a driver into PE only. A historical `drvload`-only test
installed Windows without viostor and stopped with `INACCESSIBLE_BOOT_DEVICE`.
For recovery of an existing disposable image, use
[Microsoft's offline driver-servicing procedure](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/add-and-remove-drivers-to-an-offline-windows-image).
A manual recovery does not qualify the automatic installation path.

Guest reset exits Firecracker. The supervisor starts a new process with the same
writable disk to continue installation. A remote console connection does not own
the VM lifetime; closing it must not stop the VM. Reconnect to the replacement
process after reboot. Eject before booting the installed disk when firmware
would otherwise select installation media again.

## State and platform limits

AMD64 console snapshots contain display memory and registers, keyboard/mouse
queues, held input, pending releases, keyboard LEDs and pending lock updates,
USB controller/HID state, PCI state, optical transfer state and media identity.
The current snapshot format is 25.0.0. Display state adds about 16 MiB; the
bounded snapshot reader accepts at most 32 MiB. The guest disk remains a
separate artifact, as with ordinary Firecracker snapshots.

The console reserves the VGA aperture at `0xa0000..0xbffff` and platform table
memory at 32..33 MiB. The IDE interrupt uses PCI GSI16 in native mode and IRQ14
in compatibility mode. USB shares PCI GSI16. The PS/2 keyboard uses IRQ1; no
auxiliary mouse or IRQ12 is exposed. The optional ISA discovery node and IDE
function use experimental PCI device IDs `1d0f:fc02` and `1d0f:fc01`; the USB
controller uses `1d0f:fc03`. These are fork-local identifiers, not claims of an
assigned hardware product ID.

The PCI root advertises the legacy VGA memory aperture as a read/write producer
when the console is attached. Windows 7's VGA fallback driver needs this range
in addition to the framebuffer BAR: without it, WinPE can run Setup and accept
keyboard input while continuing to show the boot logo. Headless VMs retain their
existing resource windows.

The VGA device exposes `1234:1111` for existing firmware and basic display
drivers. Its 16 MiB linear framebuffer is page-aligned host memory mapped into a
separate KVM memory slot. Guest framebuffer reads and writes execute directly,
including MMX instructions used by XP's basic display driver. The legacy VGA
aperture remains emulated for planar addressing and VGA register behaviour. Host
rendering accesses shared video memory through volatile reads. This provides
software rendering, not GPU acceleration. Sequencer and graphics-controller
registers retain only defined bits, including on readback of restored state;
their layouts follow the
[VGA sequencer](https://www.scs.stanford.edu/10wi-cs140/pintos/specs/freevga/vga/seqreg.htm)
and
[graphics registers](https://www.scs.stanford.edu/10wi-cs140/pintos/specs/freevga/vga/graphreg.htm).

PCI memory decoding enables or removes the framebuffer slot; BAR relocation
moves it. KVM synchronises its memory-slot readers and invalidates old guest
mappings during moves and removal; see
[`kvm_invalidate_memslot` in Linux v6.18](https://github.com/torvalds/linux/blob/v6.18/virt/kvm/kvm_main.c).
The device retains its backing until the slot is removed. If removal fails, the
VM retains that backing until its descriptors close. Both full and differential
snapshots copy all VRAM into device state while the VM is paused; VRAM does not
depend on ordinary guest RAM's dirty-page tracking.

Input uses a US keyboard layout. A USB UHCI controller exposes an absolute HID
mouse with three buttons and vertical scrolling through Windows built-in
drivers. The PS/2 controller supplies only the keyboard. The bridge sends
pointer input after USB enumeration and ignores it while USB is not ready. UEFI
firmware supports the same absolute tablet; the SeaBIOS text interface uses the
keyboard. USB state, keyboard lock state and pending releases are included in
snapshots. This requires no guest networking, vsock channel or separate pointer
driver.

There is one keyboard and one pointer. The auxiliary PS/2 port is absent, IRQ12
is not attached, and there is no USB keyboard. The second UHCI root port is
disconnected. Removing `optical_image_path` removes the IDE controller; it does
not leave an empty CD-ROM attached. Disk, network and vsock devices remain
separate explicit configuration choices.

The selected PC platform exposes serial, RTC, i8042, VM Generation ID, GED and
VMClock. XP and Windows 7 do not bind drivers to all these ACPI features. XP
also retains a VGA device warning while its basic display works. These
limitations are separate from the keyboard and pointer drivers; do not report a
completely clean device inventory for those guests.

The private FCC1 protocol carries physical keyboard scan codes and absolute
pointer coordinates. A console client must preserve key releases and follow the
server's bounded input acknowledgements. Closing a connection does not stop the
VM.

Buffered input retains the scan-code format selected when injected, as in
[QEMU](https://github.com/qemu/qemu/blob/master/hw/input/ps2.c). The controller
does not model serial-clock timing. Clipboard sharing and audio are not
implemented. Port `0x92` and the i8042 output port record A20 requests, but KVM
does not provide the CPU address wrapping used by an A20-disabled software
emulator.
