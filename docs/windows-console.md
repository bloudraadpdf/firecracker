# Windows console (experimental)

The optional console provides a private display/input endpoint without guest
networking or vsock. The AMD64 profile adds VGA/VBE graphics, a PS/2 keyboard, a
USB HID pointer and optional read-only ATAPI installation media. The ARM64
profile adds an MMIO display and one combined virtio keyboard/absolute pointer.
See the [validation record](windows-validation.md#current-integration-candidate)
for the current XP display/input results and historical AMD64 results. ARM64
guest qualification remains pending.

The AMD64 device path is:

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

Build the VMM with the optional `windows` capability, following the
[AMD64 build instructions](windows.md#build) or
[ARM64 build instructions](windows-arm64.md#build). Default builds retain the
console configuration schema but reject console requests and saved console
devices before opening their host resources.

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
outside the VMM. Its TCP listeners accept only loopback addresses. A remote
deployment supplies authentication and TLS through its gateway or SSH.

### ARM64

Build the
[ARM64 graphical firmware](../resources/firmware/cloudhv-secureboot/arm64/README.md).
Select `ArmVirtCloudHvConsole.dsc`, or `ArmVirtCloudHvSecure.dsc` with the TPM
and protected variable devices. The original `ArmVirtCloudHv.dsc` is headless;
console JSON alone does not select a graphical firmware build.

Add only the private console socket to the ARM64 startup configuration:

```json
{
  "console": {
    "socket_path": "console.sock"
  }
}
```

Use `--enable-pci`, modern PCI, at least 64 MiB RAM and ordinary pages. ARM64
uses 64 MiB as a device-attachment minimum, not a Windows installation minimum.
It rejects `vga_rom_path` and `optical_image_path`. The device profile contains
no VGA option ROM, legacy VGA aperture, PS/2 controller, USB controller, ISA
bridge, A20 port or IDE controller. The display has a framebuffer BAR and a
register BAR; the keyboard and pointer share one virtio PCI function.

```text
private console socket
          |
     ConsoleServer
       |       |
 MMIO display  virtio-input (keyboard + absolute pointer)
       |       |
       +---+---+
           |
      ARM64 firmware / Windows
```

Firmware must provide the matching MMIO GOP and virtio input drivers before
Windows starts. Windows needs the signed ARM64 virtio-input driver. The pinned
source accepts the combined keyboard/pointer capabilities; a source review and
cross-build do not prove signed-driver loading or ARM64 guest operation. See the
[ARM64 prerequisites and validation limits](windows-arm64.md).

A direct ARM64 installation path has not been qualified. It needs prepared
GPT/FAT media on virtio block storage; this profile has no optical device.
Preparation must handle split WIM files and load the matching signed ARM64
storage and input drivers in both WinPE and the installed image. A driver
directory beside `boot.wim` does not load those drivers. The AMD64 ATAPI ISO
procedure below does not apply.

The same external bridge, SSH routing and FCC1 protocol serve both profiles.
FCC1 keys are PS/2 set-2 physical codes; Firecracker translates them to evdev
for ARM64, including Windows keys, keypad keys, PrintScreen and Pause. Pointer
coordinates are 0..32767. Guest LEDs retain the existing PS/2 wire order. Full
input queues return a retry response without accepting half an update. On
disconnect, Input finishes a started report, cancels untouched reports and
queues releases for the resulting pressed keys. It retains lock transitions
already written to the guest and cancels feedback waits for discarded makes.
Releases remain queued until guest buffers return; firmware does not have to
notify the host when it replenishes those buffers. The event queue is required;
the status queue is optional for virtio input. The graphical firmware must send
real LED status for the bridge's lock-key synchronisation.

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

The NT6 tool takes an explicit `win7`, `win10`, `win11`,
`win11-enterprise-26h2`, `win11-pro-26h2` or `server2025` profile, a matching
pinned AMD64 Windows ISO and `virtio-win-0.1.302.iso`:

```sh
build/windows/console/prepare-nt6 win7 windows7.iso virtio-win-0.1.302.iso \
  build/windows/console/windows7-media
```

It needs `7z`, `genisoimage` and `sha256sum`. It extracts the full UDF tree,
verifies the original BIOS and EFI boot extents, adds the signed storage and
network packages under `$WinPEDriver$`, then re-extracts the result and checks
every file's size and SHA-256 digest. It removes staging files after
verification. Normal errors remove the newly created output directory, including
partial images. Existing output directories are refused. Keep `media.rs` beside
both standalone tool sources; it supplies shared checksum, boot-catalogue and
cleanup helpers.

`win11` selects the tested IoT Enterprise LTSC image. For standard Enterprise
Evaluation, use `win11-enterprise-26h2`; for the pinned standard Pro English
AMD64 media and its Professional image at index 6, use `win11-pro-26h2`. Both
profiles select the 2023 boot-file recipe in the
[secure firmware and media procedure](../resources/firmware/cloudhv-secureboot/README.md#current-windows-installation-media).
Pro installation remains unqualified; that procedure retains the source
install.wim warning and boot-chain limits. The accepted source digests are in
[`profile_config`](../tools/windows/prepare-nt6.rs); the tool rejects other
images.

For unattended installation, the
[preparation tool](../tools/windows/README.md#prepare-an-unattended-installation)
creates a new raw disk and an answer file. Select `server` for Server Core or
Desktop Experience, or `client` for a Windows client. The server account is
`Administrator`; the client account is `Firecracker`, a local administrator.
Pass the answer file as the optional final argument to the matching
`prepare-nt6` profile; the rebuilt DVD then contains Setup, the answer file and
both storage/network drivers. Only `win11-pro-26h2` with an answer file uses the
original no-prompt EX EFI CD image; graphical Pro and the other five profiles
retain their previous boot-image selection. Keep the generated DVD private
because it contains the account password. Each revised single-DVD unattended
path requires a fresh installation qualification for its selected edition.

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
The current snapshot format is 34.0.0. Display state adds about 16 MiB; the
bounded snapshot reader accepts at most 32 MiB. The guest disk remains a
separate artifact, as with ordinary Firecracker snapshots.

ARM64 console snapshots save display/configuration and one typed virtio-input
state in the PCI device manager. The console reuses that backend on restore; it
does not save a second held-key or LED state. Pending frames, partial-frame
cursor, position, held releases and guest/pending lock LEDs travel with Input.
Missing, stray or mismatched console/input topology is rejected before backend
attachment. Format29 through format33 snapshots require their matching older
binaries; format34 has no decoder for them. Snapshots do not move between AMD64
and ARM64.

The historical format31 Windows 11 test edition was IoT Enterprise LTSC 2024. It
passed Setup's hardware check without registry bypasses. The optional
[secure platform](windows-secure-boot.md) adds TPM and protected firmware
variables. The recorded Standard Windows 11 Enterprise Evaluation secure matrix
also used format31. See [current validation](windows-validation.md) for the
current source and format; those historical results do not qualify format34 or
other editions.

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

The graphical AMD64 profile exposes serial, RTC, i8042 and VM Generation ID. Set
`"vmclock": false` in `machine-config` to omit VMClock, its reserved memory,
interrupt and ACPI description. VM Generation ID remains present. Generation
notifications use the existing SCI/GPE hardware, without a separate GED device.
Default Linux and ARM hardware retain GED. XP and Windows 7 do not bind drivers
to all these ACPI features. The recorded format33 pre-installation XP baseline
uses VgaSave and retains the unconfigured VGA device at Code 1 and AWS VMClock
(`ACPI\AMZNC10C`) at Code 28; VGEN reports Code 0 with service `gencounter`. A
later isolated VGA driver attempt remained black after cold boot. That failed
attempt is historical. On VMM `93434437`, the unsigned XPDM miniport 0.1.2.0
passes all 16 advertised 32-bit modes with full-frame RGB checks, five pointer
targets per mode and saved printable ASCII with Caps Lock on. The helper
restores the original mode and exits zero before normal S5. This direct-console
check does not qualify Windows Keep/Revert/timeout UI, browser/VNC input,
nonzero-mode cold boot or full/differential display/input restore.

The [XP miniport source and build procedure](../tools/windows/display/README.md)
are included with the test tools. Windows supplies its inbox display DLL.

The modern signed AWS recognition package does not cover the XP x86 and legacy
Windows 7 hardware-ID cases. No legacy VMClock recognition installation is
qualified. Recognition alone would not establish clock use. Do not report a
clean device inventory or broader guest support from the display/input result.

The bridge uses the guest keyboard LEDs to preserve printable ASCII case and
punctuation with Caps Lock and Num Lock. It waits at most 100 ms for a pending
Caps Lock or Num Lock acknowledgement before sending another non-lock key press.
Scroll Lock does not delay input. Disconnect releases are retried if the guest
input queue is full, including after restore. Arbitrary guest keyboard layouts
and Unicode text injection are not implemented. See the
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
