# Rust QXL guest drivers

This separate GPL-2.0-only component ports the SPICE XDDM display DLL
(`qxldd.dll`) and VideoPort miniport (`qxl.sys`) to Rust for Windows XP SP3 x86
and Windows 7 SP1 x64. It is outside Firecracker's Apache-2.0 workspace.
The drivers have no third-party Rust dependencies, DDK, SDK, C compiler, CRT
or global allocator. They import existing Windows kernel services directly.

Delivery is unsigned. Signing, catalogue generation, certificate enrolment and
persistent boot-policy changes are excluded. Later Windows releases need their
own supported display driver; this INF does not select the XDDM package there.

## Source and licence

The source is [SPICE QXL XDDM](https://gitlab.com/spice/win32/qxl), commit
`e3371bd2d26473dd42798ae392e0c4cdc6a63a06`. Its archive SHA-256 is
`65f5a5536981767f743d46453aad6493cd81f07e043650807d4f3ba7324556e7`.
Keep the upstream copyright notices and [COPYING](COPYING). Translation retains
the original licence; do not link this crate into Firecracker.

The ring implementation follows `spice/ipc_ring.h` at spice-protocol commit
`d0cfb3648cc1635df4d14facf2fdda0c40876619` (v0.12.14). Its BSD notice is retained
in [NOTICE](NOTICE). Windows ABI declarations follow Microsoft's
[VideoPort](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/video/)
and [GDI DDI](https://learn.microsoft.com/en-us/windows/win32/api/winddi/)
references. Native sizes and offsets are checked for both targets.

## Build the drivers

Use the repository's pinned Rust toolchain. From the repository root:

```sh
rustup target add i686-pc-windows-msvc x86_64-pc-windows-msvc
export CARGO_TARGET_DIR="$PWD/build/windows/qxl-driver/rust-target"
cd tools/windows/display/qxl
cargo build --locked --release --features miniport,display --bins --target i686-pc-windows-msvc
cargo build --locked --release --features miniport,display --bins --target x86_64-pc-windows-msvc
for profile in xp win7; do
  case "$profile" in
    xp) target=i686-pc-windows-msvc ;;
    win7) target=x86_64-pc-windows-msvc ;;
  esac
  package="$CARGO_TARGET_DIR/package-$profile"
  mkdir -p "$package"
  cp "$CARGO_TARGET_DIR/$target/release/qxl-miniport.exe" "$package/qxl.sys"
  cp "$CARGO_TARGET_DIR/$target/release/qxl-display.exe" "$package/qxldd.dll"
  cp package/qxl.inf COPYING NOTICE "$package/"
done
```

Run the builds from the component directory so Cargo reads its linker settings.
They select Rust's bundled LLD; `build.rs` sets the native-driver entry point,
checksum and OS floors (XP 5.1, Windows 7 6.1). No Visual Studio, SDK, DDK,
C compiler, CRT or global allocator is used. Cargo's `.exe` output is a native
driver PE image; the copy commands give it the installation filename suffix.

Both target settings apply LLVM's `noimplicitfloat` attribute, preventing
implicit SIMD use in kernel callbacks without changing Rust's ABI.
The driver uses integer operations only. Explicit floating-point or vector
operations require a separate kernel-state design; see [SAFETY.md](SAFETY.md).

`driver/display_memory.rs` supplies the display DLL's memory-operation ABI in
Rust. Aligned word copies preserve padding through `MaybeUninit`; unaligned
heads and tails use byte accesses. Copy direction preserves overlap. Volatile
access prevents the compiler from replacing these helpers with recursive calls.
No SIMD register state is used.

`driver/memory.rs` supplies the compiler's memory-operation ABI in Rust.
Copying and zeroing use VideoPort's existing kernel APIs; nonzero filling uses
volatile stores so the compiler cannot introduce a recursive memset call.
`driver/miniport.rs` follows Microsoft's
[Rust kernel panic convention](https://github.com/microsoft/windows-drivers-rs/blob/main/crates/wdk-panic/src/lib.rs).

## Install a development build

Use a disposable image and retain its previous driver package. In Device
Manager, update the QXL display adapter, choose the local driver directory and
select `qxl.inf` with **Have Disk**. XP x86 and Windows 7 x64 use different
binaries; keep each architecture in its own directory. The INF excludes later
Windows versions, which do not support this XDDM driver.

The package is unsigned. XP displays an unsigned-driver warning. Windows 7 x64
requires a driver-signature-enforcement override to load an unsigned miniport.
For a disposable test VM, select **Disable Driver Signature Enforcement** from
the F8 advanced boot menu for each boot that needs the driver. The build and INF
do not enrol certificates or change boot policy. Driver signing and signed
catalogue generation are excluded from this delivery.

## Development checks

From the repository root, using its pinned Rust toolchain:

```sh
cargo test --locked --manifest-path tools/windows/display/qxl/Cargo.toml
cargo clippy --locked --manifest-path tools/windows/display/qxl/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path tools/windows/display/qxl/Cargo.toml --check
```

For native-target checks, use the component directory and target names from the
build procedure. Add `--features miniport,display --bins` to target Clippy.
Do not add `wdk-build`, bindgen, SDK import libraries or a C compilation step.
The cursor regressions compile the actual pointer callbacks with a mock GDI
endpoint and the real allocator and command ring. They force queue-full and
allocation failures after accepting a shape or movement, then check callback
success, notification, published bytes and resource cleanup. The original buggy
callback fails both failure cases.
User-mode acceptance probes use the separate existing
[compatibility-library procedure](../../README.md#windows-probe-linking);
VC-LTL and YY-Thunks are not kernel-driver dependencies.

## Implementation map

| Upstream source | Rust modules and responsibility |
| --- | --- |
| `miniport/qxl.c` | `resources`, `mode_pool`, `modes`, `adapter`, `miniport`: PCI discovery, BAR/event ownership, mode selection, ISR/DPC, power and IOCTL callbacks. |
| `include/qxl_driver.h` | `driver_info`, `ioctl`, `abi`: shared version-3 records, request codes and native encoding without copying padding. |
| `display/driver.c` | `pdev_config`, `pdev`, `display_modes`, `cosmetic`: GDI capabilities, stable PDevs, mode enumeration/assertion, escapes and line styles. `driver/display.rs` registers the DDI table. |
| `display/mspace.c`, `res.c` | `arena`, `slots`, `ring`, `display_resources`: fixed-region allocation, generation-tagged addresses, queues and retained resources. |
| `display/res.c` | `bitmap`, `image`, `image_cache`, `palette`: validated pixels, immutable images, hash/LRU cache and palette ownership. |
| `display/quic*.c` | `quic`: all five QUIC formats, predictor, adaptive Golomb models, sampling windows and MEL run coding. |
| `display/surface.c` | `surface`, `gdi`, `native_bitmap`, `secondary`: packed surface commands, GDI binding, secondary callbacks and video/RAM migration. |
| `display/rop.c` | `rop`, `draw`, `raster_draw`, `render`: ROP selection, retained drawables, copies, masked operations, scaling, alpha and colour keys. |
| `brush.c`, `text.c`, `pointer.c` | `brush`, `paint`, `text`, `path`, `cursor`, `pointer`, `native_*`: brushes, glyph/path capture, strokes and cursor commands. |
| `include/murmur_hash2a.h` | `hash`: Murmur2A and its Jump3 pixel variant. |
| Diagnostic/memory helpers | Rust `core::fmt` and `driver/{memory,display_memory}.rs`; no C runtime. The original Windows 2000 helper is not needed on XP/7. |

## Rendering and host contract

The current Firecracker QXL device advertises one surface, 32-bit primary modes
and compression level zero. The driver respects those capabilities. It declines
secondary video allocation, so GDI retains normal system-memory bitmaps. QUIC
is selected only when the host enables it and the compressed image fits a
smaller allocation than the raw image. Otherwise the raw path remains available.
Do not advertise compression or additional surfaces without endpoint support.

Primary drawing submits supported QXL commands. Other cases use synchronised
kernel-GDI bitmap views. Repeated references to one handle use one view, which
preserves overlapping copies. CPU destination drawing relies on Firecracker's
shared framebuffer; it is not a generic substitute for hosts with separate
rendered surfaces. Opaque handles must belong to a retained primary or secondary
bitmap before driver context is accessed.

ROP selection covers all 256 ROP3 truth tables and 16 ROP2 operations. Duplicate
hints preserve the pixel result, including reverting inversion. Masked commands
use the conservative blend effect. Native paths capture defined pixel, glyph,
brush and path bytes before publication. Unsupported accelerated operands and
colour transforms retain the GDI path.

Indexed copies accept 1-, 4- and 8-bit source DIBs, including signed strides and
unaligned bit/nibble crops. Capture copies the documented
[XLATEOBJ table](https://learn.microsoft.com/en-us/windows/win32/api/winddi/ns-winddi-xlateobj)
before acquiring the hardware lease. No GDI translation pointer survives.
The palette LRU retains up to 64 entries; identity zero is never cached and
identity matches also compare the complete colour table.

The raw image cache uses 32,768 hash/LRU entries in GDI heap memory and a payload
budget of 1.5 times the primary framebuffer. Hits compare dimensions, format,
palette and all defined pixels. Allocation pressure evicts old entries. Wrong
pools and stale slot generations are rejected. Compressed images bypass this
cache, avoiding a second retained raw copy solely for cache comparison.

## Ownership and failure behaviour

Read [SAFETY.md](SAFETY.md) before changing an unsafe boundary. A driver-wide GDI
semaphore serialises PDev, registry, hardware and mode operations; recursive
hardware entry is rejected. A stable PDev owns its primary attachment, secondary
registry and configuration palette. The mapping remains owned through every
session and host reference.

Each output retains its immutable inputs until host release. Evicting a cache
entry cannot invalidate a pending draw. Release IDs are opaque monotonic values,
never guest pointers. Malformed, cyclic or replayed release chains cannot free
an allocation at a reused address. Submission failure returns ownership for
retry or cancellation. Honour notification flags on failure as well as success.

Secondary callbacks validate the host limit, registry setting, video slot and
pixel format. Before shutdown they synchronise video pixels, switch GDI to RAM
and retain the old video allocation. Completed stop/reset permits detachment.
Resume creates the new surface with KEEP_DATA before switching GDI back. A failed
restore retains RAM and any candidate that could not yet be destroyed. GDI-owned
handles are not deleted twice; their contexts survive until deletion callbacks.

Normal shutdown destroys host surfaces, drains releases, clears caches and
removes the video slot. Failure retains the session and mappings. A timeout
latches the control channel failed; it never proves host quiescence. Unmapping
requires stopped hardware and an empty pool. Primary surface disable does not
perform hardware shutdown; that belongs to mode assertion.

## Qualification

Unit tests cover ABI/layout bounds, allocation and release ownership, malformed
queues, cache collisions, all ROP combinations and codec streams. The 25 QUIC
golden cases come from the original codec at the pinned source commit. They
check exact encoded lengths and hashes, then decode and compare defined bits.
Cases include all five formats, one-pixel rows, noise, gradients and runs across
2,048-symbol model windows. The reference C executable is not a Rust build or
test dependency.

Use the existing [mode and drawing probes](../README.md) for native acceptance.
A successful build or probe exit alone does not establish pixel correctness.
Capture the console and compare defined BGR channels with the same OS's GDI
software oracle. Retain the actual binary hashes, raw mismatch counts, installed
device status and shutdown result. Do not relabel an earlier receipt after a
build changes. Repeat a completed mode matrix only when a change affects it.

Both Rust drivers have passed the eighteen-mode frame, pointer and printable
text checks on XP and Windows 7, restoring the original mode and shutting down
cleanly. Those receipts identify their earlier binaries. Later drawing checks
cover binary/ternary ROPs, masks, scaling, patterns, glyphs, paths, colour keys,
alpha, indexed formats and palette changes. The frozen XP pair also passes
compatible-bitmap creation, overlap, screen/bitmap round trips and deletion:
all 122,880 BGR pixels match GDI and QXL reports device error code zero.
The frozen Windows 7 pair passes the same new cases, with only the five
alpha-rounding differences below. Both unsigned INF installations succeeded
and required a reboot; both frozen pairs report device error code zero and
shut down cleanly. The XP discard-order correction was copied after its INF
installation and before its frozen-pair drawing check.

The final cursor-notification builds also passed focused native checks on both
OSes: five pointer positions, arrow/text cursor changes, Windows-key Run, exact
stored keyboard input, device error code zero and clean S5 shutdown. These
checks do not inject cursor publication failures.

Preserve the five measured Windows 7 alpha differences separately. At opaque
source pixels with constant alpha 128, XP rounds the two source-over terms
separately, while Windows 7 rounds their sum; five red channels differ by one.
The host matches XP. The
[BLENDFUNCTION contract](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-blendfunction)
does not specify intermediate byte rounding. Do not introduce a general pixel
tolerance or select host arithmetic from the guest OS.

Native tests on a single-surface, compression-disabled host do not qualify
secondary video allocation/migration or compressed transmission. Failure and
power paths also require their own targeted evidence; normal shutdown does not
qualify them. Keep these limits explicit in review and release notes.
