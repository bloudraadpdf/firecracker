# Rust QXL guest drivers

This separate GPLv2 component ports the SPICE XDDM display driver (`qxldd.dll`)
and video miniport (`qxl.sys`) to Rust for Windows XP x86 and Windows 7 x64.
It is outside the Firecracker Apache-2.0 workspace. It has no DDK, C compiler,
CRT or third-party Rust dependencies.

The port is in progress. The current crate contains cosmetic-line conversion
and miniport mode handling; it does not yet produce an installable driver. The
installed vendor binaries remain validation references, not the final
implementation.

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
| `miniport/qxl.c` | Release-path callbacks, request dispatch, discovery, reset, interrupts, power and mode handling ported; SYS build and native qualification remain. |
| `display/driver.c` | Cosmetic styles and slot encoding ported; GDI device lifecycle, escapes and path submission remain. |
| `display/res.c` | Slot conversion and ring endpoints ported; resource ownership, caches and command payloads remain. |
| `display/surface.c` | Surface allocation, upload, synchronisation and destruction remain. |
| `display/rop.c`, `brush.c`, `text.c`, `pointer.c` | Raster operations, brush realisation, text and cursor operations remain. |
| `display/quic*.c` | Compression remains. |
| `display/mspace.c` | Shared-memory allocator remains. |
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
remain pending until the entry points are connected.

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
There is no loadable SYS artifact or native callback qualification yet.

Failed discovery releases its resource claim after dropping mappings and
events, as required by
[VideoPortGetAccessRanges](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/video/nf-video-videoportgetaccessranges).
Successful FindAdapter setup commits that claim before publishing the adapter.
