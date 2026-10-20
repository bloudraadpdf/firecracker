# Verify native Windows display modes

This user-mode acceptance probe exercises the existing QXL drivers on XP SP3
x86 and Windows 7 SP1 x64. Build Firecracker's `windows` capability and select
[the QXL console profile](../../../docs/windows-console.md#amd64), with its
matching SeaBIOS option ROM. Install the pinned virtio-win ISO's `qxl/xp/x86`
or `qxl/w7/amd64` package through normal Windows driver installation. Preserve
signature policy and record any warning or failure. Shut down and cold-boot the
private test disk before checking native graphics.

The mode sequence is 640x480, 800x600, 1024x768, 1280x1024, 1600x1200, 1152x864,
1280x768, 1280x800, 1280x960, 1440x900, 1400x1050, 1680x1050, 1920x1200,
1280x720, 1920x1080, 1600x900, 1366x768 and 2560x1440. These are source table
indices, not the sorted order shown in Windows settings. The maximum static
pixel buffer is 14,745,600 bytes. This probe uses normal Windows display APIs;
it does not supply a guest driver.

## Mode acceptance helper

The [probe](probe/probe.rs) checks all eighteen 32bpp QXL modes and emits a full
RGB pattern from [model.rs](probe/model.rs). It restores the original display
mode and cursor count on completion or failure. It uses the public XP DEVMODE
prefix, accepts dmSize 188..220, and makes no private display-driver calls.

For the executable, select the XP x86 or Windows 7 x64 profile and generate
`windows_rustflags` using the existing
[compatibility-library procedure](../README.md#windows-probe-linking). In the
same Bash session, run:

```bash
mkdir -p build/windows/display-probe
rustc --edition 2024 -D warnings "${windows_rustflags[@]}" \
  tools/windows/display/probe/probe.rs -o build/windows/display-probe/modes.exe
rustc --edition 2024 -D warnings tools/windows/display/probe/frame.rs \
  -o build/windows/display-probe/frame
rustc --edition 2024 -D warnings --test tools/windows/display/probe/probe.rs \
  -o build/windows/display-probe/tests
build/windows/display-probe/tests
```

The existing twenty host tests cover mode selection, public ABI bounds, RGB
patterns, pointer ordering, keyboard state, cleanup and callback re-entry. They
do not replace guest testing. The host frame utility uses the same pattern code
so that full-frame comparison needs no separate oracle implementation.

Use a disposable XP x86 or Windows 7 x64 reference with the existing QXL driver.
Select the matching compatibility-library build profile for each OS. Create a new
empty guest directory, copy `modes.exe` there, enable Caps Lock, and run
`modes.exe > modes.log` from that directory. It creates `mode-00.txt` through
`mode-17.txt` exclusively; existing files make the run fail.

For each case, within its ninety-second deadline:

1. Wait for `PAINT case=N phase=Pixels`. The cursor is hidden. Capture the whole
   display with the existing
   [console screenshot command](../console/README.md#unattended-capture-and-input).

1. Generate the expected PPM and compare every byte, replacing case index zero
   and filenames below for each case:

   ```sh
   build/windows/display-probe/frame 0 > build/windows/display-probe/expected-00.ppm
   cmp build/windows/display-probe/expected-00.ppm build/windows/display-probe/actual-00.ppm
   ```

1. Only after comparison passes, press and release F1. Move the pointer to the
   top-left, top-right, bottom-right, bottom-left and centre, in that order. Use
   pixel coordinates `(0,0)`, `(width-1,0)`, `(width-1,height-1)`,
   `(0,height-1)` and `(width/2,height/2)`. If already at a target, move away
   and return; unchanged positions are not accepted.

1. In the Text phase, enter ASCII bytes 32..126 in order with Caps Lock enabled,
   without repeated key events. Wait for
   `SAVED case=N bytes=95 write=true flush=true close=true` and the Saved phase,
   then press and release Enter. Escape aborts the run and requests restoration.

Require `MODE392 FINISHED status=0` with no cleanup error. Verify the original
mode was restored, then shut down normally. From the stopped OS volume, verify
all eighteen saved files contain exactly bytes 32..126, with no BOM or newline.
Retain the log and actual RGB captures. An acknowledged key or pointer command
alone does not prove guest input or stored data. Browser/VNC, normal Windows
Keep/Revert/timeout, cold nonzero-mode persistence and snapshot tests remain
separate checks in the [validation matrix](../../../docs/windows-validation.md).

## Maintained host checks

The test-only Cargo workspace has no dependencies. The existing standalone-tool
unit and Clippy gates discover it with the other tool workspaces. No Windows
image, SDK library or guest is needed for these host tests.

```sh
cargo test --locked --all-targets --manifest-path tools/windows/display/Cargo.toml
cargo clippy --locked --all-targets --manifest-path tools/windows/display/Cargo.toml -- -D warnings
```

These commands run the twenty probe unit cases. Guest executable builds retain
the separate compatibility-library procedure above.

## Native stroke and dash check

The [drawing probe](probe/drawing.rs) leaves the current display mode unchanged.
It draws forty cosmetic paths: five pen styles, opaque and transparent gaps,
and copy, XOR, inverted XOR and inverted-copy ROP2 operations. Paths have
fractional endpoints, two connected segments, closure and per-cell clipping.
The same GDI calls draw into a software DIB and directly into a visible window.
The reference therefore comes from the tested Windows version, not from the
Firecracker rasteriser.

Select the existing XP x86 or Windows 7 x64 compatibility profile, then build:

```bash
rustc --edition 2024 -D warnings "${windows_rustflags[@]}" \
  tools/windows/display/probe/drawing.rs -o build/windows/display-probe/drawing.exe
```

Run the executable from a new, writable guest directory and retain its standard
output. It writes `gdi-oracle.bgra` exclusively, flushes and closes it, then
reports `GDI READY`. The reference is a top-down 320-by-384 32bpp DIB. Following
[Microsoft's DIB synchronisation requirement](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-createdibsection),
the probe flushes GDI before reading the bitmap.

Within ninety seconds, capture the actual console frame. Keep the pointer
outside the test window and crop pixels `[64,384)` horizontally and `[64,448)`
vertically. Press F1 only after retaining that capture; Escape aborts. Require
`GDI FINISHED status=0` and process exit zero, then shut down Windows normally.
Collect the reference from the stopped disk using the existing read-only
inspection procedure. Compare all 122,880 BGR pixels with the captured region;
ignore the unused fourth byte. A screenshot without this comparison does not
establish a pass. Preserve failures without an automatic retry. This focused
check does not replace the mode, input, cursor or restore checks above.
