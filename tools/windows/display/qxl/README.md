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
| `display/driver.c` | Cosmetic styles, slot encoding and mode enumeration ported; GDI device lifecycle, escapes and path submission remain. |
| `display/res.c` | Slot conversion, rings and output/resource ownership ported; caches and drawing payloads remain. |
| `display/surface.c` | Surface allocation, upload, synchronisation and destruction remain. |
| `display/rop.c` | ROP2/ROP3 selection ported in `src/rop.rs`; drawing submission remains. |
| `display/brush.c`, `text.c`, `pointer.c` | Brush realisation, text and cursor operations remain. |
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
