# AMD64 installation console (experimental)

The optional console adds VGA/VBE graphics, PS/2 keyboard input, a USB HID
pointer and read-only ATAPI installation media to Firecracker. It does not
require guest networking or vsock. See the validation matrix below and the
[validation record](windows-validation.md).

```text
remote browser or VNC viewer
             |
     SSH tunnel / HTTPS gateway
             |
     host console bridge (RFB)
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

Build [SeaVGABIOS](../resources/firmware/seabios-console/README.md). For UEFI,
build the
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
machine configuration and drives. Use `--enable-pci`. The console requires
AMD64, at least 64 MiB RAM and ordinary pages; huge pages cannot represent its
legacy VGA memory aperture. There is no separate console configuration API
endpoint. `/vm/config` reports the configured console.

Use a private working directory for each VM. The Unix socket is mode 0600; its
parent directory must also have appropriate access restrictions. Use absolute
firmware and media paths when the supervisor changes directories. The ISO is
immutable for the lifetime of a VM and its snapshots. Restore verifies its
SHA-256 digest and the VGA ROM contents.

Follow the [console bridge instructions](../tools/windows/console/README.md) for
SSH, RFB, browser access, screenshots and keyboard input. The bridge runs
outside the VMM. Its TCP listeners accept only loopback addresses. A remote
deployment supplies authentication and TLS through its gateway or SSH.

## Installation and reboot

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
new output directory:

```sh
mise exec -- rustup run 1.98.1 rustc --edition 2024 tools/windows/prepare-xp.rs \
  -o build/windows/console/prepare-xp
mise exec -- rustup run 1.98.1 rustc --edition 2024 tools/windows/prepare-nt6.rs \
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
the loader's buffer. The tool does not supply a product key.

The NT6 tool takes an explicit `win7`, `win10`, `win11` or `server2025` profile,
a matching pinned AMD64 Windows ISO and `virtio-win-0.1.302.iso`:

```sh
build/windows/console/prepare-nt6 win7 windows7.iso virtio-win-0.1.302.iso \
  build/windows/console/windows7-media
```

It needs `7z`, `genisoimage` and `sha256sum`. It extracts the full UDF tree,
preserves the original BIOS and EFI boot images, adds `Drivers/viostor`, then
re-extracts the result and checks every file's size and SHA-256 digest. It
removes staging files after verification. Normal errors remove the newly created
output directory, including partial images. Existing output directories are
refused. Keep `media.rs` beside both standalone tool sources; it supplies shared
checksum, boot-catalogue and cleanup helpers.

Use **Load driver** in Windows Setup to select that folder. The profile chooses
the corresponding AMD64 viostor driver (`w7`, `w10`, `w11` or `2k25`); the tool
rejects different source digests. The Server 2025 evaluation profile uses the
Microsoft image with SHA-256
`7b052573ba7894c9924e3e87ba732ccd354d18cb75a883efa9b900ea125bfd51`. Its source
is the
[Microsoft Evaluation Center](https://www.microsoft.com/en-us/evalcenter/download-windows-server-2025).
Select a Desktop Experience edition in Setup when validating the installed
graphical desktop; Server Core remains a separate supported guest profile.

If recent Setup versions show an empty driver list, clear **Hide drivers that
aren't compatible with this computer's hardware**. The pinned package lists two
entries with the same display name: select the second entry for modern PCI
device `1af4:1042` (the first is transitional `1af4:1001`). The INF's primary
modern ID names subsystem `1100`; Firecracker uses `1042`. Its compatible-ID
entry matches both. The
[Virtio specification](https://docs.oasis-open.org/virtio/virtio/v1.2/virtio-v1.2.html)
permits this subsystem value and requires matching the modern vendor/device ID.
Do not modify the signed INF or catalogue.

As a PE diagnostic alternative, Shift+F10 and
`pnputil /add-driver D:\Drivers\viostor\viostor.inf /install` installs the
matching signed driver. Return to disk selection and refresh the list. This
proves PE device binding; verify driver persistence in the installed image and a
successful disk boot separately.

`drvload` loads a driver into Windows PE only. It does not replace Setup's
**Load driver** step: Windows 11 Setup installed an image without viostor after
a `drvload`-only test, then stopped with `INACCESSIBLE_BOOT_DEVICE`. To recover
that image, boot the installation media, open Shift+F10, load viostor into PE,
and identify the installed Windows volume with `dir`. If that volume has no
drive letter, use DiskPart's `list volume`, `select volume` and `assign letter`
commands to assign an unused letter. Use
[Microsoft's offline driver servicing procedure](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/add-and-remove-drivers-to-an-offline-windows-image)
to add the signed INF to that volume. For example, after verifying that
`E:\Windows` is the installed image and `D:` is the installation DVD:

```text
drvload D:\Drivers\viostor\viostor.inf
dir E:\Windows
dism /Image:E:\ /Add-Driver /Driver:D:\Drivers\viostor\viostor.inf
dism /Image:E:\ /Get-Drivers
```

Check that DISM reports successful installation and lists viostor before
restarting the installed image. Drive letters can differ between PE boots.

Guest reset exits Firecracker. The supervisor starts a new process with the same
writable disk to continue installation. A remote console connection does not own
the VM lifetime; closing it must not stop the VM. Reconnect to the replacement
process after reboot. Eject before booting the installed disk when firmware
would otherwise select installation media again.

## State and platform limits

Console snapshots contain display memory and registers, keyboard/mouse queues,
held input, pending releases, keyboard LEDs and pending lock updates, USB
controller/HID state, PCI state, optical transfer state and media identity. The
current snapshot format is 28.0.0. Display state adds about 16 MiB; the bounded
snapshot reader accepts at most 32 MiB. The guest disk remains a separate
artifact, as with ordinary Firecracker snapshots.

The Windows 11 test edition is IoT Enterprise LTSC 2024. It passed Setup's
hardware check without registry bypasses. The optional
[secure platform](windows-secure-boot.md) adds TPM and protected firmware
variables. Standard Windows 11 Enterprise Evaluation passes the recorded secure
matrix; other editions are not inferred from that result.

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
separate explicit configuration choices. TPM and protected firmware variables
are also opt-in.

The existing platform still exposes serial, RTC, i8042, VM Generation ID, GED
and VMClock. XP and Windows 7 do not bind drivers to all these ACPI features. XP
also retains a VGA device warning while its basic display works. These
limitations are separate from the keyboard and pointer drivers; do not report a
completely clean device inventory for those guests.

The bridge uses the guest keyboard LEDs to preserve printable ASCII case and
punctuation with Caps Lock and Num Lock. It waits at most 100 ms for a pending
lock acknowledgement before sending another non-lock key press. Disconnect
releases are retried if the guest input queue is full, including after restore.
Arbitrary guest keyboard layouts and Unicode text injection are not implemented.
See the
[bridge protocol](../tools/windows/console/README.md#behaviour-and-limits) for
the input-state and absolute-pointer requests.

Buffered input retains the scan-code format selected when injected, as in
[QEMU](https://github.com/qemu/qemu/blob/master/hw/input/ps2.c). The controller
does not model serial-clock timing. Clipboard sharing and audio are not
implemented. Port `0x92` and the i8042 output port record A20 requests, but KVM
does not provide the CPU address wrapping used by an A20-disabled software
emulator.

## Validation

The [validation record](windows-validation.md) lists exact tested editions and
installation, input, egress and restore checks. The
[console bridge](../tools/windows/console/README.md) describes native VNC and
browser access, authentication and the bounded input protocol.
