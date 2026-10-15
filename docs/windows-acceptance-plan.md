# Final Windows acceptance

Starting revision: `c972e9f62`, branch `feat/windows-console`.

The accepted scope is AMD64 Firecracker, installation and remote console,
conventional TAP egress, and real Tokoloshe HTTP CONNECT over vsock. It excludes
porting the Tokoloshe guest job runtime. The guest matrix is XP x86, Windows 7,
Windows 10, Windows 11 and Server 2025. Standard Windows 11 Pro/Enterprise is
explicitly included in addition to the Toets IoT LTSC edition. Record exact
editions and media; never infer support for other editions or future releases from one successful boot.

## Acceptance sequence

- [x] Inventory final build, firmware, media, disks and owned processes; record
      the duplication baseline and protect the live user desktop.
- [x] Review attached devices, PCI identities/classes, ACPI resources, firmware
      drivers, DMA bounds and snapshot invariants against primary specifications.
- [x] Compare modern Windows installation requirements with the provided virtual
      hardware. Standard Windows 11 requires additional TPM/Secure Boot work.
- [x] Implement and validate opt-in TPM 2.0, persistent TPM state and consistent
      snapshot handling. Preserve the minimal device set for other guests.
- [x] Implement and validate Secure Boot firmware, authenticated persistent
      variables and enrolled trust keys; reject tampered boot images.
- [x] Install standard Windows 11 from authorised media without bypassing its
      hardware checks; verify TPM and Secure Boot status in the installed guest.
- [x] Boot installation media with the final device set. Verify keyboard,
      pointer, optical media and target storage; complete fresh installation
      and reboot checks needed to validate the supported matrix.
- [x] Verify native VNC and noVNC input through actual viewer interfaces.
- [x] Re-run TAP and real Tokoloshe CONNECT egress after cold boot, full restore
      and differential restore on the final snapshot format.
- [x] Measure wall-clock and monotonic-clock behaviour across pause and restore;
      distinguish clock behaviour from VMClock null-driver recognition.
- [x] Fix reproduced defects with focused regression tests. Re-run affected
      guest checks whenever the device implementation or firmware changes.
- [x] Review and refine the complete patch series: atomic commits, licences,
      reproducible build/media steps, pinned artifacts and precise limitations.
- [x] Run final tests, strict Clippy, formatting and duplication checks; retain
      compact evidence, stop test processes and remove disposable disk/RAM data.
- [x] Prepare a concrete review description and report verified support and
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
failure. The secure EFI differential restore also passes. Standard Windows 11 Enterprise Evaluation has reached its desktop and passed
installed-guest secure full and differential restores on format 25. The final-format secure restore and egress results are recorded below. See [the secure platform](windows-secure-boot.md).

## Earlier installation and egress evidence

The earlier format-25 AMD64 VMM has SHA-256
`00f4e6267f83356c4eb92782f728f80ce3d68ee28f4b4ebc7e740e886d75fadb`.
Fresh XP x86 and Windows 7 x64 installations have reached their desktops with
stock USB input and passed a subsequent disk boot without optical media.
Windows 7 reports three unbound pre-existing ACPI feature IDs: `ACPI0013`,
`AMZNC10C` and `VMGENCTR`. Its USB controller, keyboard and HID input report
healthy status. These results do not establish drivers for those ACPI features.

Fresh Windows 10 Enterprise LTSC 2021 (19044.1288) has completed installation,
two installation resets, desktop input checks, a cold boot without optical
media and clean S5 shutdown. After installing the signed AWS VMClock recognition
INF, its device inventory reports no configuration errors. This INF recognises
the clock device; it does not establish a Windows VMClock implementation.

XP, Windows 7, Windows 10, Windows 11 IoT and Server 2025 have passed real
Tokoloshe CONNECT after cold boot, full restore and differential restore on
format 25. Each run checks three allowed transfers, three denied requests with
zero payload, persisted disk data and clean shutdown. Host timestamps establish
that each restore produced a new transfer. Windows 7 CONNECT used the first
format-25 build (`94f80c2a1e621ef2fbbabee70f65920944203d3bd651f414335ba8333cacd469`);
the guarded TAP rerun used the reviewed build. The retained per-run manifest
distinguishes the binaries.

XP, Windows 7, fresh Windows 10, Windows 11 IoT and Server have also passed
the guarded TAP sequence. Host receive timestamps and exact per-phase report
counts establish that each restore produced new traffic. The earlier TAP logs
alone did not establish these boundaries; the guarded reruns close that gap.
At that checkpoint, standard Windows 11 still needed its own egress checks with
Secure Boot and TPM state retained across restore. The format-27 table below
records the current acceptance state.

The first fresh Server 2025 Desktop Experience installation failed in
`RdpSysPrepRestoreOffline` with an access violation. Logs and static analysis
point to SMBIOS hardware-ID parsing. A runtime probe confirms that the firmware
provides no SMBIOS data, but the read-only DLL helper returns a failure code
without crashing. The same installation subsequently reached its desktop with
an isolated firmware image that adds valid SMBIOS Type 1 identity. The production
implementation uses an explicit CloudHv profile and persistent caller UUID;
fresh installation with that implementation has now reached Desktop Experience
using the ordinary firmware without the diagnostic SMBIOS injection. It completed both setup phases and
booted without optical media. The installed guest reports the configured UUID
`115a7a1f-ca85-4aab-81a8-42f77b44ba85`; keyboard and absolute pointer checks pass.
The successful historical Server egress result used the earlier installed image
and does not qualify this fresh installation. The final-format results below
record its subsequent restore and egress checks.

The variable helper has 20 passing tests and strict Clippy validation. A signed
platform-key deletion regression exposed stale derived mode variables; the fix
updates them in the same transaction and preserves them across state restore.
The maximum optional-device-buffer VMM snapshot fixture passes separately from
the 1,007-test serial run described above.

The physical-presence work is tracked in the
[TPM completion sequence](windows-tpm-plan.md#windows-physical-presence-completion).
Its helper persistence tests and bounded transport tests pass. Firmware execution
passes confirmation refusal, confirmed clear, unsupported-operation refusal and
an independent owner-NV check of the actual TPM clear. Installed standard Windows
11 passes Get-Tpm, physical-presence WMI queries, Secure Boot and the public TBS
probe on format 26. The final format-27 matrix is recorded below. Earlier results above
remain versioned evidence.

The format-26 VMM unit suite passes 1,015 tests with two ignored by the ordinary
suite. Of these, 72 require TAP permissions and passed in an isolated privileged
network namespace after permission failures in the unprivileged run. ACPICA
execution and the real TPM backend have separate checks. The final format-27
identity change passes nine focused tests, 11 architecture tests, API parsing,
maximum snapshot-size checks and strict AMD64/ARM64 Clippy. The maximum fixture
includes the firmware profile and occupies 29,474,015 bytes, below the 32 MiB
reader limit. Both actual CloudHv firmware artifacts pass the ELF validator and
PVH loader; the regression covers their ELF64/EM_386 entry convention. The variable helper
passes all 29 tests. These do not substitute for final-format guest checks.

The final source also passes strict all-target workspace Clippy for AMD64 and
ARM64 and the pinned Rust formatter. The workspace check exposed existing jailer
test lints; the test-only fix retains child output until the harness finishes.
Both privileged namespace tests and the focused path/descriptor tests pass.
ARM64 compilation remains a regression check, not Windows guest qualification.

Final comparable duplication analysis reports 625 clones and 6,887 duplicated
lines across 376 files; the earlier baseline was 606 clones and 6,691 lines.
The new TPM-state checks and firmware-map fixtures each have zero scoped clones.
ShellCheck passes at warning level for the egress and firmware test scripts.

## Format-27 acceptance after the restart checkpoint

The current VMM is `firecracker-smbios27-pvh`, SHA-256
`4d2fa9ca73d0ca332a637b38827ec58abe22a7c9c32333fc4a186f0386ee4069`.
The following results are distinct from the historical format-25 matrix above.

| Guest | TAP cold/full/diff | CONNECT cold/full/diff | Status |
|---|---|---|---|
| XP Professional SP3 x86, build 2600 | Pass | Pass | Persisted data and clean S5 verified |
| Windows 7 Enterprise SP1 x64, build 7601 | Pass | Pass | Persisted data and clean S5 verified |
| Windows 10 Enterprise LTSC 2021, build 19044.1288 | Corrected firmware: pass | Corrected firmware: pass | Offline storage hashes, sealed accounting and clean S5 verified |
| Windows 11 IoT Enterprise LTSC, build 26100.1742 | Corrected firmware: pass | Corrected firmware: pass | Offline storage hashes, sealed accounting and clean S5 verified |
| Standard Windows 11 Enterprise Evaluation 26H2, build 26300.9457 | Corrected secure firmware: pass | Corrected secure firmware: pass | Unchanged PCR 7/public TPM identity, read-only encrypted-disk hashes and clean S5 pass |
| Fresh Server 2025 Standard Evaluation, Desktop Experience, build 26100.32230 | Corrected firmware: pass | Corrected firmware: pass | Final-firmware fresh installation, offline hashes, accounting and clean S5 pass |

XP used the earlier format-27 binary with SHA-256
`94ea93b98c03784e4c3deb69e2c888f09811e8ce4e5a1553df16607fc90649a2`.
The subsequent ELF machine validation correction affects the explicit CloudHv
profile, which XP's SeaBIOS configuration does not select. XP uses HTTP for its
TAP reachability check; Windows 7 and Windows 10 use certificate-validated
TLS 1.2. XP and Windows 7 use the documented custom test-signed vsock driver.

The IoT guest reset approximately 20 seconds after full restore. A controlled
single-probe rerun reproduces bugcheck `0xA` (`IRQL_NOT_LESS_OR_EQUAL`). Public
Microsoft symbols locate the fault in page-table ageing. Snapshot inspection
shows Windows using the VMClock page at GPA `0x20fe000` as a page table.
VMClock's restore update changes its disruption marker at offset `0x10` from
zero to one, which Windows interprets as a present mapping of physical page zero.
The resulting page-table read at GPA `0x508` reads the boot GDT descriptor
`0x00cf9b000000ffff`, derives an invalid PFN and produces the exact recorded fault
address. The firmware reservation correction below resolves this collision
without suppressing the VMClock notification. The original snapshot and dump
are retained as failure evidence; affected firmware profiles require fresh
cold-boot acceptance.

A controlled run with only the optional SMBIOS profile omitted passes full and
differential restore. This changes Windows' allocation pattern; it does not
make the missing firmware reservation safe. The corrected profile remains the
acceptance target.

The disposable image also had duplicate startup scripts. Removing them did not
prevent the crash. The harness preparation instructions now require one probe
and a separate completed driver-installation boot.

## CloudHv reservation correction

- [x] Preserve the IoT crash dump and exact full snapshot.
- [x] Reproduce the crash with one probe and automatic restart disabled.
- [x] Reconstruct the page-table collision and exact bugcheck address.
- [x] Identify firmware flattening of low RAM and filtering of reserved PVH entries.
- [x] Preserve each usable PVH interval and each reserved hole in CloudHv HOBs;
      reject malformed maps before publishing memory resources.
- [x] Check actual UEFI GetMemoryMap output for the platform reservation and
      validate ordinary, secure and headless firmware paths as applicable.
- [x] Independently review the firmware patch and freeze artifact hashes.
- [x] Repeat affected modern Windows cold/full/differential acceptance with
      corrected firmware, including secure state and both egress paths.
- [x] Revalidate fresh installation against the final firmware and device set.

The actual ordinary-firmware UEFI map probe passes with the correction and fails
on the previous firmware. Independent inspection of all 96 returned descriptors
confirms reserved platform and SMBIOS pages, no allocatable overlap and usable
RAM above 4 GiB. The signed secure-firmware probe also passes independent review
of all 91 descriptors. The headless RELEASE build and nine exact-function
malformed-map fixtures pass. The correction and reproducible checks are committed
as `c4ee4837b`; new ordinary and secure artifact hashes are recorded in the
[firmware memory-map guide](../resources/firmware/pvh-memory-map.md).

With corrected ordinary firmware, IoT Windows passes TAP and real Tokoloshe
CONNECT after cold boot, full restore and differential restore. Offline storage
hashes, three allowed transfers, three zero-payload denials and clean S5 pass.
The new snapshot retains the VMClock header in its platform page. Windows 10
also passes both corrected-firmware paths with offline hashes and clean shutdown.
Server and standard Windows 11 also pass both paths.

A separate blank-disk Server 2025 Standard Evaluation Desktop Experience
installation also passes with the final corrected firmware. The GUI loads the
signed viostor driver; it remains a running boot driver after media removal and
cold reboot. Setup, OOBE and desktop input pass; the configured UUID is retained
and the guest exits through clean S5. Compact public evidence is retained as
`build/windows/acceptance27/server-final-map-install.tar.gz` with SHA-256
`b32b67cb9e113dad79d5c1a169a399e5cad9a0a0b651b9e2f437753f168a7f33`.
The disposable installer disk was removed after verification.

The VMM snapshot format need not change solely for this firmware correction.
The new firmware needs a cold boot; restoring an old snapshot retains its old
Windows memory allocation and cannot qualify the corrected reservation.

## Server firmware identity investigation

- [x] Preserve the failed Desktop Experience setup logs before reusing its disk.
- [x] Confirm missing SMBIOS with the public GetSystemFirmwareTable API.
- [x] Build an isolated firmware diagnostic with a valid SMBIOS entry point and
      Type 1 UUID, without changing the Windows image or VMM.
- [x] Confirm the diagnostic table and UUID through the same public API.
- [x] Complete the same fresh Desktop Experience installation with that firmware.
- [x] If the comparison establishes the cause, add a bounded CloudHv firmware
      identity configuration with a caller-supplied UUID stable across cold boots.
- [x] Reserve the table's physical memory and reject unsupported boot profiles.
- [x] Test table checksums, string bounds, UUID byte order and snapshot retention.
- [x] Repeat fresh installation with the production implementation and normal
      firmware profile, without the diagnostic SMBIOS injection.

The diagnostic UUID is a fixture, not a production default. SMBIOS identity is
independent of VM Generation ID, which changes at restore. The BIOS loader must
not overwrite SeaBIOS's own table area. Microsoft uses SMBIOS values for hardware
identification; preserve the identity for each VM rather than generating a new
one at each launch. See [Microsoft's SMBIOS guidance](https://learn.microsoft.com/en-us/windows-hardware/drivers/bringup/smbios)
and [DMTF DSP0134](https://www.dmtf.org/sites/default/files/standards/documents/DSP0134_3.9.0.pdf).

## Final secure Windows result

Standard Windows 11 Enterprise Evaluation 26H2, build 26300.9457, passes TAP
and real Tokoloshe CONNECT after cold boot, full restore and differential
restore with the corrected secure firmware. All six phases pass Secure Boot,
Get-Tpm readiness and TBS GetRandom. Independent transcript checks confirm
identical SHA-256 PCR 7 and persistent TPM object handle, public area and Name
within each run and across the intervening cold boot. No key was provisioned
and no TPM state was cleared to obtain these results.

Both closed encrypted-disk checks pass read-only storage verification. CONNECT
records three allowed transfers and three zero-payload denials in six sealed
accounting entries. Both guests exit through S5. The test image has BitLocker
protection off and no protectors; this does not qualify TPM-sealed recovery.
The desktop reports an expired evaluation licence. The legacy registry
ProductName reads Windows 10 Enterprise Evaluation; the desktop and build
identify the tested Windows 11 edition. Windows 11 Pro is not separately tested.

The secure TAP run exposed a harness permission defect: private root-owned PID
files could not be read during cleanup. Commit `d57d3ae61` reads both TERM/KILL
PID paths with privilege. An isolated native regression and the complete rerun
verify the fix. Recovery imported the paused checkpoint into fresh same-identity
helper stores without resuming the guest, then closed the matched state before
the new cold run. No failed partial helper state was reused.

The compact standard Windows 11 evidence archive is
`build/windows/acceptance27/win11-standard-final.tar.gz`, SHA-256
`b0fdd12aec8f0adcc2e807d31a3803f32d4603fc42bb1dfb7ddb9b6a353ffb32`.
It contains public transcripts, TPM public baselines, data hashes, configurations
and phase/accounting logs; private helper state and credentials are excluded.

## Review and resource closure

The final review description, ordered patch manifest and application proof are
retained under `build/windows/review/final-series/`. The export is checked by
applying every patch to the recorded upstream base through an alternate index
and comparing the resulting tree to the branch head. No push or merge is made.

Owned acceptance VMs, helpers, mounts, loop devices and network namespaces are
stopped or removed. The user's desktop and VNC tunnel are preserved. Successful
RAM snapshots and disposable phase state are removed. The standard Windows 11
reference remains closed with its matching TPM and variable store. Original
reference images and the IoT failure snapshot/dump remain as evidence. Native
free space at closure is 218 GiB on the root volume and 297 GiB on `/data`.
