# Windows guest support: investigation and validation history

This document preserves the chronological implementation record, including
failures later resolved. Use [Windows guest support](windows.md) for current
behaviour and [the patch review](windows-review-plan.md) for the final quality
checks. The final validation matrix below supersedes earlier pending results.

Implementation branch: `feat/windows-guests`, based on `658ff27eb`.

## Scope

Add x86-64 Windows guest boot through EDK II CloudHv PVH firmware and
modern virtio PCI devices. Preserve direct Linux and FreeBSD boot.
Validate AMD64 on `jacquesg@192.168.1.252`. Obtain the Windows Server 2025
evaluation ISO from Microsoft and prepare a separate guest disk. The follow-up
includes ARM64 Windows support and Tokoloshe HTTP CONNECT egress over vsock.

## Sequence

- [x] Create an isolated worktree and inspect the existing boot path.
- [x] Establish the duplication baseline: 58 clones, 650 duplicated lines
  (4.19%) across architecture, ACPI and PCI transport code.
- [x] Confirm firmware interfaces against EDK II and document the API.
- [x] Add the PVH ACPI handoff and firmware timer/power interfaces, with tests.
- [x] Add opt-in Hyper-V CPU configuration with capability validation and tests.
- [x] Define and enforce the snapshot behaviour for the new guest mode.
  - [x] Validate host capabilities before restoring Hyper-V vCPUs.
  - [x] Add SynIC/timer save, restore and resnapshot coverage.
  - [x] Execute the KVM snapshot tests and verify clock behaviour.
- [x] Resolve the firmware PCI aperture against Firecracker's fixed MMIO ranges.
  - [x] Add a firmware patch and build instructions for a pinned EDK II tag.
  - [x] Verify that the patch applies to the pinned source.
  - [x] Build the patched firmware and verify PCI enumeration.
- [x] Prepare repeatable unattended installation media with virtio drivers.
  - [x] Add a Rust preparation tool and Server Core answer template.
  - [x] Obtain the Microsoft ISO and virtio drivers; generate a separate raw
    disk, answer ISO and OVMF variable store without starting a VM.
  - [x] Check XML and generated shell syntax; verify existing output is refused.
  - [x] Verify image 1 is AMD64 Windows Server 2025 Standard Evaluation Core.
  - [x] Run the unattended installation.
  - [x] Correct EFI boot-file creation order and explicitly target the new
    default boot entry when enabling EMS.
  - [x] Validate a fresh installation with the corrected entry identifiers.
- [x] Build and run focused tests on the isolated Linux host.
- [x] Boot CloudHv firmware and verify PCI disk discovery.
- [x] Add and validate the firmware CMOS/RTC interface.
- [x] Boot Windows; verify serial console, storage, network and shutdown.
- [x] Run Linux boot regressions, formatting, Clippy and duplication checks.
- [x] Record test evidence and remaining limitations in the Windows guide.

## Evidence

### Conventional network egress follow-up

The scope remains Firecracker guest support and verified egress; it excludes
porting the Tokoloshe guest runtime. Earlier AMD64 TAP ping tests establish
local connectivity; the completed routing checks are recorded below.

- [x] Configure isolated virtio-net/TAP test paths on AMD64 and ARM64.
- [x] Verify Windows adapter activation, address configuration and routing.
- [x] Verify DNS and outbound HTTPS through the TAP path on both architectures.
- [x] Verify bidirectional payload integrity through routed TCP on both architectures.
- [x] Record commands, results and limitations in the Windows guides.
- [x] Stop test processes and remove disposable disks and network resources.

AMD64 conventional egress passes on 2026-10-13 at 15:35 UTC. Windows Server
2025 runs in Firecracker with one NetKVM device and no vsock device. The
guest uses `172.30.254.2/30`, gateway `172.30.254.1`, and the LAN DNS server.
The TAP is inside a separate network namespace, routed over a veth pair;
the host applies source NAT for Internet access. The guest resolves
`www.microsoft.com`, receives HTTPS 200 with certificate validation enabled
and proxies disabled, and compares every byte of a 2,097,152-byte TCP echo
exchange. The independent host echo server records exactly 2,097,152 bytes.
Serial evidence is `build/windows/amd64-network-serial.log`; metrics are in
`build/windows/amd64-network-metrics.json`. ACPI S5 stops Firecracker with
exit code 0 at 15:36 UTC. The disposable disk, TAP namespace, veth pair,
route and test-specific firewall rules are removed.

ARM64 conventional egress passes on 2026-10-13 at 17:39:59 UTC. Windows 11
IoT Enterprise LTSC runs in Firecracker, inside the PMU-capable emulated
Linux/KVM appliance, with default seccomp and no vsock device. NetKVM is Up;
the SYSTEM task configures `172.30.253.2/30`, gateway `172.30.253.1` and
DNS `10.0.2.3`. The appliance routes TAP traffic through `enp0s3` with source
NAT. DNS resolves `www.microsoft.com`; curl returns HTTPS 200 with certificate
validation enabled and proxies disabled. The transcript records
`VERIFIED TAP DNS HTTPS and 2097152 bytes bidirectional TCP`. The host echo
server independently records exactly 2,097,152 bytes for that connection.
An earlier 15-byte appliance control connection is separate from this proof.
Metrics show zero seccomp faults, VMM panics and vCPU failures. Evidence and
the probe script are retained under local `build/windows/arm64-host`.

After saving evidence, Firecracker and the echo server are terminated. The
TAP and test nftables table are removed, forwarding is restored to zero,
and the disposable Windows disk and preparation firmware-variable files
are deleted. The state filesystem is trimmed and the appliance powers off
with QEMU exit code zero. Its retained baseline/snapshot state occupies
18 GiB on the host. This test does not add a Windows graceful-shutdown claim.

- [x] Validate disabling hibernation during disk preparation, then verify
  that a full Windows shutdown permits ordinary offline NTFS writes.

On ARM64, `powercfg /hibernate off && shutdown /s /t 0` completes from an
elevated prompt. After QEMU exits, `hiberfil.sys` is absent and `ntfs-3g`
mounts the Windows partition with `rw` without recovery overrides. The
network probe is copied and compared before unmounting. AMD64 Server Core
also returns `HIBERNATE_EXIT=0` and shuts down through ACPI S5 with
Firecracker exit code 0. The answer template now disables hibernation.

### Legacy Windows validation follow-up

XP BIOS and device sequence:

- [x] Build pinned SeaBIOS 1.17.0 as a coreboot payload and inspect its entry ABI.
- [x] Prototype a Rust PVH-to-coreboot firmware handoff with memory and ACPI tables.
- [x] Boot the unchanged XP disk to identify the next unsupported interface.
- [x] Implement and test the required legacy virtio PCI transport and interrupts.
- [x] Verify XP storage, networking, shutdown and full/differential restore.

The firmware prototype uses the existing PVH interface. It must preserve the
guest memory map and ACPI data, and must not change the original Toets disk.
SeaBIOS source is pinned to `b52ca86e094d19b58e2304417787e96b940e39c6`.
The prototype reaches SeaBIOS, discovers the ACPI PM timer and initialises
the PCI disk. Its virtio driver needs a separate ACKNOWLEDGE write before
DRIVER, as required by the virtio initialisation sequence. With that firmware
correction, disk reads expose a VMM defect: notifications through the PCI
configuration access capability never reach the queue eventfd. The focused
regression fails with an empty eventfd, then passes after adding forwarding.
All 27 PCI transport tests pass with forwarding. The runtime test then reads
the disk, reports `Booting from 0000:7c00` and executes further BIOS calls.
At 20:20:03 UTC it stops with `KVM_EXIT_SHUTDOWN`, not guest ACPI S5. The VMM
logs a vCPU error but exits zero, so process status is not boot-success
evidence. XP boot remains unverified; the next step is to trace that exit.
The prototype and before/after logs are under `build/windows/legacy/pvh-bios`.
The incorrect zero exit is a publication race: the vCPU signals its exit
eventfd before sending its error response. Publishing the response first
fixes it. A regression saturates a blocking eventfd and checks that the
response arrives before notification can complete. All nine vCPU tests pass.
Repeating the same XP failure returns `GenericError` and process exit 1 at
20:24:02 UTC. No XP kernel or userspace boot is claimed.

The next trace reaches the XP PAE kernel. AMD KVM resets register state after
a shutdown intercept, so a temporary trace instance records the earlier
guest RIP. A breakpoint in `KiSaveProcessorControlState` finds the original
bug check: `0x79, 4, 0xac31, 0, 0`. The matching HAL symbols and disassembly
show a failed root-table signature check. `HalpAcpiRsdt` is zero, copied from
the loader's ACPI entry; the RSDP has a zero 32-bit root pointer while its
XSDT remains intact. No guest binary is modified. The trace instance and
debugger VM are removed after collecting the evidence.

- [x] Add a checked 32-bit RSDT alongside the x86 XSDT and validate both checksums.
- [x] Repeat XP boot to verify the root-table failure is resolved.

The RSDP regression first fails on the zero legacy pointer. With the fix, all
14 selected VMM ACPI tests and 26 ACPI table tests pass. Both roots reference
the same FADT, MADT and MCFG; 32-bit conversions reject overflow. ARM retains
its XSDT-only RSDP, including addresses above 4 GiB. Native and ARM64 Clippy
checks pass with warnings denied. The scoped duplication scan remains at
10 clones and 84 duplicated lines.

The XP debugger stops after the HAL accepts RSDT address `0xa051e`, length
48, with all three expected entries. Boot next reaches ACPI.sys and bug check
`0xa5, 0x0d, <device>, 0x4449555f, 0`. Its duplicate-hardware-ID check compares
EC00 with PRES: EC00 has `_UID = "ECAM0000"`, but PRES has no `_UID`.
The namespace nodes and device-extension flags confirm the missing PRES ID;
no guest state is patched. The debugger is detached after collecting evidence.

- [x] Add a unique ID to the power-resource reservation, with a regression.
- [x] Repeat XP boot and trace the next failure.

PRES now has `_UID = "POWER"`, distinct from ECAM reservations. The generated
AML regression fails before the change; both legacy-manager tests pass after
it. This follows the shared-hardware-ID rule in
[ACPI 1.0b section 6.1.6](https://uefi.org/sites/default/files/resources/ACPI_1_Errata_B.pdf).
Clippy passes with warnings denied; the file-scoped duplication scan stays at
zero clones. The next unchanged XP boot passes this check and reaches PCI
resource validation. It stops with `0xa5, 2, <PC00>, <resources>, <E820>`.
The returned resource list describes the 256 GiB aperture
`0x4000000000..0x7fffffffff` with a zero 32-bit length. ACPI.sys compares that
length with the full 64-bit range, increments its validation-error count and
stops. The E820 entries and PCI windows do not show a RAM overlap. The
debugger log and disassembly are retained under the BIOS prototype directory.

- [x] Add a legacy PCI platform with a representable memory aperture and
  transitional virtio transport, including I/O BARs and INTx delivery.
- [x] Test register behaviour, interrupt acknowledgement and snapshot restore
  before repeating XP boot and network checks.

Legacy PCI implementation sequence:

1. [x] Extend PCI BAR encoding to I/O space. Reproduce the incorrect treatment
   of address bit 2 as a 64-bit memory BAR; verify sizing, byte writes and
   neighbouring BARs. Keep the existing encoded snapshot representation.
2. [x] Add the contiguous legacy queue layout: a 32-bit page frame number,
   fixed maximum queue size and a used ring aligned to 4096 bytes. Verify
   descriptor consumption, completion writes, high addresses and reset.
3. [x] Add an explicit transitional transport selection on x86. Validate it at
   machine configuration, use an address aperture representable by 32-bit
   guests, and persist the selection with the VM's resource allocation.
4. [x] Allocate and map the legacy I/O BAR. Advertise transitional PCI IDs,
   revision and subsystem IDs; retain the modern capabilities on a separate
   memory BAR. Test command-register decoding and BAR relocation.
5. [x] Implement the legacy register layout, feature/status negotiation,
   page-frame queue addresses, notifications, configuration reads and reset.
   Follow virtio 1.2 section 3.1.2: allow configuration before feature
   negotiation and queue use before DRIVER_OK; do not require FEATURES_OK.
   Select the 10-byte or 12-byte network header from negotiated VERSION_1 and
   MRG_RXBUF features; configure TAP accordingly and preserve payload bytes in
   RX, TX and MMDS paths. Register integration, backend tests and XP guest
   validation are complete.
6. [x] Implement INTx assertion, ISR read-to-clear, masking and resampling;
   publish matching PCI interrupt pins and ACPI routes. Test real KVM delivery.
7. [x] Preserve both transport paths across full and differential snapshots;
   increment the snapshot format for changed state and validate malformed input.
8. [x] Boot the unchanged XP disk, resolve observed platform failures, then
   verify storage, TAP egress, shutdown and full/differential restore.
9. [x] Repeat modern AMD64 and ARM64 boot, TAP and Tokoloshe CONNECT tests with
   the final snapshot format and source revision.
10. [x] Stage separate Firecracker test disks from Toets' `toets/guests/win10.toml`
    and `win11.toml` definitions. After the current guest list passes, verify
    Windows 10 and Windows 11 boot, storage, TAP egress, shutdown and
    full/differential restore. Verify Tokoloshe CONNECT egress over vsock with
    compatible drivers. Resolve failures and record the exact guest revisions.

Reproducible BIOS firmware delivery:

- [x] Preserve the tested PVH handoff, linker script, SeaBIOS configuration
  and ordered-status patch under `resources/firmware`.
- [x] Document and run an isolated build from the pinned SeaBIOS revision.
- [x] Validate the rebuilt firmware with XP before marking this path complete.

The I/O BAR regression fails before the change: an encoded four-byte I/O BAR
at `0x1004` is misidentified as a 64-bit memory BAR and consumes the next slot.
I/O BARs now retain their address bits, reserved bit and sizing mask through
full and partial writes. Constructors reject invalid sizes and alignment.
All 37 PCI configuration tests pass, with native and ARM64 Clippy warnings
denied. The scoped duplication scan falls from 19 clones / 163 lines to
18 clones / 158 lines. This is BAR support only; the legacy virtio transport
is not yet exposed to guests.

The queue foundation uses the device's fixed maximum queue size and retains
all 32 page-frame bits before the 12-bit shift. It places the available ring
after the descriptor table and aligns the used ring to a 4096-byte boundary.
A zero page-frame value clears queue state. The tests exercise descriptor
consumption and completion writes in guest memory, reset after use, and the
maximum page-frame value with 32,768 descriptors. All 15 queue tests pass;
the scoped duplication scan remains at five clones and 39 duplicated lines.
Transport integration must stop a queue before reconfiguration.

- [x] Obtain Windows XP 32-bit and Windows 7 64-bit installation media.
- [x] Audit firmware, ACPI and virtio driver requirements for each guest.
- [x] Prepare and install each guest on a separate disposable disk.
- [x] Boot each installed guest in Firecracker and resolve platform failures.
- [x] Verify storage and conventional network egress; audit available vsock drivers.
- [x] Verify snapshot recovery and shutdown, and document actual limitations.
- [x] Stop test processes and remove redundant installation artefacts.

The initial legacy checks are supported by the format-19 results and the
[Viosock package audit](#legacy-viosock-package-audit) below. Subsequent custom
driver builds verify XP and Windows 7 CONNECT on format 20; see the
[completed legacy matrices](#reopened-xp-and-windows-7-vsock-validation).

Windows 7 allocation failure investigation:

- [x] Trace the matching EFI loader with Microsoft's public symbols.
- [x] Isolate the failed allocation and verify a minimal diagnostic change.
- [x] Reserve page zero in the CloudHv firmware memory map, then rebuild.
- [x] Repeat the unmodified Windows 7 loader test in Firecracker.
- [x] Recheck modern AMD64 Windows with the changed firmware.

The QEMU/OVMF debugger trace finds a 128-page `AllocateAddress` request at
physical address zero. Firmware rejects it; the loader converts the result
to `0xc0000225`. The pinned EDK II allocator explicitly rejects address zero,
although its free-memory descriptor includes it. A debugger-only experiment
excludes page zero from that descriptor; `BlpMmInitialize` then returns zero.
This isolates the memory-initialisation failure, not a complete Windows boot.
Logs and matching EFI/PDB files are in `build/windows/legacy/bootloader-debug`
on the Linux host. The firmware change must reserve the unavailable page;
it must not modify the Windows loader or remove firmware allocation checks.

The release firmware with the reservation has SHA-256
`44e90992add578d5fe3ddf395c797289dde025c8e66e2f2f51f4f833412561cf`.
At 17:29 UTC on 2026-10-13, the unmodified Windows 7 loader passes its old
failure under Firecracker, displays Boot Manager and selects normal startup.
The subsequent boot stage remains unresolved. A separate Server 2025 copy
boots with the same firmware, accepts an authenticated SAC login, writes and
reads `C:\fc-zero-page.txt`, and requests ACPI S5. Firecracker exits with code
0 at 17:30:37 UTC. That VM's disposable disk and FIFO are removed. Logs are
retained under `build/windows/legacy/{win7,modern}-zero-page` on the host.

The debugger build subsequently reaches the Windows 7 kernel and records
`KiBugCheckData = {0x79, 6, 0, 0, 0}`. Matching HAL symbols identify the check
in `HalpInitMpInfo`: bit zero at MADT offset 40 is clear.
The live table has signature `APIC`, length 72 and flags zero. ACPI defines
that bit as `PCAT_COMPAT`, for the dual 8259 controller. Firecracker creates
and snapshots both KVM PICs, but its MADT constructor sets all flags to zero.

- [x] Add a failing regression for the x86 MADT/PIC declaration.
- [x] Declare the existing x86 PICs in MADT, preserving ARM flags zero.
- [x] Repeat the Windows 7 kernel boot and trace any next failure.
- [x] Check AMD64 Linux and modern Windows for interrupt regressions.

The matching kernel base in this run is `0xfffff80002a1f000`, with
`KiBugCheckData` at RVA `0x266780`. HAL base is `0xfffff80002ffc000`, and
`HalpApicTable` is at RVA `0x2a160`. Its pointed-to table is
`0xffffffffffd03018`. GDB logs are in remote `build/windows/legacy/win7-gdb`.

The MADT regression fails before the change (flags 0 rather than 1) and
passes afterwards, including the emitted table checksum. All five selected
ACPI tests and four x86 VM/IRQ-chip state tests pass. Clippy passes for VMM
and the ACPI library. Linux uses MADT for two CPUs, switches to symmetric
I/O APIC routing, mounts its virtio PCI root disk and produces periodic
`LINUX_ALIVE` output. Server 2025 boots with default seccomp, accepts SAC
authentication, writes and reads a file, and requests S5; Firecracker exits
with code zero at 17:52:30 UTC. Both regression VMs and disposable disks are
removed. The initial Linux initrd attempt had no usable `/init`; the ext4
root-disk check supplies the successful boot evidence.

With the corrected MADT, Windows 7 passes `HalpInitMpInfo` and then waits in
`HalpPmTimerTickCountStall` (HAL RVA `0xe1a0`). `HalpCurrentTimePort` (RVA
`0x29e88`) is zero, matching the absent legacy PM timer fields in FADT. In
this run HAL is based at `0xfffff80002a1b000`; the stopped instruction at
`0xfffff80002a291e1` reads I/O port zero. A debugger-only change of the port
to the existing firmware timer at `0x608` reaches `HalpWaitForPhase0ClockTick`.
It does not establish successful timer calibration or a Windows boot. The
debug VM is stopped. Its log is `build/windows/legacy/win7-madt/debugger.log`
on the Linux host, copied locally as `win7-madt-debugger.log`.

- [x] Define conventional x86 ACPI timer/power registers and snapshot semantics.
- [x] Add focused register/FADT regressions before implementing that interface.
- [x] Preserve KVM legacy interrupt routes when installing device/MSI routes.
- [x] Reproduce lost IRQ 0 delivery through the in-kernel PIC and I/O APIC.
- [x] Implement and test RTC interrupt delivery and snapshot state.
  - [x] Reproduce writable RTC interrupt-enable bits before implementation.
  - [x] Implement periodic, update and alarm flags, read-to-clear status and IRQ 8.
  - [x] Preserve divider phase and pending flags in snapshots.
  - [x] Reserve IRQ 8 before allocating platform/device interrupts.
  - [x] Verify register/event/KVM behaviour, then boot Windows 7.

RTC register semantics follow the
[DS12885/DS12887 datasheet](https://www.analog.com/media/en/technical-documentation/data-sheets/ds12885-ds12c887a.pdf).
Periodic flags latch regardless of interrupt enable. Register C clears
all event flags on read. SET freezes calendar updates and clears UIE,
without disabling periodic interrupts. Divider reset inhibits events.

- [x] Boot unmodified Windows 7 with the conventional ACPI interface.
- [x] Repeat modern AMD64 Windows, Linux and full snapshot checks after the change.

The conventional timer must follow the [ACPI PM timer register model](https://uefi.org/specs/ACPI/6.5/04_ACPI_Hardware_Specification.html#pm-timer-register),
including `TMR_STS` on each most-significant-bit transition, `TMR_EN` and
SCI delivery. Publishing the current firmware-only read port is insufficient:
its `Instant` restarts on process creation, and it has no event or snapshot
state. An ACPI-only platform may keep `SCI_EN` set without an SMI command.
The implementation must preserve register and timer state across snapshots,
provide the conventional S5 control encoding, and declare the actual SCI
interrupt routing. ARM must retain hardware-reduced ACPI.

The conventional PM implementation passes nine ACPI tests, including timer
MSB transitions, SCI resampling and register restore. A KVM regression first
fails because IRQ 0 no longer reaches the PIC after `set_gsi_routes`. Restoring
[KVM's default legacy routes](https://github.com/torvalds/linux/blob/v6.12/arch/x86/kvm/irq_comm.c)
passes that regression and all 15 VM tests. Eight device-manager and four
x86 IRQ-chip state tests also pass. The matching duplication scan remains
38 clones and 499 duplicated lines.

Unmodified Windows 7 reads PM timer port `0x608` and next stops with
`0x5c, 0x10b, 1, 0, 0`. Matching HAL symbols identify source 1 as its RTC
clock, and the failing call waits for a phase-zero clock interrupt. The
CMOS implementation masks RTC interrupt-enable bits and has no IRQ output.
The stopped diagnostic VM is shut down; this is not a completed boot.

The RTC implementation passes seven register/event tests, 12 resource
allocator tests and the IRQ 8 delivery test against KVM's slave PIC. Clippy
passes with warnings denied, and the duplication scan remains 38 clones and
499 duplicated lines. It reserves IRQ 8 and adds a PNP0B00 device. Snapshot
format 14 adds divider ticks and the last calendar update to CMOS state.
AMD64 full device restore now passes; ARM runtime regression remains pending.
The divider restart test first failed, then passed after implementing the
500 ms restart delay and the eight-tick update-in-progress interval.
Snapshot state retains the next calendar update and periodic divider origin.
The full restore checks below validate this state in a new process.

With that candidate, unmodified Windows 7 passes the earlier HAL failure
and reaches `INACCESSIBLE_BOOT_DEVICE` (`0x7b`). Both the debugger build and
a default-seccomp production build reproduce it. The installed `viostor`
61.77.104.17200 driver has Start=0 and CriticalDeviceDatabase entries for
both `1af4:1001` and `1af4:1042`. Its SHA-256 is
`ac407d84ca819642cab837ae3b5b9c8aba80204daeaec6818e16f56b2cdf73a3`.
A temporary PCI write trace shows firmware initialisation and reset, but no
subsequent driver status writes. This does not establish the cause of the
storage failure. Both VMs are stopped and the temporary trace is removed.

- [x] Reproduce the GDB all-stop hang with a two-CPU Linux guest.
- [x] Stop every CPU on debugger interrupt, initial stop and breakpoint.
- [x] Verify thread/register reads, resume and a breakpoint on CPU 1.

The original debugger stopped only CPU 0 on interrupt. `info threads` then
hung reading CPU 1 registers. The corrected build reads both CPUs at the
initial stop, after an interrupt and after a hardware breakpoint on CPU 1.
Linux serial ticks continue after resume. GDB-feature Clippy passes with
warnings denied; duplication remains three clones and 28 lines. Logs are
in `build/windows/gdb-allstop`; the reproduction VM is stopped afterwards.

The corrected debugger shows `viostor.sys` and `storport.sys` in the loaded
module list. Bugcheck parameters are `7b`, a Unicode boot-path descriptor,
`c0000034`, zero, zero. The path is
`\ArcName\multi(0)disk(0)rdisk(0)partition(1)`.
Matching kernel PDB types identify the PCI root device node as
`ACPI\PNP0A08\0`, state `303`, problem `0c`, with no children and no
assigned resource list. The RTC node is started with problem zero.
Thus PCI enumeration is blocked by root-bus resource allocation, before
the storage device can bind to its loaded driver.

- [x] Reproduce the invalid ECAM reservation in generated AML.
- [x] Move ECAM out of the PCI root `_CRS` into a PNP0C02 motherboard reservation.
- [x] Test generated resources on both architectures and boot Windows 7 again.

The current PCI root `_CRS` includes a fixed memory descriptor for ECAM
at `eec00000..eecfffff`. The Windows arbiter also reserves that MCFG range.
The [PCI firmware requirements](https://www.kernel.org/doc/html/latest/PCI/acpi-info.html)
place ECAM in a motherboard reservation and exclude it from root-bus `_CRS`.
This is a confirmed table defect; a corrected boot must establish whether
it explains this resource conflict. No guest patch or driver change was made.
Logs and the binary resource-requirements list are in
`build/windows/legacy/win7-rtc`. The diagnostic VM is stopped.

The ECAM regression fails before the change and passes afterwards. Each
reservation needs a unique `_UID`: Windows 7 rejects the first candidate
with `a5, 0d, <device>, 4449555f, 0` (`_UID`). Adding `ECAM0000` resolves
that rejection, but the PCI root still has Code 12 and no children.
Five PCI tests pass at this stage. Final checks below include the UID
and I/O window corrections.

The Windows arbiter records a second conflict: the PCI root I/O producer
window `0000..0cf7` overlaps the started RTC device's `0070..0071` allocation.
The RTC allocation has attributes `02`, whereas the other fixed legacy
allocations are boot-reserved (`01`). The PCI window is marked conflicting.
The PDO resolves back to `ACPI\PNP0B00`, using matching kernel PDB types.

- [x] Reproduce the PCI I/O aperture covering legacy platform ports.
- [x] Restrict the x86 PCI I/O aperture to `1000..ffff` and test the generated AML.
- [x] Repeat Windows 7 boot, storage and shutdown; repeat Server full restore.
- [x] Verify Windows 7 full and differential snapshots and conventional egress.

The first Server regression also stops with `INACCESSIBLE_BOOT_DEVICE` and
requests reset. Its second boot reproduces that error. A control build with
the new RTC and the old ECAM layout enters recovery on that already-failed
disk, so it does not isolate the cause. The disposable disk is replaced
from the verified baseline before testing the I/O correction. The baseline
remains unchanged. No snapshot success is claimed for these candidates.

The corrected PCI aperture passes its red-to-green regression and all six
PCI segment tests. On 13 October at 19:30 UTC, unmodified Windows 7 reaches
the `win7\build` user session. Its startup probe records version 6.1.7601,
the running process list and a disk-write marker. Firecracker records ACPI
S5 and exits zero. The proof file is recovered through a read-only mount;
no inspection mount or loop remains. Evidence is retained under
`build/windows/legacy/win7-storage-boot`.

The same production build boots Server 2025 with default seccomp. SAC
authentication, writing and reading `C:\fc-rtc14.txt`, a full format-14
snapshot and restore in a new process all pass. The restored session reads
the saved file and emits `FC_RTC14_RESTORED`. ACPI S5 exits zero at 19:34:39
UTC. Linux also boots and resumes serial ticks after a full restore.
The disposable Server disk, snapshot memory and Linux test disk are removed.

Seven RTC tests, 23 legacy-device tests, 12 allocator tests and all 18
device-manager tests pass. The broader manager check first caught one test
counting reserved IRQ 8 as available, and two persistence fixtures missing
the x86 Hyper-V field. Those expectations are corrected. PCI duplication
is 12 clones and 102 lines; the RTC/manager scope has 38 clones and 504 lines
(five additional lines within the existing duplicated persistence fixtures).
Production AMD64 SHA-256:
`557bf9601a2c6c3f289771229182767c865bfe80a2e9fc9ea4dc52415d211698`.
Final AMD64 and ARM64 VMM Clippy checks pass with warnings denied. Source
commits are `fe083eb6f` (PCI resources), `9bee2491c` (RTC) and `a2c62d525`
(persistence test fixtures). No push or merge is performed.
XP platform support, legacy vsock, and final-format ARM64 and
modern egress regressions remain required before completion.
Windows 7 passes its conventional egress checks after cold boot, full
snapshot restore and differential snapshot restore on 13 October. Each
stage resolves DNS, receives HTTPS status 200 over TLS 1.2 with certificate
validation enabled, and compares 2,097,152 echoed TCP bytes. The independent
host listener records all three complete transfers and guest PASS reports.
Restores use new Firecracker processes with the same disk and isolated TAP;
the differential memory is rebased onto the full snapshot. Connections are
opened afresh after each restore. The native WinHTTP probe avoids PowerShell
2 COM binding and redirected-output issues. Its payload generator uses the
guest's .NET 3.5 interface without calling newer cleanup methods. Test logs
and probe sources are retained under `build/windows/legacy/win7-network`.
The restored guest requests ACPI S5 and exits zero at 20:06:03 UTC. All
temporary network rules, the namespace, listener and snapshot files are
removed. The disk and logs remain. Harness duplication returns to zero
after extracting the shared TCP echo loop; both Linux helpers compile with
warnings denied.
The debugger's `info threads` request also stalled on the stopped two-vCPU
guest; that separate issue needs a reproducer and fix.

The first modern Windows regression stops with `0xa5, 0x11, 3, 0, 0`.
CloudHv's ACPI installer omits FACS; EDK II's ACPI table protocol clears
FADT's FACS pointer unless that structure is installed separately. The
firmware patch now installs the supplied FACS. Windows Server 2025 then
boots, authenticates through SAC and writes/reads `C:\fc-pm.txt`. Full
snapshot restore preserves the authenticated session and the file; the
guest also prints `FC_PM_RESTORED`, then requests S5 and exits with code 0.
Linux cold boot and full snapshot
restore pass with PM state. Final Clippy passes with warnings denied, all
25 ACPI-table tests pass, and the ACPI library compiles for ARM64 musl.
The AMD64 production binary is
`79cad29bcd0f4df35b6b534f60196df7bd3fb761874b4ea79f2cb8f7dd968a87`;
the firmware is
`8220806a388141650ca1c5280863a9085f2d969de9af839519d231cbd4c7286b`.

The MADT commit's production AMD64 musl binary has SHA-256
`2994e8756411d3c890421c1f81cc579ee11b44044be0df07f463c8aa1d3333c5`.
The ACPI library also passes an ARM64 musl compile check. No test VM or
debugger remains active after this phase. Superseded debugger/production
binary copies are removed; the current release and current debugger remain.

Toets supplies both installation ISOs in the local macOS application-data
store. Their SHA-256 digests match `toets/guests/xp.toml` and
`toets/guests/win7.toml`: XP SP3 x86 is
`fd8c8d42c1581e8767217fe800bfc0d5649c0ad20d754c927d6c763e446d1927`;
Windows 7 Enterprise SP1 x64 is
`ee69f3e9b86ff973f632db8e01700c5724ef78420b175d25bae6ead90f6805a7`.
Both Toets configurations currently use SeaBIOS. Their golden qcow2 images
are retained unchanged. Firecracker currently supplies hardware-reduced
ACPI and modern-only virtio PCI devices with MSI-X, without legacy INTx.
These are compatibility questions to resolve, not evidence of legacy boot.

The pinned XP x86 INF files select only legacy PCI IDs: storage `1af4:1001`
(driver 51.64.104.4800) and networking `1af4:1000` (51.64.104.4900).
Firecracker exposes `1af4:1042` and `1af4:1041`, respectively. The pinned
Windows 7 AMD64 drivers (61.77.104.17200) list both modern IDs and enable
MSI-X. Thus XP needs a transport/driver change as well as a boot path;
Windows 7's modern driver declarations alone do not prove platform support.
A fresh XP install is running through Toets in the separate remote store
`build/windows/legacy/toets-store`, reusing its pinned media and QEMU without
changing the original Toets store. Firecracker boot has not been attempted.

Both fresh installations subsequently report `BUILD OK` and exit with status
0: XP at 15:57 UTC and Windows 7 at 16:01 UTC on 2026-10-13. Toets
`image check xp` and `image check win7` pass. Their read-only qcow2 images
occupy about 956 MiB and 4.2 GiB. Installation logs are retained in the
isolated store. These are QEMU installation results, not Firecracker boot
or device-compatibility evidence.

A disposable Windows 7 disk is converted to GPT with a 260 MiB EFI system
partition. `bcdboot` creates the UEFI files, and EMS is enabled in its BCD.
Its original Windows 7 EFI loader starts under Firecracker but reports
`BlInitializeLibrary failed 0xc0000225` before Windows starts. The same
disk and loader fail identically under QEMU/OVMF, both without graphics and
with standard VGA. This does not isolate the failure to Firecracker or
establish its cause. All three failed test VMs are stopped. The utility
Server Core VM also stops, and its disposable disk is removed. Local
evidence is `build/windows/legacy/win7-firecracker-serial.log` and
`win7-bcd-prep-serial.log`. Windows 7 Firecracker support remains unverified.

OVMF 2025.02 from Ubuntu's `ovmf_2025.02-3ubuntu2_all.deb` gets past that
loader error with the same Windows 7 disk, including without VGA. The VGA
control reaches the Windows startup animation, but does not establish a
completed boot. A separate CloudHv firmware worktree restores upstream UGA
code and enables `PcdConOutUgaSupport`; the same loader error remains.
The experiment is stopped and its build worktree removed. Its logs and
source diff remain under `build/windows/legacy`; the verified production
firmware retains its recorded SHA-256 digest.

The fresh XP disk is also tested with Firecracker. CloudHv discovers its
virtio disk but reports `No bootable option or device was found`, consistent
with the missing legacy BIOS path. The VM is stopped and its temporary raw
disk removed. The checked qcow2 image remains in the isolated Toets store;
serial evidence is `build/windows/legacy/xp-firecracker-serial.log`.

### Windows vsock follow-up

- [x] Establish the vsock/tool duplication baseline: 32 clones, 231 lines.
- [x] Inspect the signed Windows driver and its userspace socket interface.
- [x] Install the driver in an isolated copy of the verified Windows disk.
- [x] Reproduce indirect RX descriptor rejection in a focused unit test.
- [x] Add bounded indirect descriptor traversal and advertise vsock support.
- [x] Run shared virtio tests, Clippy and the musl release build.
- [x] Verify host-to-guest and guest-to-host payloads through Firecracker.
- [x] Validate indirect reset-event buffers and reject short/read-only buffers.
- [x] Verify fresh connections after snapshot restore.
- [x] Record the procedure, test evidence and remaining limitations.
- [x] Stop AMD64 test VMs, remove stale snapshots and commit the vsock changes.

- Firecracker already loads ELF images with `XEN_ELFNOTE_PHYS32_ENTRY`.
- EDK II CloudHv uses `hvm_start_info.rsdp_paddr`; Firecracker leaves it zero.
- CloudHv firmware identifies PCI host bridge device `0x0d57`, already used
  by Firecracker, and reads the ACPI timer at I/O port `0x608`.
- Existing Firecracker snapshots exclude several Hyper-V MSRs. This requires
  explicit handling before Hyper-V guests can use snapshots.
- `cargo check -p vmm -p firecracker --tests -j2` and Clippy with
  `-D warnings` pass on the AMD64 host. The debug build reports the existing
  empty-default-seccomp build-script warning.
- Five Hyper-V tests pass, including SynIC/timer restore and resnapshot.
  Two ACPI power/timer tests and the PVH ACPI handoff test pass.
  The five machine-configuration parser tests pass in both Firecracker
  binary test targets.
- All 23 `acpi_tables` tests pass locally on macOS, including the FADT
  power-register serialisation test.
- `rustc --edition 2024 -D warnings tools/windows/prepare.rs` passes locally
  and on the AMD64 host. Preparation and no-overwrite checks pass on the host.
- The architecture/ACPI/PCI duplication check still reports 58 clones and
  650 duplicated lines. The final expanded scan, including block I/O, io_uring
  and CMOS, reports 142 clones and 1,572 duplicated lines, unchanged from
  the same paths at the branch base (6.45% before, 6.23% after).
- Messages to both Tokoloshe tasks failed with an active-writer error; no
  coordination message has been delivered.
- The musl release binary boots patched CloudHv firmware and activates
  modern virtio PCI block storage. The firmware finds the installed Windows
  EFI loader. The first Windows attempt fails before the kernel with
  `BlInitializeLibrary failed 0xc0000185`; CMOS ports `0x70/0x71` are missing.
- Windows Server 2025 Standard Evaluation Core installs and reaches login
  under QEMU. EMS settings were reset by the subsequent `bcdboot` step;
  the template now creates EFI boot files first. The installed disk was
  corrected, EMS verified, and QEMU shut down cleanly before Firecracker use.
- Adding the CMOS/RTC interface removes the EFI loader error. The next boot
  reaches an unhandled `KVM_EXIT_HYPERV_HCALL`. Unsupported userspace
  hypercalls now return `HV_STATUS_INVALID_HYPERCALL_CODE` to the guest.
- CMOS format/time-setting tests, snapshot format tests and legacy device
  registration tests pass. CMOS state changes the snapshot format to 12.0.0.
- Windows now starts its AMD64 kernel (build 26100) and SAC under Firecracker.
  It reports `NTFS_FILE_SYSTEM` (`0x24`). The same disk reaches login under
  QEMU. A parser regression test reproduces treating a second data descriptor
  as status. Both I/O engines now support multiple data segments; the block
  suite and a vectored read/write and dirty-tracking test pass.
- With scatter/gather block I/O, Windows reaches the running SAC command
  channel under Firecracker. `id` reports `FCWINDOWS`, AMD64, Windows Server
  build 26100. SAC `shutdown` triggers ACPI S5 and Firecracker exits with code 0.
  All 37 block tests and Clippy with warnings denied pass.
- Windows boots with asynchronous storage and NetKVM after enabling EDK II
  lightweight enumeration over preassigned PCI resources. Authenticated SAC
  commands write/read a file and ping the isolated host TAP with no packet loss.
- Windows full snapshot restore preserves the authenticated command session,
  stored file, network traffic and `hyperv: true` machine configuration.
- Linux 6.18.38 boots with `hyperv: false` and a separate ext4 root. Its init
  counter continues after full restore and after differential snapshot rebase
  and restore. The source kernel was read from Tokoloshe; no source assets
  were changed.

- Windows differential snapshot restore also preserves the command session,
  file and network. Creating a new full snapshot after that restore succeeds.
  Guest wall time lags by the pause interval without time synchronisation;
  the guide records this limit.
- Guest `shutdown /r /t 0` exits Firecracker with code 0. A new Firecracker
  process boots the same disk back to SAC with the final musl binary.
- The final PCI capability test and Clippy pass after rejecting invalid
  capability access lengths.

- A fresh installation confirms that `bcdboot` invalidates the current
  boot-entry identifier during Setup. The log records exit code 1 for the
  implicit EMS commands. The template now targets `{default}` and `{bootmgr}`
  explicitly. WinPE already stages both virtio drivers; the redundant later
  driver command has been removed.

- The corrected unattended installation reaches Server Core login with SAC
  and CMD available without manual BCD changes. After QEMU shutdown, the
  fresh disk boots Firecracker, accepts the generated administrator login,
  writes and reads a file, and shuts down through ACPI S5 with exit code 0.
  Test VMs are stopped and the isolated TAP has been removed.

## Vsock evidence

- The Server 2025 AMD64 package from virtio-win 0.1.302 installs as
  `VirtioSocket` with `VirtioSocketWSP` running. No guest NIC is attached.
- The original packet path rejects Windows indirect RX buffers with
  `UnwritableDescriptor`. A focused reproducer fails before the change and
  passes with bounded descriptor traversal. Mixed direct/indirect TX buffers,
  unaligned tables and malformed-chain rollback are covered.
- All 346 virtio tests pass serially with the required privileges in an
  isolated host network namespace. Running without those privileges fails
  network-device setup; parallel vsock tests race on shared metrics.
- Host-to-guest transfer sends 2,097,152 random bytes to a Windows listener.
  A guest-initiated connection returns the stored file; the host checks every
  byte and sends an acknowledgement received by Windows.

## Test assets

The isolated host directory is
`/home/jacquesg/dev/bloudraad/firecracker-windows-guests`. Runtime logs are
under `build/windows/`: `runtime-light`, `windows-restore`,
`windows-diff-restore`, `windows-reboot`, `linux-ext4`, `linux-restore` and
`linux-diff-restore` and `windows-verified`, each with a `-serial.log` suffix. These are local test
artifacts, not repository inputs.

| Asset | SHA-256 |
| --- | --- |
| Microsoft Windows Server 2025 evaluation ISO | `7b052573ba7894c9924e3e87ba732ccd354d18cb75a883efa9b900ea125bfd51` |
| Fedora virtio-win stable ISO | `303f7ae40dad495d6ae474fdc571df58958a4dbc5c37a522d80f9a203867949d` |
| Patched EDK II `ch-97eeb7b09`, GCC release firmware | `fc8cbaf16d77721623691670c9b152d40d446a01ae8217fa99519d2798fee91d` |

References: [Cloud Hypervisor Windows guide](https://github.com/cloud-hypervisor/cloud-hypervisor/blob/main/docs/windows.md),
[EDK II CloudHv](https://github.com/tianocore/edk2/tree/master/OvmfPkg/CloudHv),
[PVH ABI](https://github.com/xen-project/xen/blob/master/docs/misc/pvh.pandoc).

## ARM64 and egress follow-up

- [x] Validate Windows reset handling without an event-queue kick.
- [x] Verify allowed and denied tunnels through the real Tokoloshe proxy.
- [x] Verify egress reconnection after restore and exact traffic accounting.
- [x] Inspect ARM64 firmware, ACPI, interrupt and guest driver requirements.
- [x] Establish the ARM64 duplication baseline before implementation.
- [x] Implement and test the ARM64 firmware boot path.
  - [x] Reach UEFI and load an ARM64 EFI shell from a virtio PCI disk.
  - [x] Add opt-in PMUv3 initialisation, interrupt description and restore tests.
- [x] Obtain official ARM64 installation media and prepare a test disk.
  - [x] Obtain and inspect the official ARM64 evaluation ISO.
  - [x] Prepare and validate an unattended ARM64 installation.
- [x] Validate ARM64 boot, devices, vsock and snapshot restore on KVM.
- [x] Run Linux regressions and record platform limitations.

### Restore and CONNECT evidence

- A Windows reset reproducer fails because the device waits for an event-queue
  kick that Windows does not send. Virtio 1.2 section 5.10.6.7 requires a CID
  reread after reset. That read now schedules the receive drain, while invalid
  or empty configuration reads do not acknowledge reset.
- The existing Windows listener accepts 2,097,152 bytes after full restore.
- The real Tokoloshe sidecar accepts a granted echo endpoint and rejects the
  adjacent port, both before and after restore. Each accepted tunnel checks
  2,097,152 bytes in both directions; sealed accounting matches all four
  outcomes and payload totals. No guest network device is attached.
- The isolated sidecar source was copied from Tokoloshe at local HEAD
  `3125a2bdd621371206cd0d4b7b50ed801a278cf8`, including current working files.
  The Windows probe calls the installed Winsock provider directly. This test
  does not imply that the full Tokoloshe guest agent is ported to Windows.

- The final vsock suite passes 81 tests; Clippy denies all warnings. The
  Rust duplication scan remains at 42 clones and 321 duplicated lines.

### ARM64 test environment

- [x] Establish an isolated appliance and duplication baseline.
- [x] Implement firmware loading, memory handoff and ARM ACPI tables.
- [x] Build patched EDK II and validate virtio PCI EFI disk discovery.
- [x] Add PMUv3 initialisation, interrupt descriptions and restore tests.
- [x] Validate musl/seccomp and Linux full/differential restore.
- [x] Build native Windows probe and ARM64 Tokoloshe egress sidecar.
- [x] Complete Windows installation and preserve a clean disk baseline.
- [x] Boot Windows under Firecracker with PMUv3 and default seccomp.
- [x] Verify CONNECT allow/deny and payloads before and after restore.
- [x] Complete ARM64 documentation, final checks and atomic commits.

- An isolated copy of the Tokoloshe development appliance runs with 8 GiB
  RAM on the Apple M5 Max. Linux `7.2.8-tokoloshe-host-8` exposes `/dev/kvm`.
  The original appliance was not changed. Local test assets are under
  `build/windows/arm64-host`; its SSH forward is localhost port 18322.
- The virtio-win ISO includes ARM64 Windows 11 storage and vsock drivers.
- The expanded ARM64 duplication baseline is 70 clones and
  926 duplicated lines across 43 Rust files.
- Implementation requires a bounded firmware loader, an ARM firmware memory
  handoff, ACPI GIC/timer/serial tables and firmware PCI resource handling.
  The existing 16550 UART can be described by SPCR and supported by EDK II's
  16550 library; validate this before adding another UART device.
- Microsoft Windows 11 IoT Enterprise LTSC 2024 Evaluation image 1 is ARM64,
  build 26100.1742. The ISO is 5,042,194,432 bytes with SHA-256
  `3dcdba9c9c0aa0430d4332b60c9afcb3cd613d648a49cbba2d4ef7b5978f32e8`.
  It came from Microsoft's Evaluation Center download, not a third-party image.
- Patched ARM64 CloudHv firmware builds and prints through Firecracker's
  existing MMIO 16550 UART on nested KVM. Its first memory-initialisation
  assertion identified the Linux-specific exclusion of the first 2 MiB in
  the FDT. Firmware boot now describes all RAM; EDK II reserves the platform
  area in its memory map. The next boot reaches DXE and PCI discovery.
- PCI enumeration and the Windows ARM64 installation are not yet validated.
  Current diagnostic boots use the GNU test binary with seccomp disabled;
  the final musl build and seccomp validation remain required.
- Both ARM64 FDT tests pass inside the appliance, including the new
  Linux/firmware memory-handoff regression. With cached PCI access scoped
  to the PCI bus and host-bridge drivers, debug firmware reaches the boot
  manager and reports no bootable device, as expected for the diskless test.
  Enumeration with a virtio disk remains to be checked.
- A subsequent KVM boot loads `EFI/BOOT/BOOTAA64.EFI` from a read-only
  virtio PCI FAT disk and reaches the interactive shell. This validates
  firmware PCI disk discovery, block reads and EFI image loading.
- The ARM64 preparation mode compiles with warnings denied and generates
  valid XML and shell syntax. AMD64 generation and no-overwrite checks pass.
  ARM64 Windows Setup loads the storage driver and begins unattended
  installation under QEMU TCG. Installation completion remains unverified.
- A separate diskless QEMU KVM probe isolates the Windows bootloader fault:
  `ESR_EL1=0x02000000` (undefined instruction), with ELR pointing to
  `mrs x8, pmcr_el0` (`0xd53b9c08`). The appliance exposes only software and
  breakpoint perf event sources; QEMU cannot enable a host PMU. Firecracker
  currently rejects `KVM_ARM_VCPU_PMU_V3`. PMU support and a PMU-capable
  validation host are required before Windows ARM64 KVM boot can be claimed.
- The PMUv3 implementation uses the existing CPU-template vCPU feature bit.
  It configures PPI 23 and initialises the PMU after the GIC exists. Restore
  recreates that configuration from the saved vCPU feature flags. FDT and
  MADT describe the PMU interrupt only when the feature is enabled.
- A second isolated Tokoloshe appliance runs under QEMU TCG with EL2 and
  PMUv3 exposed on the AMD64 host. Its Linux KVM supports the new PMU test:
  initialisation, non-default selector state and snapshot restore pass.
  The absent-PMU rejection path passes on the Mac appliance. Running timer
  counters are excluded from the stored-register equality check.
- The ARM64 VMM test build and Clippy with warnings denied pass. The ACPI
  unit tests include GIC affinity and optional PMU interrupt encoding.
- Firmware now honours preassigned PCI resources. A KVM boot with two
  virtio block devices and vsock attached reaches the EFI shell and maps
  both FAT filesystems. The ARM64 source patch is in `resources/firmware`.
- The ARM64 musl release build succeeds using Zig for musl C compilation
  and the AArch64 GNU linker. It reaches UEFI with the default
  seccomp policy enabled. The GNU diagnostic binary is no longer required
  for boot validation.
- Firmware-loader tests validate the handoff address, byte preservation,
  checksum/header rejection and size bounds. A malformed volume is rejected
  before modifying guest memory.
- ARM64 Linux direct boot, full restore, differential restore after rebase,
  and a subsequent full snapshot pass with the musl binary and seccomp.
  The initramfs counter continues from 52 to 79 after full restore and from
  79 to 95 after differential restore. The regression VMs are stopped.
- A native ARM64 Windows CONNECT probe builds with Rust's
  `aarch64-pc-windows-gnullvm` target and LLVM-MinGW 20260908. The read-only
  tools disk contains that probe, a 2 MiB payload and the signed ARM64
  viosock driver. Its Windows execution remains to be validated.
- The AMD64 VMM ACPI regression suite passes all eight tests after the ARM64
  changes. The common FADT builder already sets `HW_REDUCED_ACPI`.
- Windows ARM64 reaches first-run setup and an administrative command prompt
  under QEMU TCG. The answer template now sets the installed system's locale
  in `oobeSystem`, in addition to the separate Windows PE locale settings.
  The updated preparation tool compiles with warnings denied; XML parses.
- The real Tokoloshe egress sidecar and its test harness build for ARM64 Linux.
  The PMU-capable appliance has an isolated, non-root test directory for the
  sidecar and Firecracker. Windows execution and egress remain unverified.
- The updated ARM64 Clippy check passes with warnings denied. The firmware
  loader regression passes after converting checksum iteration to fixed-size
  array slices. The release firmware builds with SHA-256
  `93692b58448e1754c5f6cf89ca4e361c54d90985e1461130b5d76d7ea491a74a`.
- Windows registers the SYSTEM startup probe task and reports it running.
  First-run setup reports `OOBEREGION` and `OOBEKEYBOARD` errors under TCG;
  its built-in Skip action advances each page. Installation completion and
  the probe transcript still require verification.
- First-run setup subsequently reaches the Windows lock screen. Login,
  probe transcript inspection and clean shutdown remain pending. The guest
  remains in the isolated QEMU preparation VM; no live disk copy was made.
- The generated `FcAdmin` password is accepted and Windows displays Welcome.
  The desktop subsequently renders black with QEMU's `ramfb`; command access
  and the probe transcript are still being checked.
- A scan covering every changed Rust file and its ACPI/ARM/PCI dependencies
  reports 53 clones and 603 duplicated lines both at the committed AMD64 base
  and in the ARM64 worktree (40 and 42 Rust files respectively). Reports are
  under `build/windows/arm64-final-duplication-{before,after}`.
- Windows completes first-login setup and reaches the desktop. The scheduled
  task transcript confirms SYSTEM execution and successful staging of
  `viosock.inf` as `oem2.inf`. Native CONNECT success still requires the
  Firecracker vsock device and is not claimed by this preparation test.
- `shutdown /s /t 0` applies pending updates and exits QEMU. Read-only NTFS
  inspection reports volume flags `0x0000`. The original 64 GiB sparse disk
  remains in the preparation appliance; a separate sparse copy is being
  transferred to the PMU-capable Firecracker appliance.
- The first Windows Firecracker attempt loads Boot Manager, then faults on
  `mrs x9, PMCCNTR_EL0` at image RVA `0x33004`. Its JSON enabled feature bit
  2 (PSCI), not bit 3 (PMUv3). The configuration and guide now use `0b1xxx`;
  the PMU test parses that JSON instead of constructing the feature mask.
  The earlier firmware boot therefore did not validate PMU exposure.
- The initial native ARM64 probe imported `libunwind.dll`, which was absent
  from the tools disk. Rebuilding with `-C target-feature=+crt-static` removes
  that dependency. The isolated working disk contains the corrected executable
  and captures each native exit code; the preparation baseline is unchanged.
- Snapshot diagnostics also exposed missing zero padding in ARM register
  formatting. Each byte now prints two hex digits, preserving register values.
- EDK II identifies PL031 with byte reads of its identification registers.
  The RTC wrapper rejected those reads. The regression failed before the fix;
  all three RTC tests now pass and the next firmware boot has no RTC errors.
  ARM64 Clippy passes with warnings denied. The expanded duplication scan,
  including RTC, remains at 53 clones and 603 lines (41 baseline/43 current files).
- With PMUv3 enabled, Windows faults during the firmware context transition:
  `TCR_EL1=0x5b5113511` selects four translation levels, while the saved
  `TTBR0_EL1=0x17ffff000` points to the firmware's five-level table. Snapshots
  `windows-boot2` and `windows-boot3` preserve the instruction translation fault.
  QEMU/KVM on the same appliance reaches Windows kernel execution.
- Setting `ID_AA64MMFR0_EL1.PARange` to 5 (48 bits) through the existing CPU
  template gets Firecracker past that transition into Windows kernel execution.
  Snapshot `windows-boot4` confirms the register change and kernel PC
  `0xfffff8001fa60fa4`. User-space startup, CONNECT and restore remain pending.
- A fresh installation with the corrected locale template reaches Windows
  Setup's installation stage without manual partition or account entry.
  Completion of first-run setup remains pending.
- The 48-bit Firecracker boot activates the Windows storage driver, then
  activates the ARM64 vsock driver at 11:17 UTC. CONNECT is not yet verified.
  The QEMU/KVM comparison reaches user mode and shuts down after an ACPI
  power-button request. Slow emulation must not be mistaken for a guest fault.
- The outer QEMU 10.2.1 process subsequently exits with host `SIGILL` in its
  generated code buffer. Its core, instruction bytes and backtrace are
  preserved in `build/windows/arm64-pmu-host/crash-20261013` on the AMD64 host.
  The cause remains unknown. The isolated appliance is restarted from its
  existing state disk without reformatting. The clean Windows baseline and
  original Tokoloshe appliance are unchanged.
- The independent PL031 fix is committed as `3e3e67f44`.
- The appliance restart succeeds. Firecracker starts a second 48-bit Windows
  boot with a fresh proxy grant in `egress-restart`; the previous working
  disk is preserved as `windows-after-host-crash.raw`. The separate unattended
  installation completes file copying and restarts from Windows Boot Manager.
- The second boot requests `KVM_SYSTEM_EVENT` type 2 (reset), not shutdown.
  No new probe transcript or CONNECT traffic is present. The cause of the
  reset remains unresolved; it must not be counted as a successful shutdown.
- The saved probe task uses `InteractiveToken` for SYSTEM. The test installer
  now creates an explicit `ServiceAccount` principal. Registration and a
  probe run before interactive login still need verification.
- The corrected unattended installation reaches the Windows lock screen
  without the previous locale errors and exits after an ACPI power request.
  Its VM is stopped. The idle proxy harness seals an empty accounting record
  and exits with its sidecar. The temporary preparation-host swap is removed.
- A GDB run reaches the Windows kernel but stalls at guest `brk #0xf002`
  (`0xfffff800bd4013c4`). ARM debug mode enables software-breakpoint trapping,
  while its reinjection function is a no-op. The snapshot confirms the kernel
  base at `0xfffff800bd200000`. The GDB memory walker also reads the wrong
  initial table entry for Windows' 47-bit virtual address space: its first
  index includes a sign-extension bit. An offline walk with an eight-bit
  initial index finds the PE header and code.
  The diagnostic VM, relay and debugger are stopped; their evidence remains
  in `windows-gdb-kernel.{state,mem}` and the GDB session log.
- A KVM regression reproduces the guest BRK trap with software-breakpoint
  trapping disabled as well. The flag alone cannot fix delivery. The debugger
  now forwards the exit syndrome and delivers an AArch64 BRK to the guest's
  EL1 exception vector. The regression compares the resulting PC, PSTATE,
  ELR, SPSR and ESR against normal KVM delivery from EL0t, EL1t and EL1h;
  all three cases pass on the native ARM KVM host. A new diagnostic Windows
  boot uses this build. It is not production boot validation.
- That diagnostic run stops in Boot Manager with status `0xc0000001` after
  repeated failed starts. It does not reach the kernel, so it does not yet
  validate the debugger fix against Windows. ARM64 and AMD64 GDB Clippy checks
  pass with warnings denied.
- The native preparation appliance reports a state-disk write error and XFS
  shuts down the filesystem. Its Windows process is stopped. Normal mount
  log replay restores the filesystem; a subsequent read-only XFS check is
  clean. The Mac had about 6 GiB free when the error was observed.
- Cleanup removes the failed preparation clone, the completed duplicate
  installation disk, the duplicate extracted WIM and old Linux snapshot
  memory files. The native clean Windows baseline, installation media,
  scripts, screenshots and logs remain. The temporary recovery copy is
  removed after the clean filesystem check. Discard reduces the local state
  file from about 38 GiB to 18 GiB of allocated space.
- Remote cleanup removes the superseded AMD64 installation disk, the unpacked
  QEMU core (the compressed Apport report remains), historical ARM snapshot
  memory files and failed working-disk clones. The clean QEMU/KVM comparison
  disk is retained as `/mnt/firecracker-windows/windows-clean.raw`; create a
  new working clone for the next Firecracker boot. Only the GDB kernel memory
  snapshot remains for the unresolved address-translation diagnostic. Small
  snapshot states and logs remain as historical evidence. Old memory paths
  cited above are no longer available.
- The isolated remote appliance is shut down cleanly and restarted with
  `discard=unmap` on its state drive to reclaim deleted blocks on the host.
- The GDB table walker now derives its initial index width from T1SZ. Native
  ARM tests cover 47-bit and 48-bit addresses, block descriptors, invalid
  descriptors and guest BRK delivery; all three tests pass. ARM GDB Clippy
  passes with warnings denied. A fresh diagnostic Windows boot permits live
  reads of the kernel PE header and instructions at KeBugCheckEx.
- The diagnostic run does not hit its KeBugCheckEx breakpoint. A sampled PC
  resolves to KeClockInterruptNotify; this is not proof of user-space boot
  or an explanation of the earlier reset. The debugger also stalls while
  querying all threads. The diagnostic VM is terminated, then its appliance
  is powered off cleanly. Remote QEMU PID 1161474 is confirmed absent. Session
  logs remain; the Windows startup probe and ARM CONNECT validation remain
  pending.
- Review also finds that the existing ARM debugger changes the guest's IRQ
  and FIQ masks whenever debug controls are updated. Diagnostic timing and
  interrupt behaviour cannot establish production boot correctness. Preserve
  the guest masks across debugger single stepping before relying on that
  path for further boot diagnosis.
- The local preparation appliance starts one Windows VM from a fresh clone
  of its clean baseline. Its corrected tools media includes an elevated
  installer, transcript and exported task XML to verify the SYSTEM
  ServiceAccount registration. Installation is not yet verified.
- The preparation VM reaches the desktop. Login succeeds with one-millisecond
  key holds and one-second spacing; faster input was rejected. An elevated
  prompt runs the corrected installer successfully, including its
  ServiceAccount assertion and task XML export. The task reports Running,
  but its last-result field is `0x800710e0` and the probe transcript has no
  new entries. A PowerShell process exists in service session 0; its origin
  is not yet established. A restart is requested to test the corrected task
  before login. Registration alone is not a successful probe run.
- The restart exits QEMU cleanly. Read-only disk inspection finds a new
  transcript at 13:10:27 UTC under `WORKGROUP\SYSTEM`, with the expected
  probe script as its command. The exported task has a boot trigger and
  SYSTEM SID `S-1-5-18`. The installed executable matches static ARM64 probe
  SHA-256 `c194ea2680178037c652f5ae36f9fc9b62b0d72d3cab171e09acc9bf0a590623`.
  This proves a SYSTEM invocation, but it started after interactive login;
  the transcript contains no CONNECT result. The disk is unmounted and the
  same preparation VM is started again without login to test the boot trigger.
- The second boot is left at the lock screen without login. Its transcript
  starts at 13:19:42 UTC as SYSTEM, stages the existing signed driver and
  repeatedly invokes the static probe from 13:20:28. The probe reports that
  the Viosock device is absent; this QEMU preparation VM has no vsock device.
  This verifies unattended execution, not transport. QEMU subsequently exits
  cleanly after power-button requests. The disk is inspected read-only,
  unmounted and retained as `arm64-probe-fix/windows-ready.raw` for transfer
  to the Firecracker validation appliance.
- The source and transferred prepared disks both hash to
  `2b292036fe619142c09e2a3f35f3861b572fd4ce15b59dde08b456c01a698968`.
  Transfer occurs into the powered-off remote appliance's mounted state
  filesystem. Its superseded clean and working disks and the resolved GDB
  memory dump are removed. `windows-ready.raw` is retained read-only and
  `windows.raw` is a writable reflink clone. The state filesystem is trimmed
  and unmounted before appliance restart. The local preparation appliance
  is powered off and its process is confirmed absent.
- A production musl Firecracker run starts from that clone with default
  seccomp, PMUv3, 48-bit PARange and release firmware SHA-256
  `93692b58448e1754c5f6cf89ca4e361c54d90985e1461130b5d76d7ea491a74a`.
  The real Tokoloshe proxy is ready under `egress-ready`, granting
  `127.0.0.1:40567` and denying the adjacent port. No network device is
  configured. Results are pending in `windows-ready*` and
  `egress-ready-accounting.log` on the remote appliance.
- Read-only inspection of a reflink disk copy, taken during a brief VM pause,
  finds current Windows Wininit and Service Control Manager events. These
  establish user-space startup under Firecracker. Several services report
  45-second startup timeouts in this emulated environment. The vsock service
  is installed at 13:48:15 UTC and its device activates at 13:48:42. No new
  startup-probe transcript or CONNECT traffic is present in that inspection.
  The VM is resumed and the temporary copy and loop device are removed.
- The SYSTEM probe starts at 14:08:58 UTC, about 30 minutes after cold boot.
  Sealed proxy accounting records three complete 2 MiB allowed round trips
  and three denied requests. Full restore records five complete allowed and
  five denied requests, plus one interrupted 2,076,672-byte transfer.
  Differential restore records four complete allowed and five denied requests;
  the guest reports the interrupted connection error and then reconnects.
  Restoring a full resnapshot of that VM also succeeds: one complete 2 MiB
  round trip, one denied request and a 151,552-byte transfer interrupted by
  the final test pause. Guest transcripts verify the complete payloads.
  Accounting and guest logs are copied to local `build/windows/arm64-host`.
- Firecracker and the proxy are stopped after sealing the final accounting.
  Intermediate full/differential memory files and disk clones are removed;
  the prepared baseline and latest full resnapshot with its matching disk
  are retained. The state filesystem is trimmed and the appliance shut down.
- Final source review rejects ARM firmware/initrd combinations before any
  initrd copy and omits the unwired ARM legacy PCI interrupt routing table.
  All four initrd tests pass under ARM64 userspace emulation, including
  rejection without memory modification and continued Linux initrd support.
  ARM64 VMM Clippy passes with warnings denied. The expanded review scan
  remains at 55 clones and 619 duplicated lines. No appliance is restarted
  for these source checks; the PCI ACPI change still needs runtime regression.
- Final checks pass 25 ACPI-table tests and four AMD64 VMM ACPI tests;
  AMD64 Clippy denies warnings. The pinned ARM firmware patch passes a
  reverse-apply check. ARM firmware-loader, memory-handoff and initrd tests
  pass under userspace emulation, as do both address-translation tests.
- The PMU initialisation/restore test passes again on the emulated KVM host.
  The BRK comparison fails there: normal delivery yields PSTATE `0x23c5`,
  while reinjection preserves the condition flags and yields `0xa00003c5`.
  Guest PFR1 is `0x21`, without NMI, but normal delivery sets ALLINT. The
  emulated control path also clears NZCV, unlike native ARM KVM and Arm's
  [exception-entry pseudocode](https://www.scs.stanford.edu/~zyedidia/arm64/shared_pseudocode.html).
  The failing comparison is retained; production boot does not use GDB.
- A separate regression reproduces the debugger overwriting IRQ/FIQ masks.
  Removing those PSTATE writes fixes the regression. All four debugger tests,
  including actual single stepping and BRK delivery from three modes, pass
  on native ARM KVM. Three PCI tests and the absent-PMU rejection test also
  pass. The temporary 2 GiB native appliance is shut down after 42 seconds.
  ARM Clippy passes; GDB duplication stays at three clones and 28 lines.
- The final production musl binary starts a new Windows boot at 14:37:58 UTC
  with the initrd guard and corrected PCI ACPI routing. Its isolated remote
  appliance, Firecracker and CONNECT harness remain active for that check.
- Read-only coordination finds the Tokoloshe performance batch waiting for
  98% host idle. Direct messages to both tasks still fail with an active-writer
  error. At 14:59 UTC, Windows is API-paused and its outer QEMU PID 982339 is
  SIGSTOP-suspended, with `Tsl` confirmed. The benchmark batch (controller
  PID 989966, host wrapper PID 992857, idle-gate PID 999252) is left untouched.
  Resume this same emulator after that batch finishes, then resume Windows
  through its API. Do not start a replacement appliance.
- The benchmark wrapper and subsequent host-load diagnostic exit; their
  controller has no remaining child. The same QEMU process and Windows VM
  resume at 15:03 UTC. No benchmark process is stopped or modified.
- Completion audit recovers the Linux direct/full/differential serial logs
  from the native preparation filesystem. They confirm initrd boot, ticks
  53 onward after full restore, ticks 80 onward after differential restore,
  and successful full resnapshot creation. Copies are retained under
  `build/windows/arm64-host/linux-regression`.
- After preserving those logs and the private FcAdmin credential, the native
  preparation appliance is shut down and confirmed terminal. Its redundant
  local root and state files (about 20 GiB allocated together) are removed.
  The verified prepared baseline and matching resumable Windows snapshot
  remain in the remote appliance. The credential is retained with mode 0600
  inside a mode-0700 directory under `build/windows/arm64-host/credentials`.
- The final cold-boot regression passes with the committed ARM platform code.
  Its SYSTEM transcript starts at 15:10:12 UTC, before login. The persisted
  guest transcript contains eight successful 2,097,152-byte payload-equality
  checks and no errors from this boot. Sealed accounting records nine complete
  allowed tunnels and nine denied adjacent-port requests, with no partial
  transfers. The final metrics report zero seccomp faults and zero VMM panics.
  Evidence is retained as `windows-final-probe.log`, `windows-final-metrics.json`
  and `egress-final-accounting.log` under local `build/windows/arm64-host`.
- Final ARM musl binary SHA-256:
  `64107ef6afc86cc548edf61266f83222ada9cb2de32fedff5ed9cb06646fce2a`.
  The running firmware matches the release digest recorded in the ARM guide.
  Formatting and whitespace checks pass. The ARM source-review duplication
  scan remains at 55 clones and 619 duplicated lines.
- Firecracker and its proxy exit after the final test. The disposable working
  disk and test binary are removed, the state filesystem is trimmed, and the
  outer appliance powers off. QEMU PID 982339 is confirmed absent. The retained
  remote state occupies 18 GiB. Removing obsolete GNU/debug build caches saves
  approximately 9.8 GiB on the Linux host; tested release builds remain.
- Source commits are `e3d45fb44` (installation), `e76697b70` (debugger fixes)
  and `e149633c5` (ARM platform and firmware). The work remains on the requested
  worktree branch; no push or merge is performed.

Network header prerequisite:

- [x] Reproduce legacy non-mergeable RX length and payload corruption with a
  real TAP and guest descriptors (virtio 1.2 section 5.1.6.1).
- [x] Derive header layout from negotiated features for RX, TX and MMDS;
  configure TAP on activation, including activation after restore.
- [x] Verify both header layouts, merged RX, reset and restore, then run the
  network suite and native/ARM64 Clippy before transport integration continues.

The real-TAP regression first reports 76 bytes for a 64-byte Ethernet frame
with no negotiated features; the required length is 74. The backend now
selects a 10-byte legacy header or a 12-byte extended header from VERSION_1
and MRG_RXBUF. TAP configuration runs on activation, including after restore.
RX writes `num_buffers` only in the extended layout; TX and MMDS use the same
negotiated offset. Receive buffer limits include the selected header length.
No snapshot fields change: the layout derives from saved feature bits.

All 56 network tests pass. The payload regression covers real TAP RX and TX
for legacy non-mergeable, modern non-mergeable and legacy mergeable layouts,
each after cold activation, reset and backend restore. The MMDS test checks
ARP request/response offsets in all three layouts. Native and ARM64 all-target
Clippy checks pass with warnings denied. Duplication remains at 19 clones and
204 lines. These are backend checks; XP boot and guest networking still need
the transitional PCI transport. No new guest boot is claimed.

The x86 platform selection is now `pci_transport: "transitional"` in machine
configuration. Modern remains the default; ARM64 rejects the transitional
variant during deserialisation. Cold boot rejects the selection unless PCI
is enabled. The resource allocator persists the mode and selects the same
32-bit pool for allocation, relocation and release. ACPI omits the unused
64-bit producer window. Restored capability BARs must fit the selected pool
and its alignment. This changes the snapshot format to 15.

The configuration regression fails on the old unknown-field error. All 74
selected VMM tests and five API tests pass after implementation. Coverage
includes the generated ACPI aperture, both relocation paths and their KVM
notification registrations, invalid restored BAR addresses, allocator state
serialisation, the PCI enable gate and modern device-manager persistence.
The relocation fixture uses the allocated address: the low aperture begins
between BAR alignment boundaries. Native and ARM64 Clippy checks cover both
VMM and Firecracker with warnings denied. PCI duplication falls from 23
clones / 201 lines to 22 / 189; shared API fixtures reduce that file from four
clones / 36 lines to zero. Legacy IDs, registers and INTx remain pending.
No guest boot or format-15 guest restore is claimed.

### Transitional PCI I/O mapping

The x86 transitional profile now allocates a 256-byte I/O BAR at BAR 0 for
network, block, balloon and entropy devices. It exposes their legacy PCI IDs,
revision 0 and virtio subsystem IDs. Modern capabilities and MSI-X tables use
BAR 2. Devices without a legacy ID keep their modern interface.

The PCI command register gates port access. Enabling I/O decoding applies a
pending BAR relocation only if the new port range is available. Removal frees
both bus mappings and address allocations. Snapshot format 16 retains the port
allocator and mapped port base; restore rejects incompatible or malformed BARs.
The PCI configuration-access capability now reads its header independently of
its selected transfer length and uses its selected BAR for data accesses.

This stage supplies legacy register reads. Register writes, queue activation,
INTx delivery and XP runtime validation remain pending. Earlier guest results
use earlier snapshot formats; they do not validate format 16.

Validation: the original PCI identity test fails with modern device ID `1044`
instead of legacy `1005`. The separate capability test exposed a zero BAR
selector when the transfer length was zero. Both pass after the changes. All
73 focused PCI, transport, device-manager, allocator and snapshot tests pass.
Native and ARM64 all-target VMM Clippy checks pass with warnings denied. The
scoped duplication result remains 20 clones / 177 lines. No guest boot or
full-VM restore was performed for this stage.

### Legacy configuration writes

Legacy port writes now select queues, configure the contiguous queue layout
from a page-frame number, and accept supported low-word feature bits. Queue
and feature configuration can occur before a status transition. A zero page
frame clears an inactive queue; out-of-range queue selections and malformed
register accesses leave queue state unchanged. Active backend queue addresses
and feature bits cannot be replaced through this interface.

Device configuration writes use offset 20 with MSI-X disabled and offset 24
with MSI-X enabled. Vector assignments share the modern interface's range
checks. A balloon device test checks configuration writes before feature
negotiation, layout changes, valid vectors and rejected vector numbers.

Status writes, reset, notifications and early queue activation remain pending.
The next activation change must support independently configured queues: the
current backends initialise all queues together and assume their ring pointers
are valid during event processing. Waiting for every queue would not prove
legacy early-use support. The legacy queue-configuration test exercises the
actual port bus and fails before this change because queue selection stays 0.

The active-queue protection test also exposed an existing test-fixture defect:
its modern queue setup used eight-byte writes, which the common configuration
handler correctly rejects. Virtio 1.2 section 4.1.3 requires 32-bit accesses to
the halves of these addresses. The helper now writes and reads back both halves
and uses separate ring addresses within the fixture's guest memory. Earlier
activation tests could pass with overlapping queue addresses left at zero.

Validation: 55 focused transport and device-manager tests pass, including the
three legacy configuration tests and corrected modern activation/reset tests.
Native and ARM64 all-target VMM Clippy checks pass with warnings denied.
Formatting and diff checks pass; scoped duplication remains 20 clones / 177
lines. This change adds no snapshot fields and retains format 16. No guest VM
was started for these checks.

### Legacy queue lifecycle

Legacy notification and status writes activate configured queues without
FEATURES_OK or DRIVER_OK. Feature negotiation ends at first use and excludes
bits above 31. A later unused queue can be configured while another queue is
active; live queue addresses cannot be replaced. Net and balloon backends
support partial queue activation. Modern activation keeps its existing strict
queue checks.

Queue operations skip uninitialised rings. Ring-pointer initialisation commits
all validated pointers together. Snapshot restore preserves unconfigured queues
but rejects invalid sizes and consumed state on an unconfigured queue. Dirty
tracking visits configured queues. Snapshot format 17 adds the active PCI
interface, including the legacy state that has processed requests while its
status is still zero.

Legacy reset stops the backend, clears queue and feature state, drains pending
notifications and resets vector assignments. It also clears queues configured
before activation. A modern reset can reclaim a legacy-active device; normal
modern register writes cannot overwrite an active legacy queue.

The port-bus regression submits a real balloon deflate descriptor with only
queue 1 configured and status zero. It verifies completion, restores backend
and transport state into a different VM, completes another descriptor, adds
queue 0, advances status without FEATURES_OK and resets. The network test sends
and compares TAP traffic with TX active before RX is configured, including a
backend restore between activation and use.

Validation: the original port-notification regression fails because the backend
never activates. The final 233-test run covers PCI and MMIO transports, queue
primitives, snapshot state, network, balloon, block, RNG and device management.
All pass. Native and ARM64 all-target VMM Clippy checks pass with warnings
denied; formatting and diff checks pass. Scoped duplication is 41 clones /
412 lines, compared with 41 / 397 before the change; the additional lines are
the explicit modern/legacy activation wrappers on the two multi-queue backends.
Repeated test setup was consolidated. No Windows VM boot or full-VM snapshot
was run for format 17. INTx delivery and final guest validation remain pending.

### Shared PCI interrupts

The transitional platform now routes PCI INTA to GSI 16. Each device retains
its queue and configuration interrupt bits; the shared line remains pending
while any unmasked device needs service. Reading either ISR interface clears
that device's bits. PCI interrupt disable and MSI-X enable mask its INTx
contribution. KVM resampling reasserts the line after guest acknowledgement if
another source remains pending.

Backend eventfd notifications pass through the current interrupt mode on the
transitional platform. Reset drains queued notifications, clears ISR state and
retains the stable eventfds. Device removal unregisters the forwarding handler
and removes its contribution to the shared line. Modern-only platforms retain
their existing MSI-X notifier path.

Snapshot format 18 stores each device's pending ISR bits and rejects undefined
bits or nonzero INTx state on a modern-only platform. A transport test restores
pending state into a different VM and verifies read-to-clear. Full-VM and
differential restore checks remain in step 7.

Validation: the PCI pin regression fails before the change. All 59 focused
transport, device-manager and INTx tests pass. The KVM test executes a guest
interrupt handler and LAPIC EOI, checks two devices sharing the line, and checks
masking and reassertion. Further tests cover both ISR interfaces, MSI-X pending
bits, eventfd forwarding and reset. The broader 184-test PCI, MMIO transport,
allocator and snapshot-state run also passes. Native and ARM64 all-target VMM
Clippy checks pass with warnings denied, as do formatting and diff checks.
Scoped duplication remains 18 clones,
with 159 duplicated lines compared with 158 before the change. No Windows
guest has yet validated this interrupt implementation.

### Snapshot vector assignments

The restore audit found that the saved per-queue MSI-X assignment list was not
checked against the backend queue count. Restore also accepted assigned vector
numbers outside the device's MSI-X table. Such state could omit interrupt
delivery or cause a later table access to panic.

Restore now checks the list length and each queue and configuration vector
before allocating MSI-X resources. Valid table indices and NO_VECTOR remain
accepted, including shared queue/configuration vectors. The regression first
failed because an empty queue-assignment list was accepted. The device-manager
restore test now covers both modern and transitional platforms with each
supported PCI device type. These are component tests; full-VM and differential
snapshot validation remain pending.

Validation: all 186 focused PCI, MMIO transport, device-manager, allocator and
snapshot-state tests pass. Native and ARM64 all-target VMM Clippy checks pass
with warnings denied. Formatting and diff checks pass; scoped duplication
remains 9 clones / 76 lines. This validation change retains snapshot format 18.

### Format 18 full-VM restore

The native production build from `33e344a1e` passed Linux cold boot, full restore
and differential restore in three configurations: modern PCI, transitional PCI
with the legacy driver, and that legacy driver with `pci=nomsi` to require INTx.
Each restore used a fresh Firecracker process with default seccomp enabled.
Differential memory was rebased onto the full memory image before loading the
matching differential VM state.

The guest wrote a 64 KiB sequence payload, synced the file, read it back and
compared its contents every second. Sequence output continued after both
restores. The explicit INTx and modern runs each progressed from 0--2 at cold
boot, through 3--5 after full restore, to 6--8 after differential restore.
This covers the transport and interrupt path with real block I/O. Windows
storage, TAP, shutdown and snapshot checks remain in steps 8 and 9.

Binary SHA-256:
`93ed002c9192699d7ba749501d087a9714a6cfdf95b87fb1fbec3de91400923e`.
Local and host evidence is under `build/windows/legacy/restore18/`. All three
test VMs were stopped; disposable memory snapshots and the test disk were
removed after verification.

### XP PCI routing investigation

The format 18 transitional build passes the earlier XP root-resource-list
failure. XP now reaches `AcpiArbCrackPRT` in ACPI.sys and stops with `0x7E`,
exception `0xC0000005`: the `ARBITER_EXTENSION.InterruptRouting` pointer is null.
The matching Microsoft PDB identifies the routine and structure field.
`PciInterfacesInstantiated` is zero; the interrupt model is already APIC.

Changing the transitional root's primary ID from PNP0A08 to PNP0A03 produced
the same fault. That candidate was discarded. The debugger traced the failing
device through its device extension to the VCLK namespace node. PCI routing
interface initialisation had not completed when XP processed this resource.

[QEMU's VMClock implementation](https://github.com/qemu/qemu/blob/master/hw/acpi/vmclock.c)
exposes the shared memory page without an interrupt resource. Firecracker
exposes both memory and an extended interrupt resource. The
[VMClock ABI](https://github.com/torvalds/linux/blob/master/include/uapi/linux/vmclock-abi.h)
makes update notification conditional on NOTIFICATION_PRESENT. This difference
is a testable lead, not yet a confirmed fix. Any change must keep the advertised
capability, ACPI resources and restore notification behaviour consistent.

Cold-boot diagnostics on the same XP disk narrowed the failure:

1. A polled VMClock, with no interrupt resource or notification capability,
   moved the same fault to GED. The counters and memory updates were retained.
2. Small IRQ descriptors for PC-compatible interrupt lines, with both custom
   devices intact, still failed at VCLK. Descriptor encoding alone is not the
   cause.
3. Removing both VCLK and GED interrupt resources moved the same fault to RTC0.
   Disabling custom notifications therefore does not fix the platform.

All three runs stopped at the same ACPI instruction with PCI routing still
uninitialised. The debugger identified each device through its namespace node.

A fourth diagnostic retained every interrupt resource and changed only the
parents of VCLK, GED, RTC0, COM1 and PS2 from `_SB_` to `_SB_.PC00`. XP's PCI
routing query then returned success, and `PciInterfacesInstantiated` became
one. The active process list contained `smss.exe`, `csrss.exe`, `winlogon.exe`,
`services.exe`, `lsass.exe`, `explorer.exe` and service processes. This is XP
user-space boot evidence on Firecracker's transitional virtio storage and INTx
path. It does not prove networking, shutdown or snapshot recovery.

The BSP was in `HalProcessorIdle`; the second vCPU remained at the firmware
entry point. This run therefore does not establish XP SMP support. All test
VMs and debuggers were stopped. Diagnostic patches and debugger logs are in
`build/windows/legacy/pvh-bios/`. All temporary source changes were reverted
locally and on the test host; the production binary remains unchanged.

Implementation and validation sequence:

1. [x] Add a failing AML topology regression for the transitional PCI platform.
   Check the five interrupt-bearing devices' parent and retained resources.
2. [x] Compose that platform's device namespace beneath PC00. Preserve valid
   namespaces for modern PCI, MMIO and ARM64 and resolve notification targets.
3. [x] Run AML/device tests, native and ARM64 lint, formatting and duplication
   checks. Boot XP with the production build and default seccomp.
4. [x] Continue XP storage, TAP, shutdown and full/differential restore checks,
   then repeat the modern Windows regressions in steps 8 and 9 above.

The production implementation selects the parent from the persisted PCI
transport mode. Modern PCI, MMIO and ARM64 keep the existing paths. VMGenID
stays at `_SB_.VGEN`, and GED retains its absolute notification target. No
snapshot state layout changes are required; restored guest memory contains
the saved ACPI tables.

The topology regression fails before the change and passes afterwards. It
checks all five device paths, each device's interrupt resource and GED's
notification target on transitional PCI, modern PCI and MMIO. All 15 focused
ACPI tests and 20 device-manager tests pass. The latter require serial execution
because their network fixtures share a TAP name. Native and ARM64 all-target
VMM lint checks pass with warnings denied. Formatting and diff checks pass;
scoped duplication is unchanged at 26 clones / 420 lines.

The first production XP run with default seccomp stopped on a blocked vCPU
`read` syscall. A traced repeat identifies an eight-byte eventfd read. The
transitional interrupt reset drains forwarding eventfds, but the x86 vCPU
filter has no read rule. This must be fixed and tested before the production
boot requirement in step 3 can be marked complete.

The x86 vCPU filter now allows reads with an eight-byte count, matching eventfd
counters. A regression executes the compiled production filter in separate
processes: an eight-byte eventfd read succeeds, while seven- and nine-byte reads
terminate with SIGSYS. The positive case failed before the rule was added.
All four seccomp tests, release-mode Firecracker lint with warnings denied,
formatting and diff checks pass. Scoped duplication remains 6 clones / 263 lines.

XP then passed production cold boot with default seccomp, one vCPU and 2 GiB
RAM. Its startup batch copied a 64 KiB random file, compared the result with
`fc /b`, and requested shutdown only after success. Firecracker recorded ACPI S5
and exit code zero. A read-only host mount verified both files have SHA-256
`32b077fadb2d5650517dcb8d279c4be4db6772a5331bd4db25b82d571720a7e6`.
The expected COM1 messages were absent, so this is not serial-output validation.
XP TAP egress, serial output and snapshot recovery remain pending.

The serial diagnostic loads `Serial` but finds no COM port. XP's install log
reports `CM_PROB_NORMAL_CONFLICT` for COM1 and PS/2. Their ports are below
`0x1000`, outside the parent PCI bridge's advertised I/O window.
[ACPI](https://uefi.org/htmlspecs/ACPI_Spec_6_4_html/06_Device_Configuration/Device_Configuration.html)
requires the bridge to describe resources available to its children.
[QEMU](https://github.com/qemu/qemu/blob/master/hw/i386/acpi-build.c) advertises
`0x0000..0x0cf7` and `0x0d00..0xffff`, excluding configuration ports.

1. [x] Reproduce the missing transitional bridge ranges in the AML test.
2. [x] Advertise those ranges for transitional PCI; retain the BAR allocator's
   lower bound and the modern platform's resource layout.
3. [x] Run PCI tests, native and ARM64 lint, formatting and duplication checks.
4. [x] Repeat the XP serial probe with production seccomp and inspect its PnP
   result before proceeding to TAP and snapshot validation.

The new range assertion failed before the fix; all seven PCI segment tests
pass afterwards. Native and ARM64 all-target VMM lint pass with warnings
denied. Formatting and diff checks pass. Scoped duplication remains one
clone / six lines. BAR allocation still starts at `0x1000`.

The production XP repeat with default seccomp now opens COM1: `mode` reports
115200 baud and exits successfully. The startup probe then blocks on its
first serial write. The VM was stopped after three minutes, and read-only
disk inspection confirmed the probe reached `MODE_RESULT=0`. Thus the bridge
range fixes COM1 resource assignment, but serial transmission is still not
verified. Investigate UART transmit interrupts next; TAP and snapshot checks
remain pending. Logs are in `build/windows/legacy/xp-production/pci-window-*`.

The upstream `vm-superio` 0.8.2 release re-evaluates interrupts when IER is
written. Version 0.8.1 only stores the new enable bits. QEMU's UART explicitly
implements transmit-empty assertion on enable because Windows depends on it.

1. [x] Reproduce the missing transmit interrupt through Firecracker's serial
   bus interface, including acknowledgement and disable/re-enable.
2. [x] Upgrade to the upstream release, then run legacy-device tests, native
   and ARM64 lint, formatting and duplication checks.
3. [x] Repeat the XP production probe with default seccomp. Require the serial
   marker, successful guest write and ACPI shutdown before claiming a fix.

The interrupt regression fails with 0.8.1 and passes with 0.8.2. All 24
legacy-device tests pass, including serial restore. Native and ARM64 all-target
VMM lint pass with warnings denied; formatting and diff checks pass. Scoped
duplication remains two clones / 12 lines. The first post-upgrade test command
used the old test binary; the repeat uses the newly compiled binary.

XP then emitted `FIRECRACKER_XP_SERIAL` through COM1, recorded
`MODE_RESULT=0` and `WRITE_RESULT=0`, and reported the COM1 PnP error code as
zero with status `OK`. It requested ACPI S5 and Firecracker exited zero with
default seccomp. Evidence is in `build/windows/legacy/xp-production/uart-upgrade-*`.
Production binary SHA-256:
`15f7a755a2668ecd264c6aceef727e77279d4075b99004cc99b09723af6322ab`.
No snapshot state layout changes were made. This validates serial transmission;
XP TAP egress and full/differential recovery still require guest tests.

The first XP TAP boot enumerated the transitional NIC as
`PCI\VEN_1AF4&DEV_1000&SUBSYS_00011AF4`, matching the existing NetKVM INF.
XP rejected its background installation because it did not trust the driver
signature. The guest therefore had no usable adapter; gateway, Internet and
DNS probes failed. Firecracker exited normally after guest ACPI shutdown.
The image's machine driver-signing policy is already Ignore (`00`).

1. [x] Install NetKVM explicitly with `UpdateDriverForPlugAndPlayDevices` and
   record the return code and device status.
2. [x] Verify activation with production seccomp, then guest IP configuration,
   gateway traffic, DNS and outbound transfer. Fix observed failures.
3. [x] Validate full and differential recovery with storage and network probes.

The normal installer call succeeds under the existing signing policy.
`INSTALLFLAG_NONINTERACTIVE` instead fails with `CERT_E_WRONG_USAGE`; no
driver files or signing policy were changed. The installed NIC then reports
Code 10. Its [2012 driver source](https://github.com/virtio-win/kvm-guest-drivers-windows/blob/eb971e2f608e2e911f821c7a0edf83e3a9179dfc/NetKVM/Common/ParaNdis-Common.c)
requires a 32-byte I/O resource in `GetAdapterResources`. Firecracker currently
uses 256 bytes for every transitional device. The port-zero reads logged on
failure occur after the driver rejects its resources.

Next, size the legacy I/O BAR from the exposed device configuration. Preserve
the modern capabilities and optional network configuration, and use the
selected size consistently for allocation, relocation, removal and restore.
Add a failing network BAR-size regression before changing implementation.
Then repeat XP activation and egress. Test logs and the XP-compatible Rust
installer helper are in `build/windows/legacy/xp-network/`. All test VMs and
the isolated network namespace were stopped and removed after inspection.

Legacy BAR implementation sequence:

1. [x] Reproduce the network BAR sizing error with and without optional MTU.
2. [x] Compute the BAR from the legacy configuration extent and MSI-X header;
   use its persisted size for mapping, relocation and removal. Validate the
   expected device size on restore and increment the snapshot format for the
   changed hardware layout.
3. [x] Test sizing, relocation, removal and restore; run native and ARM64 lint,
   formatting and duplication checks.
4. [x] Boot XP with the rebuilt production binary and verify NIC activation,
   then continue guest egress and snapshot checks.

Legacy BAR sizes now include the 24-byte MSI-X-capable header and the device's
legacy configuration extent, rounded to a power of two. Net exposes its first
eight bytes unless MTU is configured, in which case all 12 bytes are retained.
The modern configuration remains intact. Mapping, relocation and removal use
the allocated BAR size; restore rejects sizes inconsistent with the backend.
Snapshot format 19 rejects older fixed-size hardware layouts.

The network sizing regression failed at 256 versus 32 before the change.
All 41 PCI transport, 20 device-manager, 56 network and six snapshot tests now
pass. The tests cover both network sizes, fresh-VM restore, malformed sizes,
relocation and removal. Native and ARM64 all-target VMM lint pass with warnings
denied. Formatting and diff checks pass; scoped duplication remains 23 clones
/ 239 lines. Two test-fixture errors were corrected during development: restore
needs a fresh VM's interrupt allocator, and a replacement BAR slot must first
be cleared.

The next XP production boot reaches an ioctl in NIC activation and terminates
under seccomp (syscall 16). Trace the request and add a targeted filter regression
before changing policy. This is not yet a successful network boot. Evidence is
in `build/windows/legacy/xp-network/bar-size-*` and
`build/windows/legacy/pvh-bios/pci-bar-size-*`.

The trace identifies `TUNSETVNETHDRSZ` on the vCPU thread. Network activation
sets the negotiated virtio header length there, but only the VMM-thread filter
permits this ioctl. Both architecture policies need the exact vCPU request.

1. [x] Reproduce the blocked ioctl with the compiled production filter and a
   real TAP; test 10-byte and 12-byte headers and rejection of another request.
2. [x] Add the request to the x86 and ARM64 vCPU policies, run filter tests and
   lint, and rebuild the production binary.
3. [x] Repeat XP network activation and guest egress with seccomp enabled.

The regression initially terminates on SIGSYS for a 10-byte header. With the
exact ioctl permitted, both 10-byte and 12-byte TAP headers pass; an unrelated
`TUNSETVNETLE` request still terminates on SIGSYS. All five native seccomp tests
pass. Native and ARM64 release-mode Firecracker lint pass with warnings denied.
Formatting and diff checks pass. Policy-file duplication rises from 13 clones
/ 1639 lines to 14 / 1661 because the request must appear in both architecture
and thread policies; the Rust test code adds no detected clone.

XP now passes production TAP egress with default seccomp. The guest reports a
working NetKVM adapter, static address `172.30.254.2/30`, gateway
`172.30.254.1` and DNS server `192.168.1.1`. Gateway and Internet pings each
receive three replies; DNS resolves `example.com`. The XP-compatible Rust
probe receives HTTP 200 and 577 bytes directly from that site, then sends and
compares 2 MiB through a TCP echo service on the host. The guest reports
`PASS XP_TCP_EQUAL bytes=2097152` and `TRANSFER_RESULT=0`; the host independently
records `echoed=2097152`. This proves HTTP and TCP egress, not XP HTTPS or vsock.
The guest requests ACPI S5 and Firecracker exits zero.

During preparation, the old startup probe forced a driver reinstall on every
boot. A stopped run left a zeroed installed driver header and the next boot
reported Code 39. The source driver remained intact. The test image's installed
file was restored byte-for-byte, and startup now only runs the network probe.
After the successful transfer and shutdown, read-only inspection confirms the
installed and source driver files still match. The probe selects only the active
IP-enabled adapter, excluding phantom interfaces from previous installations.

Evidence: `build/windows/legacy/xp-network/transfer-{serial,firecracker}.log`,
`transfer-guest.txt` and `echo.log`. The probe sources are
`transfer.rs` and `transfer-probe.cmd`. Production SHA-256:
`3162396d8495e378fec66908854138d09191a2f7ce09647071e186c77e06e083`.
The VM, echo service, TAP namespace, routes and test firewall rules are stopped
or removed; the disk is unmounted. XP full/differential restore and final modern
Windows regressions remain pending.

Production binary SHA-256:
`b1e25097924f4e09c2b541a85e8e9c59811f19d08021d081b258aacc02b59e1d`.
Evidence and the startup probe are in `build/windows/legacy/xp-production/`.
All test processes exited and the disk was unmounted after inspection.

QEMU also has an explicit [XP/2003 interrupt-link fix](https://github.com/qemu/qemu/commit/c82f503dd5c3f0a01a9e63741f1f875652669867):
all interrupt links need a _DIS method. Firecracker currently uses direct GSI
routes, so that patch is not a direct fix for the observed VCLK failure.

### XP full and differential restore, snapshot format 19

On 2026-10-14 at 00:49 UTC, XP passes cold boot, full restore and differential
restore with the production binary identified above. Each restore uses a fresh
Firecracker process, native KVM and default seccomp. Each stage flushes, reads
and compares a 65,536-byte patterned file, receives direct HTTP 200, and checks
every byte of a 2,097,152-byte TCP echo exchange through TAP. The host records
three separate connections, each with exactly 2,097,152 bytes.

The source stays paused between snapshot creation and termination. Each next
process uses the same disk without intervening writes. Differential memory is
rebased onto the full memory image before the final restore. No earlier disk
state is reused after the guest advances.

The final guest requests ACPI S5; Firecracker exits zero at 00:49:59 UTC.
Read-only disk inspection independently verifies the final pattern, seed
`133671`, and SHA-256
`1d66ea4ae96d031d17fa4dc9901e5b4fe4e27b19b8f3f8769ddadd184abe16b5`.
The installed NetKVM file still matches its source. Evidence and probe sources
are under `build/windows/legacy/xp-restore/`; successful logs are in `run2/`.
The first run exposed a batch-script marker redirection error and completed
without snapshots; it is excluded from restore evidence.

All test processes, mounts, network resources and temporary snapshots are
removed. Snapshot cleanup recovers approximately 2.1 GiB. This test establishes
XP storage and TAP recovery; it does not establish XP HTTPS or vsock support.
Final modern Windows regressions and the Toets Windows 10/11 tests remain open.

### Windows 7 regression, snapshot format 19

On 2026-10-14, Windows 7 SP1 x64 passes cold boot, full restore and differential
restore on the current production binary, with two vCPUs, 4 GiB RAM, Hyper-V
enabled, modern PCI and default seccomp. Each restored stage uses a fresh
Firecracker process and the same linear disk handoff described for XP.

Each stage writes and flushes a new 65,536-byte random file, reopens it and
compares all bytes. Each also resolves `www.microsoft.com`, receives direct
HTTPS 200 over TLS 1.2 with certificate validation, and compares 2,097,152
bytes in both directions through TAP. The host independently records three
complete echo exchanges and three successful guest reports with storage hashes.

The final guest requests ACPI S5 at 00:56:32 UTC; Firecracker exits zero.
Read-only disk inspection confirms the final file length and reported SHA-256:
`0154b80ca503113aa27b6081ae0b3de318960221972a7b8e9964150eec016981`.
The guest transcript confirms all three HTTPS responses and storage checks.
Evidence is in `build/windows/legacy/win7-restore19/run1/`; the parent directory
contains the probe and host scripts. Temporary snapshots occupying 4.2 GiB
are removed, and the VM, listener, mounts and test network resources are stopped
or removed. Windows 7 vsock support is not established by this TAP regression.

### Server 2025 CONNECT regression, snapshot format 19

On 2026-10-14, Windows Server 2025 build `26100.32230` passes cold boot,
full restore and differential restore on the current production binary. The
guest has two vCPUs, 4 GiB RAM, Hyper-V enabled, modern PCI, default seccomp
and no network adapter. Firecracker runs as the non-root user authorised by
the real Tokoloshe sidecar's socket grant.

Each stage writes, flushes, reopens and compares a random 2 MiB file. The
Windows vsock probe then sends that file through the granted CONNECT tunnel
and compares every returned byte. An adjacent ungranted destination returns
HTTP 403. Sealed Tokoloshe accounting independently records three Connected
entries with 2,097,152 bytes in each direction and three PolicyDenied entries
with zero payload bytes. Full and differential restore use fresh processes
and the same linear disk handoff as the XP test.

The final guest requests ACPI S5 and Firecracker exits zero at 01:07:47 UTC.
Read-only inspection confirms the final file is 2,097,152 bytes and matches
the guest's SHA-256:
`9506dda6f1eee00e3d1489dea2fb114f2f0f777ea0af1132ed76c6944c590632`.
Evidence is in `build/windows/server19/`: `cold2.log`, `full.log`, `diff.log`,
`guest-transcript.txt` and `proxy-accounting.log`.

An initial harness run incorrectly used root for Firecracker while granting
the socket to the non-root user; it failed before proxy accounting. A stale
host socket also blocked the first restore attempt. The corrected harness
uses the granted identity and removes the owned socket after confirming that
the source process has exited. Neither failed attempt is counted as a pass.
The guest, sidecar and harness have exited; mounts are removed. Deleting the
disposable disk and snapshots recovers approximately 15 GiB. Server TAP and
ARM64 regressions on this revision remain pending.

### Server 2025 TAP regression, snapshot format 19

On 2026-10-14, Server 2025 also passes cold boot, full restore and differential
restore with a NetKVM adapter and no vsock device. Each stage verifies a flushed
65,536-byte random file, DNS resolution, direct HTTPS 200 over TLS 1.2 with
certificate validation, and 2,097,152 bytes in each direction through TAP.
Three independent host echo records and guest reports confirm the payload
counts and storage hashes. The source remains paused until termination at
each snapshot handoff; each restore uses a fresh Firecracker process.

The guest requests ACPI S5 and Firecracker exits zero at 01:14:47 UTC.
Read-only inspection verifies the final 65,536-byte file against the reported
SHA-256 `5e50817328c885772943fa4179c62d4371e19a6607ae9ee978002884edbe5c29`.
Evidence and the recovered guest transcript are in
`build/windows/server-tap19/run2/`. The first launch stopped because its host
login helper lacked permission to inspect the root-owned VM process; it is
excluded from the evidence. All test processes and network resources are
stopped or removed. Disposable disk and snapshot cleanup recovers about 15 GiB.

The current ARM64 release binary builds successfully with the explicit Rust
cross-linker. Its SHA-256 is
`068748352323dd47b17949862c9497cade8623a0bf4fc230db18dd0077530b7c`.
ARM64 runtime regressions remain pending; a successful cross-build does not
establish guest compatibility.

### Legacy Viosock package audit

On 2026-10-14, the current driver ISO contains Viosock packages for Windows 10,
Windows 11 and Server 2016 or later, but none for XP or Windows 7. Direct
inspection of the upstream 0.1.173 and 0.1.208 archives finds no Viosock files.
The temporary archives are removed after recording their checksums.

The upstream [Windows 7 source build definition](https://github.com/virtio-win/kvm-guest-drivers-windows/blob/02.12.2021-last-buildable-point-Win7/viosock/sys/viosock.vcxproj)
does include x86 and x64 targets with KMDF 1.9. This establishes a historical
build target, not an available signed package or working Firecracker transport.
Upstream [release discussion 1185](https://github.com/virtio-win/kvm-guest-drivers-windows/issues/1185)
records that Viosock first entered public releases in build 285 in 2025.

XP and Windows 7 TAP results therefore do not establish vsock support. No
compatible signed Viosock package has been found for those guests, and no
custom driver build has been tested. Keep this limitation explicit in the
guest support matrix.

### ARM64 CONNECT regression, snapshot format 19

On 2026-10-14, Windows 11 IoT Enterprise LTSC passes cold boot, full restore
and differential restore with the current ARM64 release binary. The guest
uses two vCPUs, 4 GiB, PMUv3, 48-bit PARange, default seccomp and no network
device. The SYSTEM startup task runs unattended. Firecracker and the real
Tokoloshe sidecar run with the same non-root identity.

Each stage writes and flushes 2,097,152 random bytes, reads and compares the
file, verifies every byte through an allowed CONNECT echo, and checks a denied
destination. The guest transcript contains all three pass markers. Sealed
sidecar accounting contains exactly three allowed connections with 2,097,152
bytes in each direction and three denied connections with no payload.

The final guest requests shutdown at 03:25:55 UTC, and Firecracker exits zero
at 03:25:57. Read-only inspection finds no hibernation file and verifies the
final 2,097,152-byte file against the guest's SHA-256:
`0b95bed27353d10aa75b10963c8e03c9d0c46e55649339b2eb05b5cd72c90bcc`.
Evidence is retained in `build/windows/arm19/run/`.

This runs inside an emulated ARM Linux/KVM appliance, not native ARM hardware.
Cold probe startup takes about 32 minutes; the differential-restored probe
takes about 11 minutes to reach its final policy check. During that delay,
an independent read-only comparison verifies every allocated differential
memory extent against the rebased restore file. The checker also rejects a
deliberate one-byte mismatch. These results establish workload recovery but
do not establish native ARM performance.

The completed VM and proxy processes have exited. Their full and differential
snapshots and the obsolete format-12 resnapshot disk and memory are removed;
logs and the original baseline remain. The current working disk advances to
the separate TAP test. Current-format ARM TAP validation remains pending.

### Preserved SeaBIOS build and XP regression

The tested BIOS handoff, linker script, pinned SeaBIOS configuration and
ordered-status patch now live in `resources/firmware/seabios/`. Their contents
match the earlier XP test inputs. The documented build uses a fresh SeaBIOS
1.17.0 checkout and produces firmware SHA-256
`2c97c0e13ec572dd641a037c695c0b1bd9611ab6cd4601f7fa21187707fe4512`.
The first build reaches SeaBIOS successfully but cannot find `rustup` on the
host PATH; the instructions now use its installed absolute path. Compilation
with warnings denied and inspection of the Xen PVH entry then pass.

On 2026-10-14, the rebuilt firmware passes XP cold boot, full restore and
differential restore with the production format-19 binary and default seccomp.
Each stage verifies a flushed 65,536-byte patterned file, direct HTTP 200 and
2 MiB of equal TCP payload through TAP. Three host records independently
confirm 2,097,152 bytes each. Windows requests S5 at 03:42:05 UTC and
Firecracker exits zero. Read-only inspection confirms the driver remains
unchanged and checks every byte of the final file with seed `134609`, SHA-256
`edbb8537a6643194b48cc64b14d47291710a3a6f93dd0cc9110a49c74b3242d3`.

Logs, recovered transcript and final data remain in
`build/windows/seabios-pvh/run2/`. The temporary 1.7 GiB allocated disk and
approximately 2.1 GiB of allocated snapshots are removed. All native test
processes, mounts and network resources are stopped or removed. The separate
ARM TAP regression continues.

### Native Linux direct-boot regression, snapshot format 19

On 2026-10-14, the final native production binary
`3162396d8495e378fec66908854138d09191a2f7ce09647071e186c77e06e083`
boots the retained Linux 6.18.38 image with default seccomp, two vCPUs,
256 MiB, `hyperv: false` and direct MMIO boot. Its PID 1 prints the boot
marker and counter values 0-1. A full restore in a new process continues
at 2-5; rebasing and loading a differential snapshot in another process
continues at 6-10 without repeating the boot marker. Each source is paused
before snapshot creation and terminated before the next process uses its
separate disk copy.

This is a direct-boot and userspace-recovery regression. It does not add a
Linux networking or guest-shutdown claim. The host terminates each test
process after collecting its result. Logs remain in `build/windows/linux19/`;
the approximately 262 MiB of allocated temporary disk and memory files are
removed, and no native test process remains.

### Toets Windows 10 disk staging

While the ARM TAP regression runs, the original Toets Win10 golden remains
unchanged and closed. Its transferred QCOW2 copy matches SHA-256
`3561394895186a03166597132e7552efd26dd8895502a5616f4731d3b9f8704b`.
The QCOW2 check finds no errors. Raw conversion and `qemu-img compare` confirm
identical guest-visible contents. The redundant transferred QCOW2 is removed.

Read-only inspection of the separate raw disk identifies a bootable MBR
partition starting at sector 2048. Its registry reports Windows 10 Enterprise
LTSC 2021, 21H2, build 19044.7725. Hibernation is absent, automatic login is
enabled for `build`, and `EnableLUA` is zero in the supplied image. Viostor
and NetKVM are installed. The common startup directory is named `StartUp`.
These facts permit preparation of an unattended probe without changing UAC.
All inspection mounts and loop devices are removed.

The raw disk is `build/windows/win10-19/windows.raw` on the native host,
64 GiB logical and approximately 8.6 GiB allocated. No Win10 VM has started.
Boot and workload tests still follow completion of the current guest list.

### ARM TAP test allowance and modern guest preparation

The first format-19 ARM TAP attempt reaches its SYSTEM startup probe, installs
NetKVM and reports the adapter Up. It configures its address, resolves DNS and
receives HTTPS 200 with certificate validation enabled. The 2,400-poll harness
limit then expires before a bulk TCP result. The host stops that guest and
removes the TAP and NAT rules; no snapshot or full workload pass is claimed.
Its transcript and logs remain in `build/windows/arm19-tap/run1-timeout/`.
The retry keeps the same guest workload and Firecracker binary, with 5,400
poll attempts per stage and an explicit timeout diagnostic. It starts only
after the first harness has exited and cleanup has been checked.

Both Toets AMD64 test copies now contain the shared startup storage/TAP probe
and the existing native TLS client. Installation uses ordinary offline NTFS
writes after checking that the disk is closed, then compares the copied files.
Both configurations retain four vCPUs and 8 GiB, with the validated SeaBIOS
handoff, transitional PCI, Hyper-V and dirty tracking. Neither VM has started.

The original Win11 QCOW2 remains unchanged. Its transferred copy matches
SHA-256 `e8e7214a58f9bb81621f4c911157ff6d021500bee76dd538b22d71c9bbf30d01`;
the structural check passes and raw comparison reports identical contents.
Read-only inspection records build 26100.1742, display version 24H2, automatic
login for `build`, `EnableLUA` zero and no hibernation file. Its registry product
string is `Windows 10 IoT Enterprise LTSC 2024`; the Toets definition names the
image Windows 11 IoT Enterprise LTSC 2024. Viostor and NetKVM reside in
DriverStore; the active control set marks Viostor as a boot driver.

The Win11 raw disk is `build/windows/win11-19/windows.raw`, 64 GiB logical and
approximately 5.7 GiB allocated. Its transferred QCOW2 is removed, and all
inspection mounts and loop devices are detached. The native CONNECT helper
also builds with warnings denied using the fixed-endpoint option already
tested on ARM. Win10 and Win11 runtime results remain pending.

### Modern AMD64 CONNECT test preparation

The Win10 and Win11 Viosock packages are staged separately from the unchanged
virtio-win 0.1.302 ISO, whose checksum matches the Toets pin. Each staged set
contains the INF, catalogue, driver, x64 and x86 provider DLLs, and provider
service. Unused test executables and debug symbols are not extracted. Both
packages declare driver version 100.103.104.30200. The bundled bridge serves
inbound vsock-to-TCP connections; outbound tests use the existing native client.

The shared CONNECT probe now accepts a guest prefix and allowed/denied ports.
Its optional driver installation must return success before the three flushed
2 MiB storage and CONNECT checks. The host controller and offline verifier
accept native paths and user IDs, preserving the same identity for Firecracker
and the socket grant. Final verification still requires exactly three allowed
2 MiB transfers in each direction, three empty policy denials, three guest pass
markers, the expected guest identity and the final disk hash.

A host preflight finds that the initial proxy socket path is 119 bytes. A
minimal Unix-listener reproducer rejects it before any VM starts. Private
`build/windows/p10` and `p11` proxy roots reduce the generated paths to 101
bytes; binding succeeds. The actual native Tokoloshe sidecar then opens the
grant, reports ready, seals empty accounting and exits cleanly. Its socket,
listener and temporary proxy root are removed. This is host setup evidence,
not a Windows egress result. Logs remain in `build/windows/win10-connect19/`.

The CONNECT configuration has no NIC. Its driver and startup files remain
outside both guest disks until each guest's TAP test completes, so preparation
does not replace either pending TAP probe.

### ARM Linux direct-boot regression, snapshot format 19

On 2026-10-14, the final ARM production binary
`068748352323dd47b17949862c9497cade8623a0bf4fc230db18dd0077530b7c`
boots Linux `7.2.8-tokoloshe-host-8` directly with two vCPUs, 256 MiB,
default MMIO and default seccomp. The initramfs contains the existing Rust
counter probe, compiled for AArch64 musl with warnings denied. No firmware,
Windows CPU template, guest disk or network device is used.

Cold boot prints the boot marker and counters 0-8. A full snapshot loaded in
a new process continues at 9-13. After a differential snapshot and rebase,
another new process continues at 14-19. Neither restore repeats the boot
marker. Each source is paused before snapshot creation and stopped before
the next process starts. The host stops the final regression VM after proof;
this is not a guest-shutdown or Linux networking test.

Logs, configuration and input checksums remain in
`build/windows/arm-linux19/`. All three regression processes have exited.
The temporary kernel, initramfs and snapshots are removed, recovering
approximately 299 MiB inside the appliance; the temporary native-host
init executable and archive are also removed. The separate Windows ARM
TAP test was still live after its cold workload pass and full snapshot load
when this Linux regression completed.

### ARM TAP evaluation activation prerequisite

The second ARM TAP run passes its cold workload and loads the full snapshot
in a new process. The restored guest again resolves DNS and receives HTTPS
200, then exchanges part of the second TCP payload. It powers off before
the second pass report. Firecracker exits zero at 05:22:13 UTC, but the
controller correctly fails because the workload is incomplete. No differential
restore result is claimed for this run.

The recovered Application log records WLMS event 100 at 04:50:44 UTC and
event 101 at 05:20:51 UTC. The installed WLMS message resource identifies
licence-period expiry and shutdown. This matches Microsoft's documented
[hourly evaluation shutdown](https://www.microsoft.com/en-us/evalcenter/evaluate-windows-11-enterprise)
when evaluation activation is absent or expired. The guest transcript ends
during the second workload, without its exception handler or final pass.
This is a failed validation run, not a successful workload shutdown.

The retry configures the same TAP and checks the installed Windows licence
status. If it is not licensed, it invokes the standard
[`slmgr.vbs /ato` operation](https://learn.microsoft.com/en-us/windows-server/get-started/activation-slmgr-vbs-options)
and queries the status again. An unlicensed result stops the probe before
the workloads. It reports licence status and grace-period fields without
printing product keys. The three storage, DNS, HTTPS and 2 MiB TCP workloads
remain unchanged. Activation and the new runtime result are pending.

Evidence is retained in `build/windows/arm19-tap/run2-wlms-shutdown/`.
The failed controller, guest and echo server have exited; its TAP and NAT
rules are removed and forwarding is restored. The unmatched 4 GiB memory
snapshot is deleted after preserving logs. Normal offline NTFS writes install
and compare the updated probe, with no hibernation file present. The new
controller starts only after that cleanup and installation have completed.

### Toets Win10 MSI-X reset regression

The first native Win10 TAP run reaches the kernel but produces no probe or
network traffic. A paused diagnostic snapshot shows all four CPUs in
`HalProcessorIdle`, confirmed with the matching Microsoft kernel PDB. The
process list contains only System and Registry. The storage queue is active
with configuration/queue vectors 0/1, while MSI-X is disabled and its table is
empty. This is not a completed boot.

The modern virtio reset path recreates MSI-X state and clears its PCI enable
bits. [QEMU's virtio reset](https://github.com/qemu/qemu/blob/master/hw/virtio/virtio-pci.c)
unmaps virtio events without clearing the PCI MSI-X configuration. The virtio
reset must reset event-to-vector assignments while preserving PCI state.

- [x] Reproduce loss of programmed MSI-X state across a virtio status reset.
- [x] Preserve MSI-X configuration while clearing virtio interrupt assignments.
- [x] Run transport regressions and retry the unchanged Win10 guest.
- [x] Complete Win10 and Win11 TAP and CONNECT validation.

The separate ARM TAP retry again terminates before its activation gate. WLMS
event 101 at 06:34:33 UTC precedes guest poweroff and Firecracker exit zero at
06:35:37 UTC. No workload pass is recorded. Its controller correctly fails;
logs remain under `build/windows/arm19-tap/run3-wlms-shutdown/`. Activation
must run earlier on the next ARM boot, before the slow PowerShell/network
inspection path. This failure does not validate ARM TAP restore.

The regression fails on the previous implementation because reset clears the
PCI enable bit. The fix retains the MSI-X table, function mask, enable bit and
KVM routes. Both transport interfaces clear virtio vector assignments, ISR
state, forwarded events and pending bits. The modern path also drains queue
notifications. No snapshot layout changes.

The 41 transport tests pass. Serial runs of the broader PCI (128), device
manager (20), MMIO (14) and snapshot (14) suites pass; these suites overlap.
The tests require root for TAP creation and serial execution for shared
network names and metrics. Both AMD64 and ARM64 all-target VMM Clippy checks
pass with warnings denied. Scoped duplication remains six clones and 60
duplicated lines. Formatting and diff checks pass.

The unchanged Win10 guest now passes cold, full and differential storage,
DNS, certificate-validated TLS 1.2 HTTPS and 2 MiB TCP checks. It requests ACPI
S5 and Firecracker exits zero. Offline inspection finds no hibernation file,
three guest pass markers and the final 65,536-byte file matching SHA-256
`42a2e7e451cce970fcdd69e64cf83c4c12cf244e8dc0de2cf616eda0b7ca451a`.
The host records exactly three 2 MiB echoes and three pass reports. Evidence
remains in `build/windows/win10-19/run1/`. The native candidate digest
is `f67e8617398c209d465a833a24e019445aa274462f4fdde8d480dc6e4ae0fe3f`.

### Toets Win10 complete TAP and CONNECT validation

After the TAP result, the guarded installer verifies the final disk hash,
S5 and clean exit before adding the signed Viosock package and changing the
startup probe. The NIC-free run passes three 2 MiB flushed storage and
allowed/denied CONNECT iterations across cold, full and differential stages.
The real Tokoloshe sidecar seals six entries: three allowed connections with
2,097,152 bytes each way and three denied connections with zero bytes.

The guest requests S5 at 06:54:35 UTC and Firecracker exits zero. Offline
inspection confirms the `WIN10\build` identity, three guest pass markers,
no failure marker or hibernation file, and a final 2 MiB disk file with SHA-256
`f77ee7625ee2ed69c5e99de5b12d59eab23df88c712700fca94bb3166cf81bac`.
Evidence remains in `build/windows/win10-connect19/run/`. All guest, proxy
and helper processes have exited; completed TAP and CONNECT memory snapshots
are removed after preserving evidence. The original Toets QCOW2 is unchanged.

### Toets Win11 complete TAP and CONNECT validation

The Win11 IoT Enterprise LTSC 2024 AMD64 golden passes the same four-vCPU,
8 GiB SeaBIOS/transitional platform and native production binary as Win10.
Cold, full and differential stages each pass 65,536-byte flushed storage,
DNS, certificate-validated TLS 1.2 HTTPS and a byte-checked 2 MiB TAP transfer.
The controller exits zero after guest S5; offline verification confirms three
host and guest pass reports, exactly three echoes, no hibernation file and
final data SHA-256
`2b1142f4986019477371c9c3c898e0c20cab7336877091682f414e4a31c6f1dc`.

The separate NIC-free Viosock run then passes all three storage and CONNECT
stages. The real Tokoloshe proxy records exactly three allowed connections
with 2,097,152 bytes each way and three policy denials with zero bytes.
The guest requests S5 at 07:04:38 UTC; Firecracker exits zero. Offline
inspection confirms `WIN11\build`, three guest pass markers, no failure or
hibernation file and final 2 MiB data SHA-256
`39a7c5c99150086447dab90121a16b56af8d1e267014b9f7bb0e7eee699c9aa4`.

Evidence remains in `build/windows/win11-19/run1/` and
`build/windows/win11-connect19/run/`. Completed memory snapshots are removed;
guest, helper, proxy, TAP and namespace resources are stopped or removed.
The original Toets golden is unchanged. These results cover static IPv4,
not DHCP or IPv6, and do not port the Tokoloshe job runtime to Windows.

### ARM reset notification drain under seccomp

The first ARM boot with the MSI-X reset fix traps syscall 63 (`read`) at
firmware handoff. The modern reset now drains queue eventfds from the vCPU
thread; only the AMD64 policy already permits those reads. The ARM policy
now permits `read` only when its 64-bit count argument is exactly eight.
Default seccomp remains enabled.

The existing syscall test helper, compiled for ARM64, runs the old and new
compiled vCPU filters inside the appliance. The old filter traps an eight-byte
read. The new filter permits it, while counts 0, 7, 9, 16 and 4,294,967,304
still receive SIGSYS. This also checks that the argument comparison is not
limited to its low 32 bits. The native ARM release build succeeds; scoped
policy duplication remains five clones and 251 duplicated lines.

The failed handoff has no remaining VM, server or TAP. Its log is retained
under `build/windows/arm19-tap/run4-seccomp/`. The retry uses ARM binary
`f41bf66f8d0f07aae4cb49b002dab3928800efdd3f964912053e05b9e4bd7e8d`.
It runs standard online activation before driver installation and adapter
inspection, then still requires licensed status before the workload. The
07:34:57 UTC probe transcript reports `Product activated successfully` from
standard `slmgr.vbs /ato`, followed by successful NetKVM installation. The
transcript was read from a read-only reflink clone while the original guest
continued running. Evidence is retained in
`build/windows/arm19-tap/current-activation-transcript.txt`. The later licensed
status check and full ARM TAP regression remain pending; activation success
alone does not establish those results.

### Legacy regressions after the MSI-X reset fix

The final AMD64 binary `f67e8617398c209d465a833a24e019445aa274462f4fdde8d480dc6e4ae0fe3f`
repeats the Windows 7 and XP tests successfully. Windows 7 passes cold, full
and differential 65,536-byte storage, DNS, certificate-validated TLS 1.2 HTTPS
and 2 MiB TAP checks, then requests S5 and exits zero. Offline verification
matches the final file to SHA-256
`043a49e9193c2ec428f952ad59f494be2d3b2517dabe05c3fa3e06d2a9fd0de0`,
checks all three host and guest markers, and finds no hibernation file.

XP uses the documented SeaBIOS build and passes all three storage, HTTP 200
and 2 MiB TCP stages. It requests S5 at 07:16:24 UTC and Firecracker exits
zero. Offline verification checks every byte of the final 65,536-byte pattern
against seed 134984; SHA-256 is
`31a2a41c2a4684132189b589afdd7c4d05d0cf1c379953399ec8556379efa872`.
The host records exactly three 2 MiB echoes. The installed NetKVM driver still
matches its source, and no hibernation file is present.

Logs and final data remain under `build/windows/win7-reset19/run1/` and
`build/windows/xp-reset19/run2/`. All regression processes, TAP interfaces
and namespaces are removed. Full and differential snapshots are deleted after
closed-file checks. These runs do not establish XP HTTPS or legacy Viosock support.

### Linux PCI regressions after the MSI-X reset fix

The final AMD64 binary repeats Linux 6.18.38 boot and full/differential storage
restore with two vCPUs, 256 MiB, default seccomp and Hyper-V disabled. Both
modern PCI and transitional PCI with `pci=nomsi` pass. Each cold boot emits
storage counters 0-2, the full restore continues at 3-5, and the differential
restore continues at 6-8. Each counter flushes and verifies a 65,536-byte file.
Sources are paused, snapshotted and stopped before fresh restore processes;
differential memory is rebased before load. These checks stop the final VM
from the host and do not establish Linux S5 or networking.

The kernel's ELF notes include Xen type 18, the 32-bit PVH entry note, so this
also exercises the direct PVH handoff. The existing boot-contract tests cover
that handoff; no FreeBSD guest runtime result is claimed. Logs and configurations
remain in `build/windows/linux-reset19/{modern,intx}/` and its parent. The
separate root disk, compiled probe and all snapshot memory/state files are
removed after checking that the processes and file users have exited.

The earlier unchecked ECAM/resource item is now reconciled with the later
Windows 7 and ARM format-19 boot/CONNECT results. Those guests enumerate
and use PCI devices through the corrected generated ACPI resources. The final
ARM reset/activation retry remains a separate pending regression.

### CET state after Server 2025 differential restore

The final AMD64 recheck passes cold and full stages but faults after differential
restore. The extracted pagefile crash dump identifies `KePopulateContinuationContext`
at kernel RVA `0x3d6174`: it reads `IA32_PL3_SSP`, subtracts eight and faults in
`WRUSSQ` at address `0xfffffffffffffff8`. The snapshot MSR allowlist omits CET
registers, and the vCPU state has no current shadow-stack pointer field.
[QEMU's KVM implementation](https://github.com/qemu/qemu/blob/master/target/i386/kvm/kvm.c)
preserves both CET MSRs and `KVM_REG_GUEST_SSP`. Evidence remains in
`build/windows/server-tap-reset19/run2/`; the dump is retained on the native host.

- [x] Reproduce CET MSR loss across fresh KVM vCPU restore with nonzero registers.
  The native regression restores `IA32_U_CET` as zero instead of one.
- [x] Preserve all exposed CET state, including the current shadow-stack pointer,
  with correct feature checks, restore ordering, seccomp and snapshot versioning.
- [x] Run focused red/green, snapshot and architecture checks.
- [x] Repeat Server 2025 cold/full/differential TAP and real CONNECT validation.
- [x] Reconcile the final guest matrix and remove disposable diagnostic state.

The implementation now derives the CET MSR list from saved CPUID on every
snapshot, including snapshots taken after restore. It preserves the current
shadow-stack pointer through KVM ONE_REG and rejects snapshots whose pointer
presence conflicts with CPUID. Format 20 adds that pointer field; older formats
are rejected. The vCPU seccomp policy permits the exact GET_ONE_REG request.
Restore uses SET_ONE_REG before the restored vCPU starts.

The native red test loses `IA32_U_CET`; the fix passes nonzero CET MSRs and
current-pointer checks across two fresh save/restore cycles. All 18 x86 vCPU
tests, eight snapshot tests and all-target VMM Clippy pass. The duplication
baseline remains three clones. ARM all-target Clippy and the format-20 release
build also pass; the running format-19 ARM TAP test is left unchanged.

The fresh Server 2025 regression uses AMD64 binary
`8399b59d4e1e5c9e694db30dd48f40c7a4a2d59781b9b4ca2d9f7f3dbbe2400d`
with default seccomp. Cold, full and differential stages pass flushed storage,
DNS, certificate-validated TLS 1.2 HTTPS and byte-checked 2 MiB TAP transfers.
Guest S5 at 08:16:47 UTC ends with Firecracker exit zero. Offline verification
checks three guest/host passes, three exact echoes, no hibernation file and final
65,536-byte data hash
`18f5ddd48023b93d162ee336b7bdecd21939e66b2460f86e85ff898e1f01c57d`.
The previous differential crash is resolved in this regression. The remaining
format-20 guest matrix still requires validation.

Evidence remains in `build/windows/server-tap20/run2/`. Completed memory/state
snapshots and the old failed working disk are removed after closed-file checks.
The crash dump, symbols and logs are retained for comparison. The original
Server baseline is unchanged. The ARM format-20 release is
`f7f80394d1a8ebe3d6319107862b63ad63780b8e4b75793f601c49e2ecc879a4`;
its guest runtime validation remains pending.

Server 2025 also passes the format-20 NIC-free real Tokoloshe CONNECT test.
Cold, full and differential stages each flush and verify a 2 MiB file, echo it
through the allowed tunnel and verify a policy denial. Sealed accounting has
exactly three allowed entries with 2,097,152 bytes each way and three denied
entries with zero bytes. Guest S5 at 08:27:25 UTC ends with Firecracker exit
zero. Offline verification confirms three guest passes, no hibernation and
final data SHA-256
`baf5592a202485ae2a461d8ee683ed7a19cdeadd494c1512df0e3555deb740e0`.
Evidence remains in `build/windows/server-connect20/run/`. Snapshot memory,
state and the disposable Server disk are removed after closed-file checks.
The initial attempt had no console input because the generic background
launcher inherited `/dev/null`; an explicit FIFO input fixes the harness.
That attempt recorded no CONNECT entries and is excluded from the result.

The Toets Windows 10 clone also passes both format-20 paths on the final AMD64
binary. CONNECT passes cold/full/differential 2 MiB storage, allowed echo and
policy denial, with exactly six sealed accounting entries. S5 at 08:36:50 UTC
ends with exit zero. Offline data hash is
`8eb1797b6834d7668a4017b2f355ea1b48d25b17f87822f4a8a3d4110725b562`.
The TAP run then passes all three 65,536-byte storage, DNS, validated HTTPS and
2 MiB echo stages, followed by S5 and exit zero at 08:41:52 UTC. Offline data
hash is `b65a1ec34a9fe1f369ad701ceddd483d43f88c603aec4f5bf8609b356c5b4cfc`.
Both checks verify guest markers, host results and absence of hibernation.

Evidence remains in `build/windows/win10-connect20/run/` and
`build/windows/win10-tap20/run1/`; completed snapshots are removed. The first
CONNECT attempt stopped before traffic because repeated PnPUtil installation
returned 259. Package and installed DriverStore files were compared, then the
prepared clone's startup was changed to run the probe without reinstalling its
existing driver. Its original Toets golden is unchanged. The Win11 clone has
the same verified startup adjustment.

The Toets Windows 11 clone passes both format-20 paths on the final AMD64
binary. CONNECT passes cold/full/differential 2 MiB storage, allowed echo and
policy denial, with exactly six sealed accounting entries. S5 at 08:46:33 UTC
ends with exit zero. Offline data hash is
`e5e3995e1c3038e0a1582711997ebdbf1de1023129cc3d6bb13e18f8bc78ca12`.
TAP passes all three 65,536-byte storage, DNS, validated HTTPS and 2 MiB echo
stages, followed by S5 and exit zero at 08:51:42 UTC. Offline data hash is
`186a35e6a154e0ad5fc6d5429ec55ac027765f913094d207c5f9d60456f7306b`.
Both checks verify guest markers, host results and absence of hibernation.
Evidence remains in `build/windows/win11-connect20/run/` and
`build/windows/win11-tap20/run1/`; completed snapshots are removed after
closed-file checks. The original Toets golden is unchanged. All 87 changed
build inputs match the native source used for the final binaries.

Windows 7 also passes the format-20 cold/full/differential storage, DNS,
validated TLS 1.2 HTTPS and byte-checked 2 MiB TAP workloads. S5 at
08:58:39 UTC ends with Firecracker exit zero. Read-only disk inspection checks
three guest markers, three host reports and echoes, no hibernation file and
final 65,536-byte SHA-256
`74c241f4d0ae0660d0ee51cdcd2bc5e92be98662a566e4849c10522b1ef500cb`.
Evidence remains in `build/windows/win7-reset20/run1/`; completed snapshots
are removed after closed-file checks.

Linux PVH cold/full/differential regressions pass on the same format-20 AMD64
binary with modern PCI and transitional INTx. Each checks nine successive
65,536-byte flushed storage writes and read comparisons across the three
stages. Evidence remains in `build/windows/linux-reset20/{modern,intx}/`.
The disposable root disk and snapshot files are removed after closed-file
checks. These storage checks make no guest shutdown or network claim.

The ARM format-19 TAP retry completes all three workloads with the PCI reset
and eventfd seccomp fixes. Offline verification confirms three guest and host
passes, three byte-checked 2 MiB echoes, SYSTEM identity, no hibernation and
final 65,536-byte SHA-256
`e52961747db8ed5ff1f33da0bed6a1e396831bb9feef92653ac64d4e40ca2d19`.
The transcript closes, but Firecracker does not report guest shutdown before
the controller's five-minute limit. The controller exits one and stops the VM.
This run establishes workload recovery, not clean shutdown. Evidence remains
in `build/windows/arm19-tap/run/`; completed snapshots are removed.

The format-20 TAP rerun uses the same disk, firmware, probe and configuration
with binary `f7f80394d1a8ebe3d6319107862b63ad63780b8e4b75793f601c49e2ecc879a4`.
It retains the 5,400-poll workload bounds and allows 30 minutes for shutdown
under emulation. Its clean-shutdown result and the final format-20 CONNECT
check remain pending.

XP SP3 x86 passes the format-20 cold/full/differential regression on the final
AMD64 binary and documented SeaBIOS firmware. Each stage verifies 65,536-byte
storage, HTTP status 200 and a byte-checked 2 MiB TAP transfer. S5 at
09:01:35 UTC ends with Firecracker exit zero. Offline inspection checks the
installed NetKVM driver against its source, no hibernation file, and every
byte of the final storage pattern with seed 134640. Final SHA-256 is
`79d40c1bb981a9b547b33596677a41c7693df04a639359f460bb93addcb7e436`.
Evidence remains in `build/windows/xp-reset20/run2/`; completed snapshots are
removed after closed-file checks. This XP result makes no TLS, vsock or SMP
claim. The final AMD64 guest matrix is complete; format-20 ARM runtime
validation and final cleanup remain pending.

The format-20 ARM binary also passes Linux direct boot and full/differential
restore with two vCPUs, 256 MiB and default seccomp. It uses the same
`7.2.8-tokoloshe-host-8` kernel as the format-19 regression, SHA-256
`26e340aa9633c1fcbc904c368f4e0baabc84bf27ed1038a89fc4da2648541139`.
The existing Rust counter probe is rebuilt for AArch64 musl with warnings
denied. Cold boot prints counters 0-2, full restore continues at 3-5, and
differential restore continues at 6-8. Neither restore repeats the boot
marker. The controller exits zero and stops its final VM. This test makes
no Linux storage, network or guest-shutdown claim. Logs and input checksums
remain in `build/windows/arm-linux20/`; temporary initramfs, executable and
snapshots are removed. The independent Windows TAP run remains active.

The first format-20 ARM TAP run activates Windows, reports licensed state
and NetKVM Up, resolves DNS and receives HTTPS 200. Its echo helper then
exits with `WouldBlock` after a 120-second gap between payload chunks.
The controller stops Firecracker and exits one; this is not a workload or
shutdown pass. Logs remain in `build/windows/arm20-tap/run1-echo-timeout/`.

The echo helper now accepts a socket timeout, retaining 120 seconds for
existing callers. A delayed-sender test reproduces failure with a one-second
timeout and a two-second gap, then passes with a five-second timeout.
The ARM retry uses 5,400 seconds, matching its existing overall stage bound.
The controller still stops a stage that exceeds that bound. Payload size,
guest byte comparisons, Firecracker binary and shutdown requirements are
unchanged. The retry and final CONNECT result remain pending.

The format-20 ARM TAP retry passes cold boot, full restore and differential
restore with the same production binary and default seccomp. Each stage
checks flushed storage, DNS, validated HTTPS and the complete 2 MiB TCP
payload. The guest requests shutdown: KVM reports system event type 1 at
12:29:01 UTC and Firecracker exits zero at 12:29:05. The controller exits
zero and restores forwarding to its previous disabled state.

Offline verification confirms SYSTEM identity, no hibernation file, exactly
three guest passes and host echoes, and the final 65,536-byte SHA-256
`7b4c0e5f97fd0dd6285c3ca58bca6dc2f1f1f87710b029e7dd787ac89059deb5`.
Logs, transcript, final data and verification output remain in
`build/windows/arm20-tap/run/`. Completed memory and state snapshots are
removed after closed-file checks.

The final CONNECT run uses the same guest disk after guarded probe replacement,
with no network interface. Its first harness attempt fails before Firecracker
starts because the unprivileged proxy cannot traverse the appliance's root-only
parent directory. A temporary bind mount exposes only the owned test subtree
at `/run/fc-windows-tests`; parent permissions remain unchanged. The proxy is
ready and the retry is running. CONNECT results and final cleanup remain
pending; the bind mount must be removed after the test.

## Final validation and cleanup, 2026-10-14

The final ARM64 CONNECT retry passes all three stages. KVM reports guest
shutdown at 13:11:58 UTC and Firecracker exits zero at 13:12:02. Offline
verification confirms SYSTEM identity, no hibernation file, three guest
passes and six sealed proxy records: three allowed tunnels with 2,097,152
bytes in each direction, and three denied destinations with zero payload.
The final 2 MiB file has SHA-256
`88cdff315ab7d9d6d363e3575b1346eb814acd5d77c4ee2cf515f966bbd834b3`.
Evidence remains in `build/windows/arm-connect20/run/`.

The final matrix uses snapshot format 20, with default seccomp. Each Windows
row passes cold boot, full restore, differential restore, flushed storage
with readback verification, and guest shutdown with Firecracker exit zero.

| Guest | Conventional TAP egress | Tokoloshe CONNECT over vsock | Evidence under build/windows |
|---|---|---|---|
| XP SP3 x86, one vCPU | HTTP and 2 MiB TCP echo | Allowed echo and policy denial | xp-reset20/run2; xp-legacy-connect/run |
| Windows 7 SP1 x64 | DNS, validated HTTPS and 2 MiB TCP echo | Allowed echo and policy denial | win7-reset20/run1; win7-legacy-connect/run |
| Windows 10 Enterprise LTSC 2021 x64 | DNS, validated HTTPS and 2 MiB TCP echo | Allowed echo and policy denial | win10-tap20/run1; win10-connect20/run |
| Windows 11 IoT Enterprise LTSC 2024 x64 | DNS, validated HTTPS and 2 MiB TCP echo | Allowed echo and policy denial | win11-tap20/run1; win11-connect20/run |
| Windows Server 2025 Core x64 | DNS, validated HTTPS and 2 MiB TCP echo | Allowed echo and policy denial | server-tap20/run2; server-connect20/run |
| Windows 11 IoT Enterprise LTSC ARM64 | DNS, validated HTTPS and 2 MiB TCP echo | Allowed echo and policy denial | arm20-tap/run; arm-connect20/run |

AMD64 Linux modern PCI and transitional INTx storage/restore regressions
also pass. ARM64 Linux direct boot and counter continuation across both
restore types pass. These checks do not establish FreeBSD runtime support,
XP TLS or SMP, native ARM performance, or a Windows
port of the Tokoloshe job runtime. Original Toets golden disks remain
unchanged. The final native source-input audit verifies all 87 recorded
inputs; no production code changed after the CET fix.

Completed RAM snapshots, disposable Windows test disks, the superseded
Server working disk, crash dump and downloaded installation ISOs are removed
after closed-file checks. Logs, transcripts, final data, firmware, toolchains
and reusable baseline images remain. The ARM state filesystem is trimmed;
the outer QEMU appliance shuts down cleanly and its controller exits zero.
No Firecracker, QEMU appliance, probe or proxy process remains on the test
host. The temporary bind mount is removed, the test TAP is removed, and
forwarding is restored. All required runtime validation and cleanup are
complete for this experimental implementation.

## Reopened: XP and Windows 7 vsock validation

The previous completion statement was premature. XP and Windows 7 must also
pass the real Tokoloshe CONNECT checks; their TAP results do not satisfy this
requirement. The goal remains active until this work is complete or a concrete
blocker is reported.

1. [x] Audit historical Viosock source, kernel APIs, KMDF requirements and
   build tools for Windows 7 x64 and XP x86. Record pinned inputs.
2. [x] Build or obtain compatible driver packages and a compatible CONNECT
   probe. Record signing requirements and distinguish test builds from
   deployable signed packages.
3. [x] Create isolated guest disks from retained baselines. Install drivers
   and verify device start under Firecracker with default seccomp.
4. [x] Verify allowed 2 MiB bidirectional CONNECT and denied destinations
   without a guest NIC, on cold boot and full/differential restore, for each
   guest. Require guest evidence and sealed real-proxy accounting.
5. [x] Verify clean shutdown, final storage data and Linux/modern Windows
   regressions for any production changes. Update the matrix and remove
   disposable disks and processes.

The resumed audit pins historical Viosock source to
`8833d0a1c2d6431d58300a00b402708c3628b29f`. Its Windows 7 targets use KMDF
1.9. All WDF function names used by the socket driver and VirtIO-WDF occur
in the official WDK 7.1 headers. This checks API names, not build or runtime
compatibility. Microsoft documents [KMDF 1.9 support for XP and later](https://learn.microsoft.com/en-us/windows-hardware/drivers/wdf/kmdf-version-history).
The downloaded WDK 7.1 ISO has SHA-256
`5edc723b50ea28a070cad361dd0927df402b7a861a036bbcf11d27ebba77657d`.
An isolated Windows build disk is prepared for the historical driver build.

A no-std Rust probe builds for i686 with warnings denied and PE subsystem
5.1. Its imports are limited to kernel32, ws2_32 and msvcrt. The first XP
run returns error 123 when opening the NT device path through CreateFile.
The corrected probe uses the Win32 device namespace. XP and Windows 7 then
both run it and return error 2 with exit status 1, as expected before driver
installation. Both guests request S5 and Firecracker exits zero. Offline
inspection verifies the installed executable and recovered error logs,
with no hibernation file. Evidence is in `build/windows/legacy-vsock/`.
These are probe compatibility checks, not CONNECT passes. Driver build,
installation and cold/full/differential CONNECT tests remain open.

The Windows 7 x64 driver now builds with EWDK 19041 and KMDF 1.9. Its
isolated test package uses a local SHA-1 test certificate; Windows test
signing is enabled. This is not a production-signed package. The generated
INF registers the Winsock provider. Device Manager reports code 0 under
Firecracker with default seccomp and no guest NIC.

The CONNECT probe found three receive defects in the pinned historical
driver. Upstream fixes are being backported and tested in sequence:

- `661fc34f1c64e442e459eeecef4cf132ed4a89cb`: release the read callback guard
  when the request queue is empty. Before this fix, only the first HTTP
  response byte arrived.
- `a48f19c69e47f1200cbb8d867b69b6a1db227a20`: advance the buffer position
  before clearing the partial-read byte count. Before this fix, successive
  reads returned the first response byte repeatedly.
- `ed92ff4314d01ccf8c6f52707803c363899ee017`: return receive credits before
  clearing the consumed buffer length. With the first two fixes, the guest
  receives HTTP 200, but stalls after 262,144 payload bytes in each direction.
  The sealed real-proxy accounting confirms those counts. With this fix,
  cold boot passes the full 2 MiB transfer and denied-destination check.
- `124d09620663a9ac62edbf9615091c32ec3a84f8`: validate reset events against
  the event structure size, not the pointer size. The old x64 driver rejects
  the four-byte reset event after full restore. With this fix, cold boot and
  full restore both pass the workload. Differential restore then fails with
  WDF bugcheck `0x10D`, parameter 1 equal to 3. The DMA fix below resolves
  this failure.

The receive-failure runs end with guest shutdown and Firecracker exit 0.
The differential-restore crash requires a host stop and is not a pass.
XP driver installation and its CONNECT restore matrix remain open.
Evidence is retained under
`build/windows/win7-legacy-connect/` on the test host.

The XP build uses the official WDK 7.1 headers, KMDF 1.9 and XP import
libraries with the EWDK compiler. A minimal header probe reproduces a
conflict between the WDK's old 64-bit interlocked inline definitions and
the compiler's intrinsics. The WDK's `NONTOSPINTERLOCK` switch removes the
conflicting definitions; the probe and both VirtIO libraries then build.
The socket driver still needs legacy-header integration. No XP driver
runtime result is claimed.

### Windows 7 CONNECT matrix passed

Two differential-restore runs reproduce the WDF violation. The recovered
kernel dump identifies `VIOSockTxVqProcess`, which completes a send request
while its DMA buffer still has a framework reference. The historical DMA
helper can defer transaction deletion while its callback holds a reference.
It now calls `WdfDmaTransactionRelease` before dropping its own reference
and before request completion. This follows Microsoft's documented
[DMA completion order](https://learn.microsoft.com/en-us/windows-hardware/drivers/wdf/completing-a-dma-transaction).

The resulting test-signed Windows 7 driver has SHA-256
`4e1c2166870389568c5d1175c9693a634a11fa8b86dedd1f797d43e83618aab7`.
Installation returns zero, the device reports code 0, and offline comparison
confirms the installed driver matches that package.

With default seccomp and no guest NIC, cold boot, full restore and
differential restore each pass the real Tokoloshe CONNECT workload. The
sealed accounting contains exactly three allowed transfers, each with
2,097,152 bytes in each direction, and three policy denials with zero payload.
The guest transcript contains one pass for each iteration. Windows requests
ACPI S5 and Firecracker exits zero. Offline validation finds no hibernation
file and verifies the final 2 MiB storage file against SHA-256
`4de64b22c8a9abd3869ba22348a32ed0936f31907553c4ad93c1740b10774d09`.
Evidence is retained locally and on the host under
`build/windows/win7-legacy-connect/run/`. Completed RAM snapshots are removed.

The XP kernel driver also builds with warnings treated as errors, using
the XP import libraries and KMDF 1.9. Its Winsock provider build and runtime
validation remain open.

### XP CONNECT matrix passed

The XP provider now builds against the WDK 7.1 static C runtime. Import
checks against the actual XP system files pass for both the driver and
provider. The kernel and user builds treat warnings as errors. Catalog
generation for XP x86 and signature verification both pass. The package
uses a local test certificate, not a production driver signature.

The installed XP driver has SHA-256
`c02248900ed76be3ccf8a62221cdbc6cfe942013b5e3052ce4da39854370a709`;
the provider has SHA-256
`42f433c6f5c4bc7133b490daaaf28d7f51624aeb7a19128e3459d966db1e1338`.
Installation returns zero and the device reports error code 0. Offline
comparison confirms both installed files match the signed package.

With one vCPU, default seccomp and no guest NIC, XP passes cold boot, full
restore and differential restore through the real Tokoloshe proxy. Each
iteration verifies 2,097,152 bytes in each direction and HTTP 403 for the
denied destination. The sealed log contains exactly six entries: three
allowed transfers and three denials with zero payload. The guest records
three pass markers, requests ACPI S5, and Firecracker exits zero. Offline
verification finds no hibernation file and checks every byte of the final
2 MiB storage file. Its SHA-256 is
`4c3a318d2b13a0f389308c295f677f539ccfd08bd5b1fc7ba3396755124b196e`.

The first successful run is retained under
`build/windows/xp-legacy-connect/run1/`. A final repeat under `run/` also
passes all checks and records `CONNECT_RESULT=0`. Its guest requests S5 at
15:03:05 UTC and Firecracker exits zero. This repeat verifies the corrected
batch launcher: output redirection precedes the exit-code echo, so CMD
cannot interpret the final digit as a file handle. Evidence is retained
locally and on the host.

The ordered driver patches, pinned inputs, build and packaging scripts,
installers, guest probes and final-data verifier are preserved in
[tools/windows/legacy-vsock](../tools/windows/legacy-vsock/README.md). The
patch series applies cleanly to the pinned upstream revision; both probes and
installers compile from the preserved sources. No production Firecracker
code changed during the legacy Viosock work, so the preceding Linux and
modern Windows regression results still apply.

### Legacy validation cleanup

The XP and Windows 7 working disks, build VM, toolchain ISOs and completed
RAM snapshots are removed after checking that no process holds them open.
This releases about 35 GiB from the legacy build/test directory, plus the
completed snapshot files. The crash dump is compressed from 184 MiB to
41 MiB; its debugger analysis, matching symbols, signed driver packages,
build logs and runtime evidence remain. No owned Firecracker, QEMU builder,
probe or proxy process remains, and the test disk mount and loop devices
are gone. The unrelated Windows QEMU VM on the host is untouched.

Both legacy vsock matrices are complete. The Windows scope remains the
experimental Firecracker platform and verified egress; production signing
of the custom legacy drivers and a Windows Tokoloshe job runtime are not
provided by these tests. XP TLS and SMP remain outside the verified matrix.
