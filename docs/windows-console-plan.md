# Remote Windows installation and console

This series starts at `f92e897f4b7d1157b26824a4773ceaaeee434b53` on
`feat/windows-console`. It adds optional AMD64 installation hardware and a
host-side remote console. The accepted guest matrix is XP x86, Windows 7,
Windows 10, Windows 11 and Server 2025. Existing headless Windows and Linux
behaviour remains a regression requirement. ARM console hardware is outside
this initial AMD64 implementation.

## Sequence

- [x] Preserve the reviewed Windows branch and measure duplication.
- [x] Establish display, PS/2 and ATAPI interfaces from primary references.
- [x] Implement and unit-test bounded VGA/VBE display state and rendering.
- [x] Implement and unit-test PS/2 controller, keyboard and mouse state.
- [x] Implement and unit-test read-only IDE/ATAPI optical media and transfers.
- [x] Add typed opt-in configuration, PCI/PIO/MMIO mapping and ACPI resources.
- [x] Persist and validate all added device state, including display memory.
- [x] Add a local console interface and remote display/input service.
- [x] Build matching SeaBIOS and EDK II display/optical firmware.
- [x] Verify firmware screen, keyboard input and boot from installation media.
- [x] Complete a remote Windows installation, including reboot and media eject.
- [x] Exercise each supported guest's display/input and snapshot restore.
- [x] Re-run headless regressions, formatting, Clippy and duplication checks.
- [x] Record results, remaining limits and reproducible operator procedures.
- [x] Stop owned processes and remove disposable images after preserving evidence.

## Ownership

The display agent owns display hardware and Windows 7 validation; the input
agent owns PS/2, the console bridge and Windows 11 validation. The optical
agent owns ATAPI, media preparation, UEFI firmware and Server 2025 validation.
The parent owns shared integration, XP/Windows 10 and headless regression
checks. Shared-file edits are coordinated before ownership is transferred.

## Verification rules

Protocol tests precede guest claims. A screenshot alone does not establish
input, installation or restore. Remote-console validation must work without
a guest network device or guest Viosock driver. Browser disconnection must
not stop installation. Restore must reject invalid device state or mismatched
optical media. Record guest, firmware and binary revisions with each result.

Baseline command: `mise exec -- jscpd src/vmm src/acpi-tables src/firecracker
tools/windows --ignore '**/generated/**,**/*.patch' --reporters console --silent`.
Result: 591 clones, 6,570 duplicated lines; log in
`build/windows/console/duplication-before.log`. This invocation uses different
thresholds from the preceding review; compare only like-for-like invocations.

Final owned-code check excludes the unmodified `tools/windows/console/web`
vendor tree. It reports 596 clones and 6,611 duplicated lines. The new matches
are test setup; no new production-code clone was identified. Native VMM
tests pass (964); strict AMD64 and ARM64 Clippy and formatting pass. The final
reset logging change also passes the 20 reset tests with TAP privileges.
The pre-integration device-model checkpoint also passes strict Clippy and
formatting after the KVM mapping accessors and their test move into the
integration patch. History refinement preserves the final tested source tree.

All five installed guests passed the matrix in [the console guide](windows-console.md#evidence).
Owned VMs, bridges and temporary mounts are stopped. Disposable RAM snapshots,
generated ISOs, obsolete binaries and compiler caches were removed. Installed
test disks, final firmware, logs, digests and screenshots remain for reuse.
