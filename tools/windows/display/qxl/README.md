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

`src/ring.rs` ports the SPICE command/cursor producer and release consumer
operations. Packed entries use byte access; counters use aligned volatile
access. The original notification arm/barrier/recheck sequence is retained.
Its source is `spice/ipc_ring.h` at spice-protocol commit
`d0cfb3648cc1635df4d14facf2fdda0c40876619` (v0.12.14); the upstream BSD notice
is retained in [NOTICE](NOTICE).

## Development checks

From the repository root, with its pinned Rust toolchain:

```sh
cargo test --manifest-path tools/windows/display/qxl/Cargo.toml
cargo clippy --manifest-path tools/windows/display/qxl/Cargo.toml --all-targets -- -D warnings
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
| `miniport/qxl.c` | Mode state and slot encoding ported; discovery, mappings, events, interrupts, power and IOCTL entry points remain. |
| `display/driver.c` | Cosmetic styles and slot encoding ported; GDI device lifecycle, escapes and path submission remain. |
| `display/res.c` | Slot conversion and ring endpoints ported; resource ownership, caches and command payloads remain. |
| `display/surface.c` | Surface allocation, upload, synchronisation and destruction remain. |
| `display/rop.c`, `brush.c`, `text.c`, `pointer.c` | Raster operations, brush realisation, text and cursor operations remain. |
| `display/quic*.c` | Compression remains. |
| `display/mspace.c` | Shared-memory allocator remains. |
| `include/murmur_hash2a.h` | Cache hashing remains. |
| `miniport/minimal_snprintf.c` | Replace diagnostic formatting with `core::fmt`. |
| `miniport/wdmhelper.c` | Contains only Windows 2000 compatibility code; XP/7 use VideoPort event APIs. |

The build must use Rust ABI declarations, raw DLL imports and Rust's linker.
Do not add `wdk-build`, bindgen, SDK import libraries or a C compilation step.
