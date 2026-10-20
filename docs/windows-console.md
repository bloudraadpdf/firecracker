# Windows console (experimental)

The optional console provides a private display/input endpoint without guest
networking or vsock. The AMD64 profile adds selectable graphics with boot VGA/VBE,
a PS/2 keyboard, a
USB HID pointer and optional read-only ATAPI installation media. The ARM64
profile adds an MMIO display and one combined virtio keyboard/absolute pointer.
See the [validation record](windows-validation.md#latest-qualified-integration-baseline)
for the current XP display/input results and historical AMD64 results. ARM64
guest qualification remains pending.

The AMD64 device path is:

```text
native GUI, remote browser or VNC viewer
             |
     SSH tunnel / HTTPS gateway
             |
     host console bridge (RFB)
             |
      private Unix socket
             |
       Firecracker event loop
        |       |       |       |
     display   PS/2   USB HID  ATAPI -- read-only ISO
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

### Optional native GUI

The `gui` Cargo feature adds a native framebuffer viewer. It uses winit and
wgpu; it does not change guest hardware or provide guest GPU acceleration.
VNC and browser access remain available. GUI libraries run in the viewer
process, outside the headless VMM's event loop and seccomp sandbox.

On a Linux desktop, build and launch from the repository root:

```sh
cargo build --locked -p firecracker --features gui
build/cargo_target/debug/firecracker --gui --enable-pci --no-api \
  --config-file /srv/vm/config.json
```

Local launch requires `--config-file` and `--no-api`. For an API-managed VM,
start its normal supervisor and use attach mode. The complete configuration must
include `console.socket_path` and the normal
firmware, disk and machine settings below. Use a private working directory.
The GUI starts the same executable as a headless child, forwards the other
arguments, and opens its console. Closing the window powers off and reaps that
child and removes its owned console socket; shut down Windows first to avoid
losing unsaved work. A guest shutdown
or reset closes the viewer. Start the command again to continue installation
after reset. The GUI does not restart VMs or change installation media.
If the console disconnects before the owned VMM exits, the window shows the
connection error and waits for the VMM's exit status. Close the window to power
off a VMM that remains running. Attach mode reports a disconnect immediately.

Linux GUI builds use the native GNU target and need an X11 or Wayland session
and a Vulkan driver. The normal desktop runtime libraries must be installed:
X11 needs `libX11`, `libXcursor`, `libXi`, `libXrandr`, `libxkbcommon` and
`libxkbcommon-x11`; Wayland needs its client/cursor libraries and
`libxkbcommon`. On Ubuntu, the X11 keyboard library is supplied by
`libxkbcommon-x11-0`. Firecracker does not supply a default seccomp policy for
GNU builds. Its existing empty-policy warning applies; a production deployment
must supply a reviewed GNU policy with `--seccomp-filter`. For a jailed or
headless host, keep the normal VMM build and use the separate viewer.

Build that viewer on Linux, macOS or Windows:

```sh
cargo build --locked -p console --no-default-features --features gui \
  --bin firecracker-viewer
```

Start one standard `rfb` bridge for the VM, then forward its port using the
[SSH instructions](../tools/windows/console/README.md#native-vnc-viewer).
On the local computer:

```sh
build/cargo_target/debug/firecracker-viewer 127.0.0.1:5900
```

On Windows the executable is `build\cargo_target\debug\firecracker-viewer.exe`.
Use `--password-file PATH` if the bridge requires VNC authentication. The file
must be a regular file of at most 64 bytes, containing the bridge's 1..8
printable ASCII password characters without spaces; trailing whitespace is
ignored. Use mode
`rfb`, including on macOS: this viewer sends standard keysyms. It needs no
password when the bridge uses security type None. Only loopback addresses are
accepted; remote access uses SSH forwarding. Closing an attached viewer leaves
the VM running. A Linux `gui` build also supports this attach mode:

```sh
build/cargo_target/debug/firecracker --gui-connect 127.0.0.1:5900
```

Use `--gui-password-file PATH` there if needed. Do not combine attach mode with
`--gui` or `--config-file`, or run a second bridge for the same VM.

The viewer uses linear filtering when scaling, preserves aspect ratio and maps
pointer coordinates through the same physical-pixel viewport used for rendering,
including on Retina displays.
The host cursor is hidden over the guest image. Focus loss releases held keys
and buttons. Command/Super sends the Windows key; Option sends Alt. The guest
must use the existing US keyboard mapping. These shortcuts are also available:

| Shortcut | Action |
| --- | --- |
| Ctrl+Alt+Home | Press and release the Windows key |
| Ctrl+Alt+End | Send Ctrl+Alt+Del |
| Ctrl+Alt+Enter | Toggle fullscreen |

On Mac keyboards, Fn+Left is Home and Fn+Right is End. Function keys remain
available to firmware and Setup. The viewer supports raw RGB888 and display
resize, with a 32 MiB frame bound and about 30 updates per second. It has no
audio, clipboard transfer, IME, reconnect loop or desktop toolbar. Host GPU
requirements apply to the viewer computer, independently of the guest OS.

Check the portable protocol and input code without KVM:

```sh
cargo test --locked -p console --no-default-features --features gui --lib
cargo clippy --locked -p console --no-default-features --features gui \
  --all-targets -- -D warnings
```

On Linux with an X11 display, Vulkan and the XTest library (`libXtst`), run the
native event tests as well. They use real window-close, keyboard, pointer and
focus events. Each case runs in a separate process because winit permits one
event loop per process. Xvfb with a software Vulkan driver can supply the display
on a headless host.

```sh
cargo test --locked -p console --no-default-features --features gui --lib \
  gui::tests::x11::native_window_lifecycle_and_input -- --exact --ignored
```

Linux bridge cancellation and VMM child cleanup are tested in the normal
`console` and `firecracker` unit tests with `--features gui`. Default and
`windows`-only builds have no GUI dependencies.

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
    "optical_drives": [
      {"drive_id": "setup", "path_on_host": "/srv/media/windows.iso"},
      {"drive_id": "exec"}
    ]
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

For Windows 10/11 with the pinned virtio display driver, select
`"display": "virtio-gpu"` in this object and use explicit `cloud-hv` firmware
without `vga_rom_path`. This attaches one virtio-VGA PCI function. Its boot display
uses the same VGA framebuffer; the guest driver then supplies native frames and
cursor updates to both VNC and the optional native viewer. The initial preferred
mode is 1024x768. Omit `display`, or select `vga`, for the basic VGA profile.
The Windows 10 candidate has passed native scanout, mode changes and full
restore. The USB clock correction also passes cold boot and pointer input. See the
[GPU candidate record](windows-validation.md#virtio-gpu-integration-candidate).

The AMD64 `"display": "qxl"` candidate exposes one revision-two QXL PCI
adapter for the existing Windows XP and Windows 7 XDDM drivers. Its boot VGA
and native driver share the framebuffer; there is no second display device.
Build the [QXL SeaVGABIOS profile](../resources/firmware/seabios-console/README.md#build-on-amd64-linux)
using `qxl-vgabios.config`, and select its ROM with the BIOS firmware profile.
Native QXL frames and cursor updates use the same console socket as VNC and the
native viewer. For XP and Windows 7, set `machine-config.vmclock` to `false` and
install the matching [generation-counter prerequisites](../tools/windows/generation/README.md).
The adapter advertises modes from 640x480 through 2560x1440 at 32 bits per pixel.
Change modes through the guest driver. The host `/display` update API remains
specific to virtio GPU.

QXL attachment, interrupt delivery, deferred work, error/reset handling and
snapshot restore have native KVM component coverage. Native XP/7 vendor-driver
loading and eighteen-mode pixel/input checks pass through the real RFB bridge.
See the [guest validation records](windows-validation.md#windows-xp-native-qxl-checks)
for signing conditions, cold boot, stopped-file evidence and full/differential
restore results. Both current legacy references report no device errors after
generation-counter servicing. The Mac native viewer additionally passes XP/7
text entry, guest window dragging and pointer targeting after host resize.
A Windows 7 post-restore navigation issue remains open; these results do not
establish complete Windows support.

For an installed Windows 10 guest, attach the pinned virtio driver ISO and run
this in an elevated command prompt, using its actual drive letter:

```bat
pnputil /add-driver D:\viogpudo\w10\amd64\viogpudo.inf /install
```

The package supplies `viogpures.exe`. For example, use
`D:\viogpudo\w10\amd64\viogpures.exe 1280 720` to request that mode. Verify the
active adapter and its error code in Device Manager after installation and cold
boot. For Windows 11, use its separate package:

```bat
pnputil /add-driver D:\viogpudo\w11\amd64\viogpudo.inf /install
```

Keep the complete driver directory when staging a package. Its INF lists the
required files; a SYS and catalogue alone can be insufficient. Windows 11's
display driver runs from DriverStore. Read its installed path with PowerShell:

```powershell
Get-CimInstance Win32_SystemDriver -Filter "Name='VioGpuDod'" |
    Select-Object Name, State, PathName
```

The [Windows 11 checks](windows-validation.md#windows-11-native-gpu-checks)
record the tested editions, driver, modes and remaining limits.

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
It rejects `vga_rom_path` and `optical_drives`. The device profile contains
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

## Request a modern display size

With `display: "virtio-gpu"`, request a preferred mode after VM start:

```sh
curl --unix-socket firecracker.sock -X PATCH http://localhost/display \
  -H 'Content-Type: application/json' \
  -d '{"width":1280,"height":720}'
```

Both dimensions are required. Each must be 1..4096, and the framebuffer must fit
within 32 MiB at four bytes per pixel. Invalid requests leave the preferred mode
unchanged. This endpoint requires the Windows feature on AMD64 and the modern
GPU; legacy VGA and ARM64 return an unsupported-path or device error.

A 204 response means the preferred mode was accepted and a virtio display-change
notification was issued when needed. The guest controls its actual scanout. A
repeated size is a no-op. Saved GPU state retains the preference and pending
notification; reset retains the preference. Existing VNC and native viewers
follow the guest's resulting framebuffer size.

For automatic Windows changes, install the matching upstream VioGpu resolution
service and session helper supplied with the external driver package. The
[driver's configuration handler](https://github.com/virtio-win/kvm-guest-drivers-windows/blob/d92b9c073ec1108f51846d0bd109c5796b46fd36/viogpu/viogpudo/viogpudo.cpp)
notifies that service after reading the new display information. Register the
service once from elevated PowerShell. For example, with the matching Windows 10
package directory on drive D:

```powershell
$driver = 'D:\viogpudo\w10\amd64'
Copy-Item "$driver\vgpusrv.exe", "$driver\viogpuap.exe" "$env:windir\System32\"
& "$env:windir\System32\vgpusrv.exe" -i
Get-Service vgpusrv
```

Keep both files in System32: the service launches `viogpuap.exe` for each user
session. Use the package for the guest release and architecture. A preferred-size
request alone does not prove that the desktop has changed; inspect the returned
console frame. No custom guest service or display driver is built into Firecracker.

## AMD64 installation and reboot

`console.optical_drives` selects at most two read-only ATAPI masters on one PCI
IDE controller. List order selects primary then secondary; each `drive_id` must
be unique, nonempty and contain only letters, digits or underscores. Omit `path_on_host` to keep a physical drive attached
without a disc. An empty list attaches no controller. The earlier experimental
`optical_image_path` field is rejected.

Both channels start in PCI native mode. BAR0/BAR1 address the primary
command/control registers and BAR2/BAR3 address the secondary. Native interrupts
share PCI INTA on GSI16; acknowledging one drive does not clear the other's
pending interrupt. In compatibility mode, the primary uses `0x1f0`/`0x3f6` and
IRQ14, and the secondary uses `0x170`/`0x376` and IRQ15. Only attached drives
reserve their legacy IRQs. The PCI I/O-enable bit gates all ports.

Each drive supports read-only PIO ATAPI, eject and reload of its current medium.
DMA, audio and recording are not provided. The existing bridge eject command
addresses the primary drive. Host media changes require an idle, unlocked drive.
A failed update preserves its current medium. Changes report unit attention
(`06/28/00`) before media-dependent commands proceed. INQUIRY and media-event
polls leave that attention pending; REQUEST SENSE can report it directly.

Media-event polling reports the latest insertion or removal and current tray
status. Header-only polls retain the event. A full poll consumes the event
without clearing unit attention. This follows the
[MMC-4 section 6.7 polling rules](https://13thmonkey.org/documentation/SCSI/mmc4r05.pdf).
Pending notifications survive snapshots.

After boot, replace one drive's medium through the existing Firecracker API:

```sh
curl --unix-socket firecracker.sock -X PUT \
  http://localhost/optical-drives/exec \
  -H 'Content-Type: application/json' \
  -d '{"drive_id":"exec","path_on_host":"/media/command.iso"}'
```

The path and body IDs must match a drive attached at boot. Omit `path_on_host`
or set it to `null` to remove the disc and open the tray. Insertion closes it.
A successful update returns204; an absent drive, locked or busy drive, invalid
ISO, or pre-boot request returns400 with a fault message. Retry a busy drive
once its transfer ends. Updates also work while the VM is paused. They do not
add hardware or change the channel order. `GET /vm/config` and subsequent
snapshots record the new path only after the update succeeds. Under the jailer,
make each replacement ISO available inside the jail before sending its path.
Retain immutable replacement files for snapshot restore.

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

Each attached drive identifies normally with no inserted disc and preserves
that state across restore. Closing an empty tray does not create a medium.
Restore checks the channel layout and each medium's identity before attachment.

## State and platform limits

The `windows` feature contains a virtio-gpu 2D command decoder, queue processor
and software renderer. They follow
[VIRTIO 1.3, section 5.7.6](https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html).
Backing lists are limited to 8192 entries. The decoder checks request sizes,
queue selection and nonzero resource identifiers, and preserves requested fence
identifiers in response headers. The renderer supports one cropped scanout,
scattered guest backing, explicit rectangular transfers and flushes, eight
32-bit pixel formats and a 64x64 cursor. A resource is limited to 32 MiB and
4096 pixels per dimension; all resources together are limited to 64 MiB and
64 resource identifiers. Cursor colours use premultiplied alpha.

The queue processor accepts split descriptor buffers and validates their guest
addresses, direction, request length and response capacity before executing a
command. It completes rejected commands with an error response when the output
buffer is valid, and preserves requested fence identifiers on error responses.

Renderer checkpoints retain resources, guest backing, preferred dimensions,
unflushed pixels, the displayed scanout and the copied cursor. Restore validates
their limits and guest addresses before cloning pixel buffers. Unflushed resource
pixels remain separate from the displayed pixels across restore.
Resource pixels and backing entries use bounded flat arrays. Decoding checks
array lengths before allocating their contents; resource ranges must cover those
arrays in order, without overlaps, gaps or unused trailing data.

The GPU device implements modern feature negotiation, queue activation and
notifications, display-change events, reset and device checkpoints. Device state
includes queue progress and pending display events; transport restore reactivates
it. The PCI transport identifies the GPU as 1af4:1050, with display class 03.
On AMD64 its capabilities and MSI-X structures use BAR4, leaving BAR0 and BAR2
for the VGA framebuffer and registers required by the firmware. VGA memory,
register decode, BAR relocation and KVM slot lifetime have one implementation
for reuse by both PCI display profiles. Other virtio devices keep their existing
BAR layout. The AMD64 GPU transport also maps and checkpoints the VGA framebuffer,
registers and legacy ports, with PCI memory/IO decode and BAR relocation.
Select `"display": "virtio-gpu"` to attach it. The console uses VGA until the
driver selects a native scanout, shows black when native scanout is disabled,
and returns to VGA after device reset. The
[validation record](windows-validation.md#windows-11-native-gpu-checks) gives the
Windows 10/11 driver results and remaining viewer and restore checks. The earlier
full installed-reference matrix covers the legacy VGA profile.

AMD64 console snapshots contain display memory and registers, keyboard/mouse
queues, held input, pending releases, keyboard LEDs and pending lock updates,
USB controller/HID state, PCI state, optical transfer state and media identity.
The current snapshot format is 41.0.0. Basic VGA state adds about 16 MiB; QXL
also saves 48 MiB of private RAM/VRAM and its protocol ROM and control state. The
bounded snapshot reader accepts at most 128 MiB, including GPU state. The guest
disk remains a separate artifact, as with ordinary Firecracker snapshots.

ARM64 console snapshots save display/configuration and one typed virtio-input
state in the PCI device manager. The console reuses that backend on restore; it
does not save a second held-key or LED state. Pending frames, partial-frame
cursor, position, held releases and guest/pending lock LEDs travel with Input.
Missing, stray or mismatched console/input topology is rejected before backend
attachment. Format29 through format39 snapshots require their matching older
binaries; format41 has no decoder for them. Format40 also requires its matching
binary. Snapshots do not move between AMD64 and ARM64.

The historical format31 Windows 11 test edition was IoT Enterprise LTSC 2024. It
passed Setup's hardware check without registry bypasses. The optional
[secure platform](windows-secure-boot.md) adds TPM and protected firmware
variables. The recorded Standard Windows 11 Enterprise Evaluation secure matrix
also used format31. See [current validation](windows-validation.md) for the
current source and format; those historical results do not qualify format34 or
other editions.

The console reserves the VGA aperture at `0xa0000..0xbffff` and platform table
memory at 32..33 MiB. The IDE channels share PCI GSI16 in native mode and use IRQ14/IRQ15
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
disconnected. An empty `optical_drives` array removes the IDE controller; it does
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

Current XP and Windows 7 graphics use the existing QXL drivers. The
[native mode probe](../tools/windows/display/README.md) checks their eighteen
advertised modes through normal Windows display APIs. The prototype miniport
is no longer included; its recorded results above remain historical.

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

The native GUI was exercised on macOS with an existing Windows 10 Firecracker
desktop through an SSH-forwarded RFB bridge. Display, keyboard, Windows-key
shortcut and pointer operation were observed; motion coalescing and removal of
a second frame delay made pointer movement smoother. Fullscreen aspect ratio
and linear filtering were checked with a synthetic RFB framebuffer.

On AMD64 Linux, an X11/Vulkan software-rendering check launched a firmware-only
VMM through `--gui`, closed its window and verified child reaping and console
socket removal. Protocol, input, bridge cancellation and socket ownership use
normal Rust unit tests. The Windows AMD64 viewer target passes `cargo check`;
native Windows and Wayland viewer sessions have not been exercised. These GUI
checks do not expand the guest qualification matrix.

The [validation record](windows-validation.md) lists exact tested editions and
installation, input, egress and restore checks. The
[console bridge](../tools/windows/console/README.md) describes native VNC and
browser access, authentication and the bounded input protocol.
