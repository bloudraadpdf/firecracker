# Firecracker XP x86 linear display prototype

This original Rust PnP miniport supplies sixteen bounded 32bpp modes for XP SP3
x86 on Firecracker's PCI display. The service is `fcvbe`; Windows supplies the
inbox XPDM `framebuf.dll`. The package is unsigned and experimental.

The recorded 0.1.2.0 package on VMM `93434437` passed all sixteen modes,
full-frame RGB comparisons, five pointer targets per mode and saved printable
ASCII with Caps Lock enabled. It restored the original mode and shut down
normally. See the [validation record](../../../docs/windows-validation.md). This
does not qualify a rebuilt SYS, the Windows Keep/Revert UI, browser/VNC input,
nonzero-mode cold boot or restored display/input state. Earlier binary
inspection and unchanged-source unit results are retained separately.

The mode table lists 640x480, 800x600, 1024x768, 1280x1024, 1600x1200, 1152x864,
1280x768, 1280x800, 1280x960, 1440x900, 1400x1050, 1680x1050, 1920x1200,
1280x720, 1920x1080 and 1600x900, in that index order. All use 32bpp XRGB,
width\*4 stride and zero pan. The 60Hz field is virtual metadata, not a physical
timing claim. There is no EDID, offscreen, palette, banked/planar, VDM,
acceleration or multi-monitor qualification.

One validated 16MiB mapping remains stable across mode changes; the reported
visible length follows the selected mode. The driver publishes a current mode
only after complete geometry/enable/stride/bank/pan readback. Power Off retains
selection; On restores that mode without clearing. Reset restores captured boot
state and clears the current-mode claim. The recorded mode test covers mode
changes. Power and snapshot behaviour still need their own guest checks.

## Installation contract

The INF matches only `PCI\VEN_1234&DEV_1111&SUBSYS_11111234&REV_01` on XP x86.
Empty NT 5.2/NT 6 model sections prevent later x86 systems selecting this
prototype. It neither changes the device identity nor associates the existing
`vga` service.

Normal SetupAPI copies the owned SYS to DIRID 12 (`system32/drivers`) and
associates `fcvbe` with the device. The system-start kernel service uses the
Video load group, following the pre-Vista display-driver contract. The existing
XP `.SoftwareSettings` path directly supplies `InstalledDisplayDrivers` as
REG_MULTI_SZ `framebuf`, and `VgaCompatible` as REG_DWORD 0. The prototype does
not implement the full VGA request surface.

The existing Windows XPDM `framebuf.dll` stays in `system32`. It is not copied,
modified or included in this package. Before installation, require an authentic
XP `framebuf.dll` from the tested guest image. Record and verify its matching
guest provenance and bytes for each image. The INF uses no `Include`/`Needs`
bridge to `display.inf` or `vga.Services`.

This package has no catalogue or embedded signature. It has no WHQL or release
signing qualification. Keep the existing XP driver-signing policy. If normal
Warn policy prompts for this unsigned package, record the prompt and operator
consent. A Block result is a real failure. No policy, certificate, registry,
service or PnP identity is changed outside the normal installer.

## Normal isolated installation

Use a fresh isolated XP test disk. Preserve the original reference.

1. Stage both package files together in `C:\Drivers\FCVBE`, and verify their
   bytes through the separate stopped read-only phase. Confirm the owned
   service/package is absent and the inbox framebuf matches the recorded guest
   provenance.
1. Cold-boot that clone with one explicitly reviewed VMM/hardware binding. In
   Device Manager, select the existing matching PCI adapter and choose Update
   Driver, manual selection, then Have Disk.
1. Select the directory `C:\Drivers\FCVBE` in the manufacturer's-files field, or
   use Browse to select `fcvbe.inf`. Do not type the full INF file name into a
   directory field. Select the matching prototype model.
1. Retain the actual unsigned prompt, selected INF, service, copied SYS hash,
   SetupAPI log and reboot request. Use a normal restart when required.
1. After a separate cold boot, verify the active selected INF/service and Code
   0, actual pixels, saved keyboard text and CapsLock, pointer bounds and clean
   S5. Installation completion is not activation or cleanup proof.

For a normal update on an isolated disk with an existing prototype, stage the
new package in a fresh directory, such as `C:\Drivers\FCVBE-new`. Preserve the
old package and installed SYS before the normal Windows update. Select that
fresh directory through Have Disk; Windows, rather than the staging phase,
replaces the installed SYS. A successful install still needs a separate cold
check.

## Adapter information

After validating the actual PCI resources and 16 MiB memory count, the driver
uses `VideoPortSetRegistryParameters` to report `Firecracker linear framebuffer`
and a four-byte memory value of 16777216. The retained XP DDI writes REG_BINARY;
the memory bytes are `00 00 00 01`, decoded as a little-endian u32. Adapter text
is null-terminated UTF-16. Do not force REG_DWORD or edit these values manually.
Chip, DAC and BIOS identity remain unset because no distinct identity has been
validated. Information-write failures are logged and do not reject the adapter.

The [Microsoft WMI consumer contract][memory] specifies bytes for AdapterRAM and
maps it to `HardwareInformation.MemorySize`. The [legacy producer table][info]
says MB. This conflict remains a runtime gate: after normal installation, retain
the actual registry type/data and verify that the XP Advanced Adapter tab shows
16 MB and the approved adapter name. An incorrect UI result fails this image's
information qualification and requires a corrected driver.

## Build and review

These are kernel sources: no_std, no allocator, no user CRT, VC-LTL or
YY-Thunks. User-mode XP compatibility libraries remain separate from kernel
imports. The external WDK 7.1 XP x86 `videoprt.lib` and headers are not
redistributed. Original Rust/INF source uses the repository's existing
Apache-2.0 licence. No C implementation, QEMU or GPL dependency is introduced.

First follow the existing
[Windows build prerequisites](../README.md#build-prerequisites), including the
i686 Rust target. Set `XP_RUSTC` to the explicit reviewed Rust 1.98.1 compiler,
`XP_LINK` to its matching lld-link, and `XP_WDK_X86_LIB` to the external
directory containing that XP x86 import library. Run from the repository root. A
fresh output under the existing ignored build directory keeps the reviewed
package intact:

```sh
(
set -eu
: "${XP_RUSTC:?Set the explicit Rust 1.98.1 compiler path}"
: "${XP_LINK:?Set its matching lld-link path}"
: "${XP_WDK_X86_LIB:?Set the WDK XP x86 library directory}"
XP_SOURCE="$PWD/tools/windows/display"
XP_BUILD="$PWD/build/windows/display-xp"
mkdir -p "$PWD/build/windows"
mkdir "$XP_BUILD"
cp "$XP_SOURCE/fcvbe.inf" "$XP_BUILD/fcvbe.inf"
"$XP_RUSTC" --edition 2024 --crate-name fc_vbe --crate-type lib \
  --target i686-pc-windows-msvc "$XP_SOURCE/miniport.rs" \
  --emit "obj=$XP_BUILD/fc_vbe.o,llvm-ir=$XP_BUILD/fc_vbe.ll" \
  -C opt-level=2 -C panic=abort -C codegen-units=1 -D warnings
"$XP_LINK" /driver /subsystem:native,5.1 /osversion:5.1 \
  /entry:DriverEntry@8 /machine:x86 /nodefaultlib /manifest:no \
  /incremental:no /opt:ref /opt:icf /fixed:no /release \
  "/out:$XP_BUILD/fcvbe.sys" "/map:$XP_BUILD/fcvbe.map" \
  "/libpath:$XP_WDK_X86_LIB" "$XP_BUILD/fc_vbe.o" videoprt.lib
)
```

The reviewed WDK `videoprt.lib` also supplies `KeBugCheckEx` from
`NTOSKRNL.EXE`; its archive name does not name the runtime DLL. Other SDK inputs
require their own symbol/DLL and final-image review.

The local source has 36 passing unit cases: all 27 prior safety tests plus nine
meaningful table, mode, mapping and power tests. These use the existing decoder
and real-callback mock. These tests do not prove guest load, pixels, mode
application or restore.

From the repository root, use the reviewed host Rust compiler to run the unit
tests in a fresh ignored output directory. This uses the same source and adds no
test framework:

```sh
(
set -eu
: "${XP_RUSTC:?Set the explicit Rust 1.98.1 host compiler path}"
XP_SOURCE="$PWD/tools/windows/display"
XP_TEST_BUILD="$PWD/build/windows/display-xp-tests"
mkdir -p "$PWD/build/windows"
mkdir "$XP_TEST_BUILD"
"$XP_RUSTC" --edition 2024 --test "$XP_SOURCE/miniport.rs" -D warnings \
  -o "$XP_TEST_BUILD/driver-tests"
"$XP_TEST_BUILD/driver-tests"
)
```

After normal installation, test every advertised mode through Display settings:
actual dimensions, colour/stride, high and low edges, pointer bounds,
apply/revert including timeout, saved text/CapsLock, clean S5 and selected
nonzero-mode cold boot. Full and differential restore must retain geometry, VRAM
and input. TAP and CONNECT remain separate. An installed adapter, listed modes
or a working-properties message does not establish these results.

A rebuilt image needs its own full image review: XP native subsystem,
stdcall/ABI, checksum/relocations, actual stock imports, resident code, stack
and complete executable-section disassembly. The standard user-mode Rust target
permits SIMD; integer source and no_std do not make a future image kernel-safe.
The reviewed image's 16 videoprt imports include six forwarders whose exact
targets exist in both retained XP kernel candidates. That proof does not select
a kernel or qualify a rebuilt image.

Primary contracts are the Microsoft
[Manufacturer/OS selection](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/inf-manufacturer-section),
[SourceDisksFiles](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/inf-sourcedisksfiles-section),
[AddService](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/inf-addservice-directive),
[pre-Vista display start type](https://learn.microsoft.com/en-us/windows-hardware/drivers/display/setting-the-start-type-value)
and
[normal update API](https://learn.microsoft.com/en-us/windows/win32/api/newdev/nf-newdev-updatedriverforplugandplaydevicesa).
The pinned actual XP `display.inf` provides the legacy `.SoftwareSettings`,
typed settings and Video-service/event-log precedent. Modern WDDM-only registry
sections are not substituted for that XP contract.

[info]: https://learn.microsoft.com/en-us/windows-hardware/drivers/display/setting-hardware-information-in-the-registry
[memory]: https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-videocontroller
