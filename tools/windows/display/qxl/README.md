# Rust QXL guest drivers

This separate GPLv2 component ports the SPICE XDDM display driver (`qxldd.dll`)
and video miniport (`qxl.sys`) to Rust for Windows XP x86 and Windows 7 x64.
It is outside the Firecracker Apache-2.0 workspace. It has no DDK, C compiler,
CRT or third-party Rust dependencies.

The port is in progress. The current crate builds unsigned miniport images and
contains shared display-driver components. It does not yet produce an
installable driver package. The installed vendor binaries remain validation
references, not the final implementation.

## Source and licence

The source is [SPICE QXL XDDM](https://gitlab.com/spice/win32/qxl) commit
`e3371bd2d26473dd42798ae392e0c4cdc6a63a06`. Its archive SHA-256 is
`65f5a5536981767f743d46453aad6493cd81f07e043650807d4f3ba7324556e7`.
Retain upstream copyright notices and [COPYING](COPYING). A Rust translation
retains the source licence. Do not link this crate into Firecracker.

`src/cosmetic.rs` ports `GetCosmeticAttr` from `xddm/display/driver.c`.
Custom styles include the GDI density when converted to QXL Q28.4 pixels;
alternate styles retain single-pixel marks and gaps. Invalid lengths, counts
and overflow return an error before a command can be submitted.

`src/modes.rs` ports mode conversion and the mode-selection state from
`xddm/miniport/qxl.c`. It reads packed ROM mode records with checked bounds,
preserves mode IDs and orientation, alternates the two custom-mode IDs, and
defines the documented
[VIDEO_MODE_INFORMATION ABI](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddvdeo/ns-ntddvdeo-_video_mode_information)
directly in Rust. No SDK headers or libraries are used.
Requests validate the aligned framebuffer size before they change mode state.

`src/slots.rs` ports the shared miniport/display slot address calculation from
`CreateMemSlots`, `CreateVRamSlot`, `PA` and `VA`. A slot validates its physical
and virtual ranges, ID and generation. Address conversion checks the complete
buffer and rejects stale generations before use.

`src/driver_info.rs` defines the version-3 display/miniport interface from
`include/qxl_driver.h`: 264 bytes on x86 and 440 bytes on x64. Encoding writes
each field and clears padding, rather than copying kernel stack padding into
the returned record. Mode records also use explicit field encoding. Short
output buffers remain unchanged.

`src/ring.rs` ports the SPICE command/cursor producer and release consumer
operations. Packed entries use byte access; counters use aligned volatile
access. The original notification arm/barrier/recheck sequence is retained.
Its source is `spice/ipc_ring.h` at spice-protocol commit
`d0cfb3648cc1635df4d14facf2fdda0c40876619` (v0.12.14); the upstream BSD notice
is retained in [NOTICE](NOTICE).

## Development checks

From the repository root, with its pinned Rust toolchain:

```sh
cargo test --locked --manifest-path tools/windows/display/qxl/Cargo.toml
cargo clippy --locked --manifest-path tools/windows/display/qxl/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path tools/windows/display/qxl/Cargo.toml --check
```

## Completion checklist

- [x] Separate licence boundary and checked cosmetic-style conversion.
- [ ] Shared QXL protocol, Windows ABI and Rust-only PE build for both targets.
- [ ] Miniport discovery, BAR mappings, modes, interrupts, power and IOCTLs.
- [ ] Display lifecycle, surfaces, resource allocation and release rings.
- [ ] Drawing, brushes, text, paths, raster operations and cursors.
- [ ] Complete INF/package and explicit signing procedure.
- [ ] Install both Rust binaries on XP and 7; compare native GDI output,
      resolution changes, input and lifecycle against the existing probes.

Existing successful vendor-driver checks do not qualify the Rust replacements.

The port covers the full XP/7 driver package. The remaining source mapping is:

| Upstream source | Rust replacement |
| --- | --- |
| `miniport/qxl.c` | Release-path callbacks and SYS builds ported; native load, lifecycle and request qualification remain. |
| `display/driver.c` | Cosmetic styles, slots, mode enumeration, PDev setup and hardware sessions ported; surface callbacks, escapes and rendering remain. |
| `display/res.c` | Slot conversion, rings and output/resource ownership ported; caches and drawing payloads remain. |
| `display/surface.c` | Surface allocation and create/destroy ownership ported; GDI bitmap binding, synchronisation and RAM migration remain. |
| `display/rop.c` | ROP2/ROP3 selection ported in `src/rop.rs`; drawing submission remains. |
| `display/brush.c`, `text.c`, `pointer.c` | Cursor shapes and commands ported; native pointer callbacks, brush realisation and text remain. |
| `display/quic*.c` | Compression remains. |
| `display/mspace.c` | Fixed-region allocation and release replaced by `src/arena.rs`; display resource integration remains. |
| `include/murmur_hash2a.h` | Ported in `src/hash.rs`, including the Jump3 pixel variant. |
| `miniport/minimal_snprintf.c` | Replace diagnostic formatting with `core::fmt`. |
| `miniport/wdmhelper.c` | Contains only Windows 2000 compatibility code; XP/7 use VideoPort event APIs. |

The build must use Rust ABI declarations, raw DLL imports and Rust's linker.
Do not add `wdk-build`, bindgen, SDK import libraries or a C compilation step.

`src/videoport.rs` declares the Windows VideoPort interface directly in Rust,
with `extern "system"` and raw imports from `videoprt.sys`. It checks pointer
layout for both Windows architectures at compile time. The declarations use
Microsoft's [VideoPort DDI reference](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/video/),
including
[initialisation data](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/video/ns-video-_video_hw_initialization_data),
[configuration](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/video/ns-video-_video_port_config_info),
[access ranges](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/video/ns-video-_video_access_range)
and [request packets](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/video/ns-video-_video_request_packet).
The configuration declaration contains only the stable prefix the driver reads.
ABI declarations and compile checks do not prove that a driver loads or runs.
The [safety contract](SAFETY.md) defines the ABI, mapping and pointer obligations.

`src/resources.rs` ports PCI discovery and the mapping/event setup from
`miniport/qxl.c`. Successful VideoPort allocations have owners that release
them if a later setup step fails. A short mapping is rejected and unmapped.
The initialisation path checks PCI identity, revision, interrupt configuration,
memory and I/O range types, ROM/RAM signatures, and mode-table bounds.

`src/memory.rs` validates the immutable ROM prefix before RAM access. It checks
page arithmetic, slot IDs, the primary allocation and the complete shared RAM
header. The resource layer reads device bytes with volatile access; it does not
create ordinary Rust slices over mutable device memory. These paths compile
for both Windows targets; native failure cleanup and driver lifecycle tests
remain pending.

`src/mode_pool.rs` copies ROM mode records into a VideoPort paged-pool
allocation, then uses the same `ModeSet` implementation as the domain tests.
The allocation has one owner; its mode-state borrow ends before it is freed.
The public methods return mode copies or iterators tied to the owner. No
global allocator or C runtime is required.

`src/adapter.rs` implements reset and memory-slot registration, ISR/DPC event
handling, the display interface and miniport IOCTL dispatch. Requests consume
input before writing output so a shared input/output buffer is supported.
Failed slot registration leaves interrupts masked.

`src/miniport.rs` registers the actual VideoPort callbacks through
`miniport::driver_entry`. It publishes the adapter only after setup succeeds,
keeps paged mode state out of interrupt handling, and completes each request
through its status block. `SurfacesEnabled=0` disables non-primary surfaces.
The SYS images link for both targets; native callback qualification is pending.

Failed discovery releases its resource claim after dropping mappings and
events, as required by
[VideoPortGetAccessRanges](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/video/nf-video-videoportgetaccessranges).
Successful FindAdapter setup commits that claim before publishing the adapter.

## Build the miniport

Use the repository's pinned Rust toolchain. From the repository root:

```sh
rustup target add i686-pc-windows-msvc x86_64-pc-windows-msvc
export CARGO_TARGET_DIR="$PWD/build/windows/qxl-driver/rust-target"
cd tools/windows/display/qxl
cargo build --locked --release --features miniport --bin qxl-miniport --target i686-pc-windows-msvc
cargo build --locked --release --features miniport --bin qxl-miniport --target x86_64-pc-windows-msvc
cp "$CARGO_TARGET_DIR/i686-pc-windows-msvc/release/qxl-miniport.exe" "$CARGO_TARGET_DIR/qxl-xp.sys"
cp "$CARGO_TARGET_DIR/x86_64-pc-windows-msvc/release/qxl-miniport.exe" "$CARGO_TARGET_DIR/qxl-win7.sys"
```

Run the builds from the component directory so Cargo reads its linker settings.
They select Rust's bundled LLD; `build.rs` sets the native-driver entry point,
checksum and OS floors (XP 5.1, Windows 7 6.1). No Visual Studio, SDK, DDK,
C compiler, CRT or global allocator is used. Cargo's `.exe` output is a native
driver PE image; the copy commands give it the installation filename suffix.

The x86 settings also apply LLVM's `noimplicitfloat` attribute, preventing
implicit SIMD use in kernel interrupt handlers without changing Rust's ABI.
The driver uses integer operations only. Explicit floating-point or vector
operations require a separate kernel-state design; see [SAFETY.md](SAFETY.md).

`driver/memory.rs` supplies the compiler's memory-operation ABI in Rust.
Copying and zeroing use VideoPort's existing kernel APIs; nonzero filling uses
volatile stores so the compiler cannot introduce a recursive memset call.
`driver/miniport.rs` follows Microsoft's
[Rust kernel panic convention](https://github.com/microsoft/windows-drivers-rs/blob/main/crates/wdk-panic/src/lib.rs).

The x86 and x64 images pass static checks for machine type, native subsystem,
OS floor, relocations and PE checksum. They import only `videoprt.sys` and
`ntoskrnl.exe`. Every imported VideoPort function is present in the XP and
Windows 7 guest copies of that module. These are unsigned development images;
they have not been installed or executed. The Rust display DLL, package,
signing procedure and native qualification remain incomplete.

## Display memory

`src/arena.rs` replaces the fixed-region allocation used by `display/res.c`.
It manages the supplied BAR range without an OS heap or global allocator.
Eight-byte-aligned blocks use first fit; release coalesces both adjacent free
blocks. Free-list headers occupy free memory only. Live allocation tokens
retain the allocated extent and cannot be copied. A release to another arena
returns the token without changing either arena.

Keep the display allocation semaphore held for allocation and release. A token
must remain live until all guest and host users have finished; dropping it does
not free device memory. Mapping reset and destruction require the release of
all outstanding resources. The original driver uses only fixed-region malloc
and free; unused general-purpose dlmalloc APIs are not imported or reproduced.

## Raster operations

`src/rop.rs` selects QXL methods for all 256 ROP3 truth tables and the 16 GDI
ROP2 operation numbers. It derives required operands from the truth table,
then chooses the original specialised command or a ternary command. It does
not maintain a second 256-entry classification table. Tests evaluate every
input combination independently of the selector.

Duplicate-command hints must preserve the pixel result. The original ROP3
table incorrectly marks destination inversion as idempotent; the Rust selector
marks it as reverting on repetition. ROP2 negated-AND cases use the conservative
blend hint, matching the original ROP3 table, rather than its unsafe binary
blackness hint. Tests check these hints against repeated application for every
input. Masked commands use the blend effect. GDI callbacks and command
submission are still required before these operations run in Windows.

## GDI mode enumeration

`src/engine.rs` implements the `DrvGetModes` callback and owns its temporary
kernel GDI mode buffer. It queries the miniport's count and records, validates
returned lengths and checked allocation sizes, and frees the buffer on every
exit. Iteration returns mode copies, including records with extended strides.
Both driver halves use request codes from `src/ioctl.rs`.

`src/display_modes.rs` filters supported graphical modes and encodes complete
220-byte `DEVMODEW` records with explicit field writes. Size queries count the
same supported records that output queries return. Short output buffers are
unchanged. The encoder preserves depth, frequency and rotation and clears all
unused fields. Its layout also matches the existing native display probe.

The engine imports kernel functions directly from `win32k.sys`, following
[EngAllocMem](https://learn.microsoft.com/en-us/windows/win32/api/winddi/nf-winddi-engallocmem)
and [EngDeviceIoControl](https://learn.microsoft.com/en-us/windows/win32/api/winddi/nf-winddi-engdeviceiocontrol).
It does not use the user-mode `gdi32.dll` exports. Callback registration and
native mode-enumeration qualification require the complete display DLL.

## Output and resource ownership

`src/display_resources.rs` connects the arenas, address conversion and real QXL
rings. Main-memory resources store their header and payload in one allocation;
video-memory resources retain a main-memory header and a video allocation.
Explicit resource references let a cache and several commands share a payload.
The final reference releases both allocations.

An output owns up to six resource references, matching the original driver.
Its builder can be filled, submitted, retried or cancelled. A failed submission
returns the builder with its references intact. Successful publication consumes
the builder and registers it as pending before advancing the command ring.
The ring entry carries the encoded payload address, command type and zero flags.

Release IDs are monotonic opaque integers. Host IDs are looked up among live
pending outputs; they are never cast to guest pointers. The release consumer
handles linked completions with a bounded work budget and retains unfinished
chains for the next call. Unknown IDs, self-links, cycles and replayed old IDs
cannot free a new allocation at a reused address. Cache references survive
command completion until explicitly released.

Tokens require explicit release or cancellation. Dropping a token does not
free memory that the host might still use. Keep the allocation semaphore held
for pool operations, and act on returned ring-notification flags. Pending
outputs may be discarded only after the device has stopped all access; a
timeout does not establish that condition. Normal teardown also releases
builders and cache references, then requires `Pool::is_empty()` before unmap.

Integration tests use the actual command and release rings to check publication,
shared resource lifetimes, bounded release chains, cancellation, allocation
failure, malformed completions and reuse of storage. These memory paths also
pass Miri. Native PDev lifecycle and drawing callbacks still need to connect
the pool to the Windows driver.

## Surface lifecycle

`src/surface.rs` allocates checked RGB555 and xRGB8888 secondary surfaces and
publishes their create/destroy commands through the output pool. Layouts use
four-byte-aligned positive storage strides and bottom-up memory. QXL records
carry the negative stride. Both the packed 41-byte surface command and 40-byte
primary-create record use explicit field encoding.

A prepared surface owns an unpublished allocation and permits a safe mutable
pixel slice. Publication consumes it and returns an active surface, which
exposes only a raw pixel pointer. A failed publication returns the prepared
surface with pixels intact for retry or cancellation. `keep_data` preserves
pixels supplied before creation, including data restored from system memory.

Creation and destruction each retain a command reference to the allocation.
Completion of creation leaves the active surface reference live; publication
of destruction transfers that reference to the destroy command. Video memory
stays live until all command references are released, including destruction.
A full ring returns the active surface so destruction can be retried.
Device-stop cleanup requires proof that the host no longer accesses the surface.

The PDev surface table must reserve IDs within the advertised device limit and
preserve command ordering when reusing them. GDI bitmap binding, CPU/hardware
synchronisation and migration between video and system memory are still needed
for the complete Windows callbacks. Five focused tests cover the current
layout, wire-format and ownership paths; all pass Miri using the shared ring
fixtures from `display_resources/test_utils.rs`.

## Kernel GDI bitmaps

`src/gdi.rs` declares the native `SIZEL` and `SURFOBJ` layouts and owns GDI
bitmap handles. A software draw area creates and associates a bottom-up bitmap
over the surface's existing pixel allocation, then locks its surface object.
Construction failures delete the bitmap. Destruction unlocks the object before
deleting its handle. A draw area created from an active surface borrows that
owner, preventing surface destruction while the draw area remains live.

Device bitmaps use an initialised, stable driver surface context and registered
hooks. Their construction guard deletes the handle on failure; `into_raw`
transfers successful ownership to GDI. The driver must keep that context live
until GDI calls its deletion callback. The primary pixel mapping uses the same
software-bitmap owner through its internal raw constructor, whose caller must
prove the complete pixel extent and lifetime.

These functions import from `win32k.sys`, using Microsoft's
[bitmap creation](https://learn.microsoft.com/en-us/windows/win32/api/winddi/nf-winddi-engcreatebitmap)
and [surface locking](https://learn.microsoft.com/en-us/windows/win32/api/winddi/nf-winddi-englocksurface)
contracts. `SURFOBJ` is checked against the actual `winddi.h` layout: its private
and public device-handle names are alternatives, not additional fields.
Native PDev callbacks and Windows execution must still qualify these owners.

## PDev configuration

`src/pdev_config.rs` selects an exact supported mode from the requested
`DEVMODEW` dimensions, depth, frequency and rotation. It retains the miniport's
stride, colour masks and mode index, and initialises the original GDI defaults,
fonts, colour calibration and cursor capabilities for the complete driver.
Malformed input and unmatched modes fail before configuration is returned.
These capability records must not be published until their callbacks exist.

The native `GDIINFO`, `DEVINFO` and font layouts use the same field-by-field
encoding as the display/miniport interface. `src/abi.rs` contains that shared
encoder; arrays and nested records preserve native offsets while clearing
padding. Constructors initialise valid field values rather than relying on
struct padding. The native PDev retains a `gdi::Palette` for its bitfield colour
masks and places that live handle in `DEVINFO`. The palette owner deletes its
GDI handle on destruction. Callback lifecycle integration is still required
before these records can be returned to Windows.

`PreparedDevice::new` runs the native mode query, selection and palette creation.
It drops the temporary mode buffer before creating the palette and owns that
palette through device teardown. `device_info()` supplies the owner's live
palette handle when the callback encodes its output. Preparation does not
activate hardware or register an incomplete callback table.

## Mapped miniport interface

`src/display_device.rs` checks the version-3 miniport interface before creating
display resources. Primary pixels and I/O pages must fit the main memory slot,
remain disjoint, and agree with the reported physical addresses. Rings and
control records use the separate shared RAM header layout; they are not part
of the allocatable slot. ROM pointers and event handles retain the miniport's
mapping and lifetime contract.

Native mapping selects the configured mode, queries the interface and requests
the complete video BAR. Once an address is returned, its guard pairs the map
with an unmap request on failure or teardown. Native PDev code must retain it
through all GDI, pool and host access, and stop those users before destruction.
Validation checks reported geometry; it cannot prove that a kernel address is
mapped. Focused tests cover the geometry and rejection paths; native
mapping and teardown still require Windows qualification.

The miniport map response uses the same native record encoder as interface
queries. Pointer fields retain their provenance, padding is cleared, and
short response buffers remain unchanged.

## Hardware control commands

`src/control.rs` ports the serialised control-port path from `qxldd.h` and
`driver.c`. One channel belongs to each physical adapter and must be shared
across its PDevs. Exclusive access covers shared-header writes, stale-event
clear, port issue and completion. Revision checks select the original legacy
or asynchronous port and reject unsupported operations before touching memory.

The native wait uses
[KeWaitForSingleObject](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/wdm/nf-wdm-kewaitforsingleobject)
with a relative 60-second timeout. Only `STATUS_SUCCESS` proves completion;
timeouts and other results latch the channel failed and prevent further writes.
This avoids the ambiguous Boolean returned by
[EngWaitForSingleObject](https://learn.microsoft.com/en-us/windows/win32/api/winddi/nf-winddi-engwaitforsingleobject).
It uses a raw Windows kernel import, with no SDK library or new dependency.
Failure does not prove the host has stopped: retain all command resources and
mappings until explicit hardware quiescence. Native PDev integration and
qualification remain part of the unfinished display DLL.

## Cursor shapes and commands

`src/cursor.rs` ports the cursor payloads from `pointer.c` and `res.c`: alpha,
monochrome, indexed and direct-colour shapes, transparent shapes, position,
visibility and trails. Bitmap views check row bounds, retain signed strides
and omit GDI row padding. Colour shapes include the palette and AND mask in
protocol order. Each shape uses one checked, contiguous QXL data chunk.

Shapes own a pool resource. A cursor-set output retains a separate reference
until host release; cache eviction or a failed submission cannot free that
reference. Failed publication cancels only its unpublished output. Explicit
shape release ends the cache reference. This component does not register native
callbacks; PDev integration, cache lookup and native bitmap readback remain
part of the full display-driver port.

## Native hardware session

`src/hardware.rs` connects a mapped adapter to its command, cursor and release
rings and resource pool. The session borrows the mapping, owns the guest ring
endpoints and creates one control channel. Activation registers the video slot,
reads its acknowledged generation and creates the primary surface. One adapter
owner must serialise this session across all PDevs and power/mode callbacks.

Normal shutdown destroys host surfaces, flushes or resets the device as required
by its revision, drains releases and removes the video slot. A failed operation
retains the session and all mappings. Explicit reset can stop unknown in-flight
work; it invalidates the old ring and slot generation. A stopped session cannot
be restarted. Release all retained surface/cache tokens before unmapping.

This session is the native hardware part of the unfinished PDev integration.
It does not register GDI callbacks, migrate live GDI bitmaps to system RAM or
replace the final Windows qualification. Those remain required before shipping.

## Native PDev ownership

`src/pdev.rs` implements the
[configuration](https://learn.microsoft.com/en-us/windows/win32/api/winddi/nf-winddi-drvenablepdev),
[completion](https://learn.microsoft.com/en-us/windows/win32/api/winddi/nf-winddi-drvcompletepdev)
and [destruction](https://learn.microsoft.com/en-us/windows/win32/api/winddi/nf-winddi-drvdisablepdev) callbacks
for a stable, heap-owned PDev. Enable validates both output extents before
allocating, selects the requested mode and palette, and writes only known native
capability records. Completion stores the installed GDI handle. Destruction
drops the Rust value before freeing its original allocation.

Opaque callback handles refer to `src/kernel_object.rs` allocations. Alignment
is computed inside the supplied extent without losing pointer provenance; the
stored value stays at that address until destruction. Handle transfer and
recovery have one owner. Driver initialisation creates one shared GDI semaphore
for all PDevs; driver shutdown requires every PDev to have been destroyed.

These callbacks are not registered yet. Mode assertion, rendering state,
compression and the remaining callbacks must be complete before the full
DDI table is published. The PDev setup callbacks do not activate hardware.

## Native bitmap migration

`src/native_bitmap.rs` binds stable driver contexts to GDI bitmaps. Primary
bitmaps retain their context until the owned GDI handle is deleted; they do
not expose `EngModifySurface` transitions, which Windows forbids after the
primary surface is published.

Secondary device bitmaps own their video resource and private GDI draw area.
RAM migration copies synchronised pixels, switches GDI to a bottom-up system
buffer and retains the old video resource. Detaching that resource requires
completed hardware shutdown. The resulting system bitmap has no dependency
on the old mapping and can restore into a new generation.

Restoration publishes the copied video surface before switching GDI back. A
failed GDI switch leaves the RAM bitmap intact and retires the new host surface;
if retirement cannot be queued, the error retains that candidate explicitly.
GDI-owned device handles are not deleted a second time by their driver callback.
Full PDev callback attachment and native migration qualification remain required.

Primary attachment retains the miniport handle in the PDev, then owns its stable
mapped-device allocation and hardware session together. It creates the GDI
bitmap before activating hardware. A failed activation retains the session;
it does not unmap device memory. The driver-wide GDI lock also rejects recursive
hardware entry. One PDev at a time can own a session for each miniport handle.

Hardware shutdown releases an empty, stopped session while retaining its GDI
bitmap. A replacement PDev can then own the adapter before the old bitmap is
deleted. Resume queries a fresh generation and retains it before activation.
Surface disable releases an attachment only after hardware shutdown and pool
cleanup. The primary handle is deleted before any retained session and mapping. Mode
assertion, rendering hooks and the final DDI table are still required before
these native attachment functions can be published as a display driver.

## Native raster callbacks

`src/render.rs` implements primary-surface copying, raster operations, text,
stroked paths, stretching and alpha blending, using kernel GDI's
[standard-bitmap operations](https://learn.microsoft.com/en-us/windows/win32/api/winddi/nf-winddi-engbitblt).
The PDev hardware lease covers synchronisation, temporary CPU views, drawing
and update publication. Repeated references to one primary use the same GDI
view, including overlapping source/destination copies. Source and mask views
remain read-only; the destination is updated even if GDI reports partial failure.

Standard GDI bitmaps pass through without conversion. Opaque surfaces must
match a retained primary handle before any driver context is accessed. Secondary
device-bitmap rendering and the remaining raster callbacks are still required;
these callbacks are not yet registered in a complete display DLL.

The native callbacks retain GDI's opaque font, path, transform, line-attribute,
brush and colour-adjustment objects only for their synchronous call. Stretch
ROP4 excludes unused source and mask operands before surface lookup. All current
engine imports are present in the tested XP and Windows 7 kernel export tables.
True-colour transparent drawing remains separate: `EngTransparentBlt` documents
only limited source formats, so its presence alone does not prove that path.

This CPU destination path uses Firecracker's shared primary framebuffer: frame
capture reads its guest-memory pixels directly. It is not a generic replacement
for QXL hosts that retain a separate rendered surface. Native Windows execution
and the accelerated command paths remain unfinished.
