# Windows guest validation

Windows support is experimental and incomplete. A pass below applies only to its
tested guest, installed drivers, firmware, VMM and route. Use the
[AMD64 guide](windows.md), [console guide](windows-console.md) and
[secure platform guide](windows-secure-boot.md) to prepare a test.

## XP and Windows 7 native graphics work

The pinned virtio-win 0.1.302 ISO includes existing QXL packages for XP x86
(`5.1.0.10018`) and Windows 7 x64 (`6.1.0.10024`). Their INFs match
`PCI\VEN_1b36&DEV_0100&SUBSYS_11001af4` and install `qxl.sys` with `qxldd.dll`.
These packages need the QXL device interface. They do not bind to the modern
virtio GPU used in the Windows 10 checks below.

Native XP/7 graphics through those existing packages is incomplete. The first
Rust component decodes packed commands, primary surfaces, signed rectangles,
ring counters and generation-tagged memory-slot pointers. Shared memory now
provides a packed ROM with 18 bounded 32bpp modes, command/cursor/release rings,
atomic control fields, and RAM that shares the existing boot framebuffer.
The transport validates registered RAM/VRAM slots, rejects stale generations,
and bounds each command/cursor dispatch to 32 entries. A full release ring
stops execution before it can lose an acknowledgement. Commands are released
only after their handler succeeds. Each dispatch publishes one linked release
chain. If a later command fails, its successful prefix is still released and
the failed command remains pending. Duplicate addresses cannot form a chain
cycle. Boundary, reset and transport tests check
those components. Primary-surface checks cover signed row strides, exact frame
pixels, clipped fills, overlap-safe copies and untouched row padding. Raw bitmap
decoding covers indexed bit/nibble order, RGB555, RGB24, RGB32, alpha, palettes
and top-down/bottom-up rows. Direct and chained storage use checked memory-slot
pointers, with a 32 MiB data limit and at most 4,096 chunks. Tests reject invalid
dimensions, strides, palette indices, backlinks, truncated chains and cycles.
Surface raster operations cover source/brush selection, input and result
inversions, copy, OR, AND, XOR, blackness, whiteness and destination inversion.
Bitmap copies support source crops, clipped destinations and nearest-neighbour
scaling while preserving row padding. Tests cover descriptor validity and bit
truth tables, signed destination strides and rejection before destination writes.
Prepared modern and compatibility drawing commands render fills, patterned
brushes, bitmap and screen copies, blackness, whiteness and inversion. Clip
rectangles form a union; overlapping clips apply the operation once per pixel.
One-bit masks support origins and inversion without a colour palette. Screen
copies capture the source before writing, including alpha bits. Command-ring
checks cover failure without writes or acknowledgement and successful retry.
Image commands also cover opaque source/brush combinations, raster blending,
all 256 ROP3 operations, true-colour transparency and premultiplied alpha with
constant opacity. Alpha commands decode both layouts and honour destination
alpha. Unused source and brush fields are not dereferenced. Scaling supports
nearest pixels and bilinear interpolation with source-edge clamping.
Text commands decode positioned A1, A4 and A8 glyphs, top-down and bottom-up
rows, foreground raster operations and optional backgrounds. Overlapping glyphs
form one coverage mask before clipping and painting. Counts, packed data and
coverage allocations are bounded; malformed later glyphs cannot leave a partly
painted background. The existing driver's A4 coverage uses 16-step A8 levels.
The cosmetic-line rasteriser preserves Q28.4 endpoints, clips work to the
surface dimensions and uses wide integer arithmetic for coordinate extremes.
Its pixel output matches 778 captured native GDI cases, including fractional
starts, short segments, reversed directions and endpoint ties. The reference
and capture procedure are in [the QXL test data](../tests/data/qxl/README.md).
Stroke commands now decode continued and closed cosmetic paths in both drawing
layouts. Styles support fixed-point dash lengths, odd-length cycles, initial
gaps, explicit style resets and phase progression through clipped pixels.
Foreground and background raster operations share the brush; a zero descriptor
leaves the destination unchanged. Unused solid-line style and background fields
are not read. Prepared stroke pixels use the common clip union and retain
operation order at crossings. Limits are 1 MiB of packed path data, 65,536
points, 100 style entries, 16,777,216 raster steps and 1,048,576 prepared pixels.
Checks cover continuation, closure, clipping, reversed dash progression, chunk
boundaries, malformed records and rejection before writes. The existing Windows
driver does not advertise hardware Bezier or geometric-wide-line support; GDI
decomposes those requests before submission. Such packed path commands remain
unsupported. Native XP/7 stroke and dash rendering still needs validation.
Image commands can reference primary surface 0. They capture its pixels before
destination writes, so overlapping and scaled source reads use the original
frame. Surface descriptors use their 22-byte layout and can have zero dimension
fields, as emitted by the Windows driver. Alpha commands use source-surface
alpha only when its flag is set; raw RGBA images keep their own alpha. Invalid
surface IDs and source crops fail before destination writes. Offscreen surfaces,
composite and self-bitmap commands remain unsupported.

The integration targets revision 2 with one primary surface and compression
disabled. The [pinned Windows driver source](https://github.com/freedesktop-unofficial-mirror/spice__win32__qxl/tree/24cbb39f18538672f0a6d6788d5643ad272ac13c/xddm/display)
uses synchronous I/O for revision 2
and raw bitmap fallback when the ROM's compression level is zero, including for
alpha images. It initialises `self_bitmap` to zero and has no composite command
producer. The remaining integration and native checks must verify those paths
with the signed packages.

Cursor commands decode set, move, hide and trail parameters without reading
unused union fields or guest scratch space. Shape decoding covers alpha,
monochrome and 4/8/16/24/32-bit colour cursors, palettes and MSB-first masks.
It reuses bitmap pixel decoding and premultiplied alpha blending. Cursor drawing
clips the hotspot at display edges and preserves monochrome inversion and the
colour-mask rules. Shapes are limited to 256 by 256 pixels. The zero-sized
transparent cursor does not read unused chunk links. Cursor commands and trail
parameters still need controller integration and native guest validation.
The layout follows
the published [QXL device ABI](https://sources.debian.org/src/spice-protocol/0.12.14-1/spice/qxl_dev.h/).
It does not expose a QXL PCI device yet. Component checks do not establish guest
driver loading or rendering. Graphics qualification across all agreed guests
must finish before work returns to unrelated platform features.

## Windows 11 native GPU checks

Runtime source `ae208ad25` passes native AMD64 scanout checks on Windows 11 Pro
and Enterprise Evaluation, both reporting build 26300. Each used an isolated
disk copy and its matched Secure Boot and TPM state. The original references
were unchanged. These were GNU development runs with `--no-seccomp`.

Both guests accepted the ISO's `viogpudo/w11/amd64` package through normal
`pnputil` installation. The active `VioGpuDod` driver reports Code 0, version
`100.103.104.30200` and a Microsoft Windows Hardware Compatibility Publisher
signature. Its installed SYS matches SHA-256
`adb624a34dc1b6519af0601268af7c1e3e8972c16c69467f6e3144e66b67f5e1`.
Windows 11 installs this file in DriverStore; the service's `PathName` identifies
the active file. Both resolution helpers have valid WHCP signatures.

Each guest changed actual scanout through 1024x768, 1280x720, 1366x768,
1920x1080 and back to 1024x768. All 22,400 checked colour pixels match at each
step. Windows repositions the test window during mode changes; the checks use
its observed position. Both guests recorded five pointer clicks. Enterprise
also recorded all five submitted text strings exactly. Pro's separate input
check records both Control+A chords with Control held and shows the Windows
Start menu after the Windows key. It does not establish the test TextBox's
select-all behaviour.

After installing the complete Windows 11 vsock package, all 52 present devices
report OK in both guests. Enterprise also reports Secure Boot enabled and TPM
present and ready. Both guests shut down through ACPI S5 with VMM exit code 0.

Windows 11 IoT Enterprise LTSC build 26100 also passes these five modes, pixel
checks, literal text submissions and pointer clicks on the same source. A fresh
64 GiB disk completed graphical UEFI installation and two setup resets with the
optimised GNU VMM. Its signed Windows 11 GPU package has the same version and SYS
hash above. The complete vsock package installed with exit code 0. After omitting
the unused VMClock device with `"vmclock": false`, all 48 present devices report
OK. The guest completed ACPI S5 with VMM exit code 0.

An earlier IoT installation failed applying `install.wim` with `0x80070570`
after full snapshot restore from the debug VMM into the optimised VMM. The WIM
passed complete host verification; the uninterrupted cold installation above
used the same prepared media and succeeded. The cause remains unresolved. This
result does not qualify Windows 11 GPU snapshot restore or viewer routes. The
earlier installed-reference results below use the legacy display profile.

## Virtio GPU integration candidate

Runtime source `67ac42dfc` adds the AMD64 `virtio-gpu` console profile. Windows 10
Enterprise LTSC 2021 x64, build 19044, accepted the pinned virtio-win 0.1.302
viogpudo package through normal `pnputil` installation, with exit code 0. The
active `VioGpuDod` service reports Code 0, version `100.103.104.30200` and a
Microsoft Windows Hardware Compatibility Publisher signature. The installed SYS
matches SHA-256 `97eb164029d8b14f9529d3b5e1313abf71dead7f3192fc7a2474e6e54dee41bd`.

The GNU development VMM `9a5950db` used the graphical CloudHv firmware
`730de0fc`, one disposable reference-disk copy, one GPU and no option ROM. It ran
with `--no-seccomp`; this is display evidence, not production sandbox evidence.
The original reference disk was preserved.

Native frames pass at 1024x768, 1280x720 and 1920x1080. All 41,600 checked pixels
inside a guest-drawn rectangle match RGB `0d4795`. A full format-37 snapshot
restores the active 1920x1080 scanout; the resumed guest accepts keyboard input,
changes mode to 1024x768 and opens Start and Recycle Bin through absolute pointer
clicks. The resumed guest reports 59 healthy present devices and completes S5
with VMM exit 0. The 4 GiB RAM artifact was retired after evidence capture.

Two subsequent unoptimised cold boots retain the healthy GPU but report Code 43
on the USB controller. The unchanged optimised VMM `85a79d13` starts both devices
and reports 59 healthy present devices. A regression test reproduces a stopped
USB frame counter when timer dispatch is delayed. USB now samples monotonic time
on guest I/O as well as timer events, with bounded catch-up and no advancement
while the VM is paused. No snapshot field or format change is needed.

The corrected unoptimised candidate `10415388` cold-boots both devices, reports
59 healthy present devices, opens Start through the absolute pointer and accepts
elevated keyboard commands. Both GPU and USB report Code 0, and the guest
completes S5 with VMM exit 0. The six USB integration tests and strict VMM Clippy
pass with and without Windows support. The clock regression fails on the old
implementation and passes with the correction, including the paused-clock check.

The result does not qualify Windows 11, fresh installation,
browser/VNC/native-viewer interaction, host-requested resize, differential GPU
restore or the remaining Toets integration.

## Host-requested display modes

Source `7d3bab807`, GNU development binary `527be718`, requests preferred display
sizes through `PATCH /display`. One Windows 10 reference session uses the
existing VioGpu helpers from pinned virtio-win 0.1.302. Both executables have valid
Microsoft Windows Hardware Compatibility Publisher signatures and match the
retained package hashes. The upstream `vgpusrv` service is running with automatic
startup; its session helper `viogpuap` remains present in all 38 observations.

The actual guest dimensions and captured native console frames follow:

```text
1024x768 -> 1280x720 -> 1366x768 -> 1920x1080 -> 1024x768
```

The 1366x768 request tests a custom width. An invalid 4096x4096 request returns 400
without changing the 1024x768 scanout. Repeating 1024x768 returns 204 and preserves
that scanout. All 60 present PnP devices report healthy; Windows completes S5 and
Firecracker exits0. The stopped disk is collected read-only and the loop is
detached. No new disk clone or snapshot RAM file is required.

This run retains the earlier firmware and installed kernel drivers and uses
`--no-seccomp`. It proves these host-requested changes with the external Windows
helpers, not sandboxing, other Windows versions, fresh installs, viewer routes
across mode changes or real format40 guest restore. Component tests cover saved
preferences and pending notifications. The guest clock offset remains unchanged.

## Runtime optical media candidate

Source `d8e5bed69`, GNU development binary `6a307899`, cold-boots the same
Windows 10 reference with two optical masters and snapshot format40. The primary
starts loaded as D:; the secondary starts empty as E:. Both enumerate as healthy
`FC Virtual CD-ROM ATA Device` instances.

One bounded guest session records these states in order:

```text
D: primary-a   E: empty
D: primary-a   E: secondary-b
D: primary-c   E: secondary-b
D: primary-c   E: empty
D: empty       E: empty
D: primary-a   E: empty
D: primary-a   E: secondary-b
D: primary-a   E: finish
```

All 57 observations have no read errors; each loaded disc has the expected label,
sentinel and SHA-256. Seven runtime PUT updates succeed. A missing replacement
ISO returns400 and leaves the reported secondary path unchanged. All 60 present
PnP devices report healthy. The finish disc ends the monitor, Windows completes
S5 and Firecracker exits0. Collection uses the stopped disk through a read-only
loop and NTFS mount; both are detached afterwards. No new VM disk or snapshot
RAM file is required.

This run uses the retained graphical firmware and installed drivers, with
`--no-seccomp`. It qualifies these Windows 10 media changes, not production
sandboxing, fresh installation, other Windows versions or real format40 guest
restore. Component tests cover pending notifications and full device-manager
restore. The inherited guest clock offset is unchanged and unqualified.

## Latest qualified integration baseline

The latest qualified AMD64 Windows-feature binary is `6f63cd8d`, built from runtime
source `95aaccfb` and using snapshot format 34. Its default counterpart is
`1c68a888`. Later documentation and secure-helper cleanup changes do not change
these binaries. The table records completed installed-reference checks on this
runtime separately from unchanged component results and remaining edition gates.

Current source uses snapshot format 40 for two-channel optical state and pending
media-change notifications. The
format-34 baseline below does not qualify the GPU or format-40 restore path.
The GPU candidate above records actual format-37 restore evidence; it does not
qualify the later optical state format.

Reuse a recorded result when the inputs relevant to that check are unchanged.
Retain the guest configuration, driver and firmware hashes, VMM capability and
source identity with each result. A documentation change does not require a new
VMM build or guest run. Repeat a check for changed behaviour or an unresolved
failure, and retain the earlier failed record.

| Area                                       | Latest qualified result                                                                                                                                                                                                                                                                                                                                                                                                        | Remaining gate                                                                                                                           |
| ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------- |
| XP SP3 x86 display modes                   | Unchanged miniport 0.1.2.0 on recorded `93434437`: all 16 advertised 32-bit modes pass full-frame RGB, five pointer targets per mode and saved printable ASCII with Caps Lock.                                                                                                                                                                                                                                                 | Normal Keep/Revert/timeout UI, browser/VNC input, other layouts and guests.                                                              |
| XP current input and persistence           | Current `6f63cd8d`: nonzero 1024x768x32 cold mode, Windows Run/absolute pointer/raw Caps Lock input, held Shift across full/differential restore, explicit release and exact saved `aAaAAaAAa` pass.                                                                                                                                                                                                                           | Broader input/UI coverage beyond the tested paths.                                                                                       |
| XP TAP and CONNECT                         | Current `6f63cd8d`: both routes pass cold/full/differential transfers, stored data, public verification, both format-34 headers and final S5.                                                                                                                                                                                                                                                                                  | Legacy XP HTTP does not qualify modern TLS; fresh installation and other images remain separate.                                         |
| Windows 7 SP1 x64                          | Current `6f63cd8d`: Windows Run/absolute pointer/raw Caps Lock and saved `aAa`; both TAP and CONNECT pass cold/full/differential transfers, storage, public verification, format-34 headers and S5. TAP generation notifications also pass.                                                                                                                                                                                    | Held-key restore on this guest and fresh installation remain unqualified.                                                                |
| Windows 10 Enterprise LTSC 2021 x64        | Current `6f63cd8d`, 19044.1288: signed driver setup, RTC-policy reboot, normal SYSTEM task, saved `aAa` input and both transport routes pass.                                                                                                                                                                                                                                                                                  | Held-key restore is separately tested on XP; fresh installation remains separate.                                                        |
| Windows 11 IoT Enterprise LTSC x64         | Current `6f63cd8d`, 26100.1742: signed driver setup, RTC-policy reboot, normal SYSTEM task, saved `aAa` input and both transport routes pass.                                                                                                                                                                                                                                                                                  | Fresh installation, other editions and broader input paths remain separate.                                                              |
| Windows 11 Pro x64                         | Current `6f63cd8d`, 26300.9457/26H2: input, Secure Boot/TPM, authenticated UEFI cold deletion and TAP/CONNECT cold/full/differential checks pass.                                                                                                                                                                                                                                                                              | Enabled BitLocker protection, TPM-sealed recovery, attestation and fresh installation are not qualified.                                 |
| Windows 11 Enterprise Evaluation x64       | Current `6f63cd8d`, 26300.9457/26H2: signed drivers, policy reboot/input/task, Secure Boot/TPM and both TAP/CONNECT cold/full/differential checks pass, including secure checks, public verifiers, two format-34 headers per route and S5.                                                                                                                                                                                     | Fresh installation and other editions remain separate.                                                                                   |
| Windows Server 2025 Core x64               | Current `6f63cd8d`, ServerStandardEval 26100.32230: fresh installation and ISO-free boot, signed 2k25 drivers, RTC-policy cold boot, normal SAC input with exact stored `aAa` plus CRLF, 42 healthy present devices and normal SYSTEM task pass. TAP and CONNECT pass cold/full/differential checks, stored data, public verifiers, both format-34 headers per route and normal S5/VMM exit zero.                              | Graphical input and Secure Boot/TPM are not qualified by this ordinary firmware profile.                                                 |
| Windows Server 2025 Desktop Experience x64 | Current `6f63cd8d`, ServerStandardEval 26100.32230: fresh installation and ISO-free boot, signed 2k25 drivers, RTC-policy cold boot, 48 healthy present devices, Windows Run/absolute pointer/raw Caps Lock with exact stored `aAa`, and normal SYSTEM task/TAP staging pass. TAP and CONNECT pass cold/full/differential checks, stored data, public verifiers, both format-34 headers per route and normal S5/VMM exit zero. | Secure Boot/TPM are not qualified by this ordinary firmware profile.                                                                     |
| Host regression checks                     | Current default/Windows unit, documentation, example and benchmark checks, standalone tools, platform module and Linux PMEM/bzImage checks pass; default API, jailer, seccomp and snapshot checks also pass.                                                                                                                                                                                                                   | Results cover the tested Ryzen 5950X/Linux 7.0 host and Linux 6.1.188 guest; CPU-template skips and supported-host coverage remain open. |
| ARM64 and performance                      | Final default/Windows boot and restored-memory measurements complete; the maintained analyser flags executable-text and restored-memory costs.                                                                                                                                                                                                                                                                                 | Native ARM64/KVM qualification and the flagged footprint comparisons remain open.                                                        |

Both Secure Boot references identify Windows 11 x64 26300.9457/26H2; Enterprise
is the Evaluation edition. No activation or licence changes were made. Pro later
reached 100% encryption; Enterprise's recorded volume was 80% encrypted. Both
were unlocked with protection off. Clear-key read-only verification does not
qualify enabled BitLocker protection, TPM-sealed recovery or attestation.

Completed current transport rows require all three fresh phase proofs, stored
bytes, public offline verifier exit zero, both actual format-34 headers and
normal S5/VMM exit zero. CONNECT also requires three allowed bidirectional 2 MiB
transfers and three zero-payload denials with six sealed accounting entries.
These checks do not preserve live TCP connections or qualify Tokoloshe's Windows
job runtime. Original images and failed attempts remain preserved; disposable
working disks and snapshot RAM were retired only after compact proof fetch and
hash verification.

The retained XP all-mode component result uses one installed Professional SP3
x86 reference, SeaBIOS and the normally accepted unsigned XPDM warning, without
a signing-policy or registry repair bypass. Its 16 files contain exact printable
ASCII bytes 32 through 126; logs confirm 80 accepted pointer targets including
(0,0), mode application and restoration. The current persistence test separately
retains exact nine-byte saved input across cold/full/differential execution.

## Measured footprint

The maintained boot-time and memory-overhead tests ran once per binary set, with
their normal sampling loops, PCI and MMIO, one vCPU, 128 MiB RAM and Linux
6.1.188. The retained baseline is consumer revision `658ff27e`, not upstream
main. Both final capability builds passed all selected test bodies and teardown.
The normal analyser, with unchanged thresholds, failed both comparisons:

- Baseline to final default: executable text increases by about 250 KB (7.08%);
  restored memory overhead increases by about 200 KB for MMIO and 260 KB for
  PCI.
- Final default to Windows capability: executable text increases by about 291 KB
  (7.70%); restored memory overhead increases by about 267 KB for MMIO and 293
  KB for PCI.

No boot metric was flagged. These are measurements on a shared host, not
qualification on a dedicated performance host. The default build includes
transitional PCI/INTx and common API/snapshot decoding; optional live Windows
devices are feature-gated. Source review found no confirmed missing feature
gate, but does not attribute the measured bytes or prove that the implementation
cannot be smaller. The footprint failures remain recorded; no threshold change
or repeat run was used to obtain a pass.

## Run the maintained checks

Use the [normal test environment](../tests/README.md). The VMM unit suite needs
its documented swtpm/libtpms and ACPICA inputs; standalone tools have separate
workspaces. Run the existing build and platform entry points from the repository
root:

```sh
./tools/devtool test -- \
  integration_tests/build/test_unittests.py::test_tool_unittests \
  integration_tests/build/test_clippy.py::test_tool_clippy
./tools/devtool test -- \
  integration_tests/build/test_unittests.py::test_unittests \
  integration_tests/build/test_clippy.py::test_rust_clippy
./tools/devtool test -- integration_tests/functional/test_platform_devices.py
```

Omit `--binary-dir` for the platform module so its fixtures build both default
and Windows counterparts. These commands are regression entry points, not a
claim that the latest candidate has passed every test. See the
[test requirements](../tests/README.md#standalone-tools-and-platform-devices).

The affected USB and console tests require the Windows feature. On an AMD64
Linux host with the musl target and build prerequisites installed, reproduce the
focused checks and supported release profile with:

```sh
cargo test --locked --target x86_64-unknown-linux-musl --features windows \
  -p vmm --lib devices::usb:: -- --test-threads=1
cargo test --locked --target x86_64-unknown-linux-musl --features windows \
  -p vmm --lib device_manager::console_usb::tests:: -- --test-threads=1
cargo test --locked --target x86_64-unknown-linux-musl --features windows \
  -p vmm --lib device_manager::console_server:: -- --test-threads=1
cargo clippy --locked --target x86_64-unknown-linux-musl --features windows \
  -p vmm -p firecracker --all-targets -- -D warnings
cargo build --locked --target x86_64-unknown-linux-musl --features windows \
  -p firecracker --release
```

For the recorded source, the three test selectors run 18, 5 and 18 tests. Reject
zero selected tests. An all-features release build enables fuzzing and is not
the Windows release recipe. The
[XP display driver and mode probe](../tools/windows/display/README.md) include
their source, build inputs and a guest acceptance procedure. Their host unit
tests use the normal standalone-tool gate. The existing console screenshot
command and the probe's shared RGB model provide the full-frame comparison.
Retain all sixteen captures, stopped case files and the full probe log for a
fresh guest run. Host unit and console bridge tests do not replace that check.

### Guest egress and restore

Prepare authorised media with the [media tools](../tools/windows/README.md).
Complete driver installation and shut down before using an isolated test disk.
Follow the [egress procedure](../tools/windows/egress/README.md) for the exact
probe build, guest configuration, environment variables and offline-verifier
inputs. Legacy user-mode probes use the documented VC-LTL5/YY-Thunks link
profiles; a successful cross-link is not guest runtime qualification.

Choose one route. TAP uses the maintained network setup with a guest NIC and no
vsock; retain its network namespace through both restores. CONNECT uses the
[real Tokoloshe proxy](../tools/windows/legacy-vsock/README.md) and vsock with
no guest NIC. After setting the route's documented environment, run its
controller as the ordinary test user:

```sh
# TAP
timeout --signal=TERM --kill-after=120s 900 \
  bash tools/windows/egress/tap-check.sh
# CONNECT, in its separately prepared test directory/environment
timeout --signal=TERM --kill-after=120s 900 \
  bash tools/windows/legacy-vsock/check.sh
```

The TAP controller's public launcher uses sudo for its VMMs and editor. CONNECT
runs the VMM, proxy and sidecar as the same ordinary user. After the controller
has stopped and the disk is closed, run the matching offline verifier in a root
shell with the same documented verification environment:

```sh
# TAP
bash tools/windows/egress/tap-verify.sh
# CONNECT
bash tools/windows/egress/connect-verify.sh
```

Require cold, full and differential phase proofs, both actual snapshot version
headers, fresh guest output after each restore, byte-equal transfers, stored
data and final S5. CONNECT also requires six sealed accounting entries and
zero-payload policy denials. Enable the documented generation checks where
supported; XP uses polling, not notifications. For protected guests, use the
secure-state and read-only BitLocker procedures in the same egress guide. Retain
the final disk and its matching stopped variable/TPM stores together.

## Recorded format33 integration candidate

Format 33 and older results are historical. Their installed XP/Windows 7 TAP and
CONNECT passes, modern Windows installation and protected-runtime checks do not
qualify format 34 or the latest source. Earlier Secure Boot/TPM results do not
establish TPM-sealed recovery or attestation; recorded BitLocker protection was
off. The existing validation artifacts retain exact binary, firmware, media,
driver and source identities, failed runs and full logs. Rebuild snapshots for
the selected format; do not load an older saved state as an upgrade test.

### Earlier format-34 Windows 7 attempts

The earlier normal `f90f364e` receive failure and `93434437` caller's concurrent
Docker firewall-rule preservation failure remain retained as failed attempts.
Docker concurrency was observed, but the exact deletion actor and
receive-failure cause were not established. A Debug-logging pass did not repair
or qualify those failures. The separately completed current `6f63cd8d` runs are
the qualification entries above.

### Recorded Windows 7 reference

The historical format-33 Enterprise SP1 x64 reference retains signed NetKVM and
Microsoft generation-counter drivers after servicing and restart. Its TAP run
passes cold/full/differential transfer, storage and generation checks, including
both prearmed child completions. Wrong probe hashes and existing output paths
are rejected without changing the earlier data. A separate CONNECT clone passes
fresh connections in all three phases with the external test-signed Viosock
profile, six sealed accounting entries and final S5. These installed-reference
results do not qualify another policy, guest image or VMM.

### Recorded Windows XP reference

The historical format-33 Professional SP3 x86 reference passes cold queries with
Microsoft generation-counter 6.3.9600.17831 and running KMDF. Separate TAP and
CONNECT clones pass cold/full/differential transfers, polling with three
distinct generation IDs, final storage and S5. TAP uses normal NetKVM warning
consent; the separate default signature-policy check fails. Its original offline
collector also remains failed; a separate parser accepts the retained
trailing-space marker. CONNECT passes three bidirectional 2 MiB transfers and
three zero-payload denials with six sealed accounting entries. These results use
the retained original reader and host inputs; a rebuilt reader needs new
qualification.

The [ARM64 guide](windows-arm64.md) describes the separate firmware path and
historical emulated results. Native ARM64 installation, input, security and
network/restore qualification remain open. Cross-host migration and
firmware-stage snapshots are also unvalidated.

The completed default and Windows measurements retain the failed footprint
comparisons against the unchanged thresholds, as described above. They do not
qualify other supported hosts or erase the measured costs. Egress passes and
process/snapshot consistency do not establish power-loss durability, a latency
bound or production readiness. Tokoloshe's Windows job runtime is outside this
guest-platform and egress scope.
