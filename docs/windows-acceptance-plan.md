# Final Windows acceptance

Starting revision: `c972e9f62`, branch `feat/windows-console`.

The accepted scope is AMD64 Firecracker, installation and remote console,
conventional TAP egress, and real Tokoloshe HTTP CONNECT over vsock. It excludes
porting the Tokoloshe guest job runtime. The guest matrix is XP x86, Windows 7,
Windows 10, Windows 11 and Server 2025. Standard Windows 11 Pro/Enterprise is
explicitly included in addition to the Toets IoT LTSC edition. Record exact
editions and media; never
infer support for other editions or future releases from one successful boot.

## Acceptance sequence

- [x] Inventory final build, firmware, media, disks and owned processes; record
      the duplication baseline and protect the live user desktop.
- [ ] Review attached devices, PCI identities/classes, ACPI resources, firmware
      drivers, DMA bounds and snapshot invariants against primary specifications.
- [x] Compare modern Windows installation requirements with the provided virtual
      hardware. Standard Windows 11 requires additional TPM/Secure Boot work.
- [ ] Implement and validate opt-in TPM 2.0, persistent TPM state and consistent
      snapshot handling. Preserve the minimal device set for other guests.
- [ ] Implement and validate Secure Boot firmware, authenticated persistent
      variables and enrolled trust keys; reject tampered boot images.
- [ ] Install standard Windows 11 from authorised media without bypassing its
      hardware checks; verify TPM and Secure Boot status in the installed guest.
- [ ] Boot installation media with the final device set. Verify keyboard,
      pointer, optical media and target storage; complete fresh installation
      and reboot checks needed to validate the supported matrix.
- [x] Verify native VNC and noVNC input through actual viewer interfaces.
- [ ] Re-run TAP and real Tokoloshe CONNECT egress after cold boot, full restore
      and differential restore on the final snapshot format.
- [x] Measure wall-clock and monotonic-clock behaviour across pause and restore;
      distinguish clock behaviour from VMClock null-driver recognition.
- [ ] Fix reproduced defects with focused regression tests. Re-run affected
      guest checks whenever the device implementation or firmware changes.
- [ ] Review and refine the complete patch series: atomic commits, licences,
      reproducible build/media steps, pinned artifacts and precise limitations.
- [ ] Run final tests, strict Clippy, formatting and duplication checks; retain
      compact evidence, stop test processes and remove disposable disk/RAM data.
- [ ] Prepare a concrete review description and report verified support and
      remaining constraints without claiming untested OS compatibility.

## Ownership and resource limits

The display agent reviews platform/device correctness and patch quality. The
optical agent validates installation media and guest installation. The input
agent validates TAP and vsock egress. The parent validates browser interaction,
clock behaviour, integration and final evidence. Shared-source edits require
coordination. Each lane runs at most one guest. Do not open a writable disk in
multiple VMMs. Keep the user's Windows 10 desktop and port 5900 available except
for bounded tests that require its restart. Use separate test ports and disks.

All test processes need an owner and a bounded lifetime. Check free space before
large copies; remove RAM snapshots and disposable disks after recording results.
Keep original installation media, installed reference disks and compact evidence.

## Combined-platform checks

The first format-25 build passes 1,007 VMM tests with one external-helper test
ignored by the ordinary suite. The separate real-swtpm test and Linux guest
checks cover that helper boundary. Run the VMM suite serially: parallel tests
contend over shared TAP names and global metrics. The serial run has no failures.
The API suite passes 84 tests, plus a focused restore-policy boundary test;
ACPI tables pass 27 and utilities 18. All 11 Rust VMM integration tests pass.
Their boot checks now process events until guest exit or the existing deadline,
instead of assuming that the first event is shutdown. Strict Clippy
passes for AMD64 and ARM64; ARM64 compilation does not qualify ARM Windows.

These checks do not replace the remaining installation and egress matrix.
The secure firmware probe passes signed boot, unsigned-image rejection, cold
variable persistence, full restore into isolated stores and runtime helper
failure. The secure EFI differential restore also passes. Standard Windows 11 installation
and installed-guest secure restore checks remain in progress. See [the secure platform](windows-secure-boot.md).

## Final-format installation and egress progress

The reviewed AMD64 VMM has SHA-256
`00f4e6267f83356c4eb92782f728f80ce3d68ee28f4b4ebc7e740e886d75fadb`.
Fresh XP x86 and Windows 7 x64 installations have reached their desktops with
stock USB input and passed a subsequent disk boot without optical media.
Windows 7 reports three unbound pre-existing ACPI feature IDs: `ACPI0013`,
`AMZNC10C` and `VMGENCTR`. Its USB controller, keyboard and HID input report
healthy status. These results do not establish drivers for those ACPI features.

Windows 10 and Windows 11 IoT have passed TAP and real Tokoloshe CONNECT after
cold boot, full restore and differential restore on format 25. Both paths check
payload equality, persisted disk data and clean shutdown. CONNECT also checks
three allowed transfers and three denied requests in sealed proxy accounting.
The final-format XP, Windows 7 and Server reruns remain in progress.

The variable helper has 20 passing tests and strict Clippy validation. A signed
platform-key deletion regression exposed stale derived mode variables; the fix
updates them in the same transaction and preserves them across state restore.
The maximum optional-device-buffer VMM snapshot fixture passes separately from
the 1,007-test serial run described above.
