# Windows guest validation

Support is experimental and applies to the tested editions and builds. Use the
[AMD64 guide](windows.md), [installation console](windows-console.md) and
[secure platform](windows-secure-boot.md) for operator instructions.

## AMD64 matrix

These native Linux/KVM Firecracker tests use snapshot format 28. Each guest
passes cold boot, full restore and differential restore through both TAP and the
real Tokoloshe HTTP CONNECT proxy over vsock. Every phase establishes new
host-recorded traffic and flushed storage. Each differential restore ends with a
clean S5 shutdown. CONNECT accounting records three allowed transfers and three
zero-payload denials per run.

| Guest                                               | Build       | TAP  | CONNECT |
| --------------------------------------------------- | ----------- | ---- | ------- |
| Windows XP Professional SP3 x86                     | 2600        | Pass | Pass    |
| Windows 7 Enterprise SP1 x64                        | 7601        | Pass | Pass    |
| Windows 10 Enterprise LTSC 2021                     | 19044.1288  | Pass | Pass    |
| Windows 11 IoT Enterprise LTSC                      | 26100.1742  | Pass | Pass    |
| Windows 11 Enterprise Evaluation 26H2               | 26300.9457  | Pass | Pass    |
| Server 2025 Standard Evaluation, Desktop Experience | 26100.32230 | Pass | Pass    |

Full present-device inventories in the Windows 10, IoT Windows 11 and Server
runs report zero device problem codes.

Modern guests use the corrected
[CloudHv memory map](../resources/firmware/pvh-memory-map.md). All six profiles
use the same pinned Rust 1.98.1 VMM. Its SHA-256 is
`ee9a558f79ac76284600aa58b53561ba873e2c64caab5803bdb74e3f76af8ac6`. The
secure-variable helper SHA-256 is
`9a79eb2d3e741adf6f8592747336181d13b53b9532fefa4b3397a00defcd8347`.

All six profiles completed fresh graphical installation and disk boot without
optical media before this format-28 review. A separate blank-disk Server Desktop
Experience install passes with the final corrected firmware, boot viostor
driver, retained system UUID, input and S5. VNC/noVNC input checks cover
keyboard modifiers, lock keys, keypad, pointer targets, reconnect and
full/differential restore.

Standard Windows 11 retains Secure Boot, Get-Tpm readiness, successful TBS
GetRandom, SHA-256 PCR 7 and an existing persistent TPM object's public identity
across all six network phases and the intervening cold boot. Its public TPM
baseline is byte-identical to the prior format-27 baseline. Read-only encrypted
volume checks verify stored data. The image has BitLocker protection off and no
protectors; these results do not qualify TPM-sealed recovery or attestation.
Windows 11 Pro is untested. CIM identifies Windows 11 Enterprise Evaluation; the
legacy registry ProductName reports Windows 10 Enterprise Evaluation. The
Enterprise Evaluation desktop reports an expired licence; no activation changes
were made.

XP and Windows 7 use the documented custom test-signed
[legacy vsock packages](../tools/windows/legacy-vsock/README.md). XP's TAP probe
uses HTTP; the modern TAP probes use certificate-validated TLS 1.2. Legacy
Windows retains documented device warnings. The AWS VMClock recognition INF
establishes enumeration only, not guest consumption of VMClock counters.

The matrix records the executable hashes above. Later helper recovery and
paused-failure corrections have separate unit and jailed lifecycle regressions;
the installation and network matrix has not been rerun with those executables.

## Reproduce

Prepare authorised media with the [media tools](../tools/windows/README.md),
then run the
[egress controllers and offline verifiers](../tools/windows/egress/README.md).
Use a disposable disk and matching stopped TPM/variable stores. Keep one probe
per guest; finish driver installation in a separate boot before testing. Record
VMM, firmware and media hashes and preserve phase logs, public TPM baselines,
accounting and data hashes. Do not use screenshots or exit status alone as proof
of network or restore success.

Protocol regressions cover actual UEFI memory descriptors and nine malformed PVH
map fixtures, real swtpm export/import, authenticated-variable policy,
physical-presence confirmation/refusal and helper failure. The final native VMM
suite passes 1,032 tests with no failures or ignored cases, including real
ACPICA and swtpm integration. TAP cases run with the required privileges in an
isolated network namespace. The maximum optional-buffer snapshot fixture remains
29,474,015 bytes under the 32 MiB reader limit. Helper and console suites pass
31 and 29 tests, respectively. AMD64/ARM64 all-target strict Clippy and pinned
formatting pass. Tool regression fixtures cover stale restore markers, large
producer output and mount/loop cleanup with preserved verification and signal
failures. The normal pytest build gates cover the standalone tools. ACPICA and
swtpm tests run by default; jailed platform lifecycle tests use a distributable
PVH fixture without Windows media. See the
[test guide](../tests/README.md#standalone-tools-and-platform-devices).

Ordinary Linux validation uses the existing API, seccomp, boot-timer and memory
suites with Linux 6.1.188 and Amazon Linux 2023. The PMEM regression reads a
real 8 MiB guest mapping before and after restore. The overhead monitor excludes
these guest pages and keeps its original VMM limits. The sparse snapshot oracle
uses memfd so filesystem extent metadata cannot alter its exact block counts.

On one AMD64 host, five 1-vCPU/128-MiB full-restore samples per transport show
mean overhead RSS increases of 425,165 bytes for PCI and 552,960 bytes for MMIO
against the fork base. The default ACPI/RTC platform also adds six descriptors.
This is measurable Linux overhead. These measurements do not qualify the full
supported-host, security or performance matrix, or establish upstream
acceptance.

## Other architectures and limits

[ARM64 validation](windows-arm64.md) records separate emulated Linux/KVM tests.
AMD64 results and ARM compilation do not qualify native Windows ARM performance.
Cross-host migration and firmware-stage snapshots are unvalidated. Tokoloshe's
Windows job runtime is outside this guest-platform and egress scope.
