# Windows guest validation

Windows support is experimental. Use the [AMD64 guide](windows.md),
[installation console](windows-console.md) and
[secure platform guide](windows-secure-boot.md) for configuration.

## Current integration candidate

The candidate uses snapshot format 33. Windows hardware and firmware require the
default-off `windows` Cargo feature. Both builds retain the saved-state fields;
the default build rejects Windows configuration before opening its backends.
Ordinary Linux snapshots must restore between the two builds.

The current correction adds conventional PC GPE status/enable state and routes
VM Generation ID notification through the existing SCI. Default and Windows
release builds pass. The following checks used Rust 1.98.1 on native AMD64
Linux/KVM.

### Current host checks

The full unit suites used the current production source. Subsequent changes were
confined to test configuration and error assertions; both affected tests pass on
the final source. Standalone-tool results cover unchanged implementation inputs.
The functional checks use the current release VMMs.

| Check                                                  | Result                                          |
| ------------------------------------------------------ | ----------------------------------------------- |
| Default VMM unit suite                                 | 891 passed; none ignored                        |
| Windows VMM unit suite                                 | 1,098 passed; none ignored                      |
| Privileged jailer mount and retained-namespace parents | Both passed, including their child protocols    |
| Variable helper (unchanged inputs)                     | 67 passed; none ignored                         |
| Console helper (unchanged inputs)                      | 31 passed; none ignored                         |
| Strict workspace Clippy                                | Default and all features, GNU and musl          |
| Strict ARM64 cross-Clippy                              | GNU default/all features; command 0, run failed |
| Standalone tool Clippy and Rust/Markdown style gates   | Passed                                          |
| Platform devices                                       | 7 passed                                        |
| Selected API and boot tests                            | 65 passed; 2 empty CPU-template skips           |
| Selected basic Linux snapshot tests                    | 36 passed                                       |
| Selected Linux snapshot-cycle tests                    | 12 passed in one invocation                     |

Both strict ARM64 GNU cross-Clippy commands exited zero. The enclosing run and
its post-run guard failed because compiler-cache allocation exceeded the fixed
cap. Later cache custody checks and retirement do not change that failed result.
These command results establish compilation checks only.

The VMM suites execute the real swtpm restore, ACPICA evaluation and maximum
optional-buffer snapshot-reader tests. The console backlog regression passes;
replacing only its connector with the preceding blocking call reproduces the
five-second failure. Normal discovery ignores two privileged jailer parents and
their two child entrypoints. Both parents were then run explicitly with the
required privileges; they invoke the children. These results do not imply that
every ignored documentation example was executed.

The selected API/boot tests have 65 passes and two empty CPU-template skips; no
custom CPU-template coverage is claimed. All 12 selected cycle cases pass in one
invocation with unchanged test thresholds. An earlier cycle run failed on a
2,317 ms pause API call. Its cause remains unresolved. Diagnostic tracing of the
passing run recorded no owned Firecracker executions and provides no usable
trace; the pass does not establish a scheduler or storage-latency bound.

### Current Windows 7 reference

On the current format-33 implementation, an installed Windows 7 Enterprise SP1
x64 reference retains the installed SSU and SHA-2 updates after a normal
restart. NetKVM 61.77.104.17200 and Microsoft's generation-counter driver
6.3.9600.18080 retain device Code 0, `IsSigned=True`, the pinned selected
INF/SYS and running services. The KMDF runtime is running. Two actual driver
queries return the same nonzero 16-byte generation ID, followed by normal
shutdown.

This installed reference also passes the maintained optional TAP generation
checks on PowerShell 2 through cold boot, full restore and differential restore
with format-33 snapshots under normal Windows driver policy. The SYSTEM startup
task invokes the public `tools/windows/egress/tap.ps1` directly with
`-GenerationChecks`. Each phase checks DNS, certificate-validated TLS 1.2 HTTPS,
a 2 MiB equal TCP echo in each direction and matching storage hashes. Before
each snapshot, an owned child reports ARMED while alive; after restore, that
same child PID completes with a changed generation ID matching two fresh driver
queries. The public verifier accepts seven matching guest/host reports, three
distinct nonzero IDs and both original child completions.

On this reference, an incorrect probe digest, an existing event directory and an
existing transcript are each rejected. All three checks leave the NIC
configuration, earlier transcript and storage unchanged, preserve existing event
files where present, send no reports and request normal S5.

The cold and full source instances stop at the normal controller snapshot
boundaries. The final differential instance requests S5 and the VMM exits zero.
Host process, mount, loop and network closure checks pass. These TAP results
qualify only this installed reference and route.

A separate clone of this installed reference passes format-33 cold, full-restore
and differential-restore CONNECT checks using the documented external
test-signed Viosock package. It retains the isolated test-signing policy without
an integrity-check bypass. A SYSTEM startup task opens fresh connections through
the real Tokoloshe HTTP CONNECT proxy over vsock, with no guest NIC. Each phase
passes a 2 MiB equal transfer in each direction and an HTTP 403 policy denial
with no payload. Accounting seals three allowed and three denied entries; the
final disk file matches its guest SHA-256. Both states report v33.0.0. Cold/full
instances stop at snapshot boundaries; the final instance requests S5 and the
VMM exits zero. Host process, mount and loop closure checks pass. This qualifies
only the installed Win7 test-signing profile and fresh connections.

### Current Windows XP reference

On the current format-33 implementation, the installed XP Professional SP3 x86
reference has the genuine Microsoft generation-counter package 6.3.9600.17831.
Separate read-only catalogue-member checks accept its installed INF and SYS; WMI
records the selected signed driver. After a distinct cold boot, the device has
Code 0, `gencounter` and KMDF are running, and two actual reads return the same
nonzero 16-byte generation ID. The guest then requests normal S5 and the VMM
exits zero. The stopped disk, host processes, mounts and loops are closed.

A separate installed-reference clone passes format-33 cold, full-restore and
differential-restore TAP checks with the
[shared polling reader](../tools/windows/generation/README.md#xp-polling-during-egress).
Six genuine driver queries form three matching pairs with distinct nonzero
generation IDs. Each phase passes a 2 MiB equal TCP echo in each direction and
hostname HTTP with status 200. The offline verifier accepts all 65,536 storage
bytes. Both saved states report v33.0.0. The final guest requests normal S5 and
the VMM exits zero. Host process, mount, loop and network closure checks pass.

The TAP run uses the vendor-signed NetKVM package with normal XP driver-warning
consent. These checks do not establish WHQL status or installed catalogue
registration. The separate default driver-policy signature check fails. The
original offline collector failed on a trailing-space marker. A separate parser
accepts the retained actual bytes after the marker correction; the original
collector remains failed and was not rerun. Those results qualify the TAP route
on that installed reference.

A separate installed-reference clone passes format-33 cold, full-restore and
differential-restore CONNECT checks without a guest NIC. The real Tokoloshe
proxy connects to the host-loopback echo target. Each stage passes a byte-equal
2 MiB transfer in each direction and a policy-denied request with status 403 and
zero payload. Six sealed FetchLog entries bind those transfers and denials to
the three stages. Six genuine generation-counter polls form three matching pairs
with distinct nonzero IDs. The unchanged offline verifiers accept the final 2
MiB storage pattern and normal guest S5 with VMM exit zero.

This run binds source-manifest SHA-256
`836e743b99a7f62e8013267c4ccd33027f72b029291d9f607e9f44d3261b10aa` and
build-receipt SHA-256
`6033bf6a0f48b6aaa36b9501d04528eb2d5b07f38d95b6bbc1aa6cc96a037772` to these
recorded binaries:

| Input             | SHA-256                                                            |
| ----------------- | ------------------------------------------------------------------ |
| VMM               | `c1b1d608138f147c1d8c3314ebdfbf40abad824fb9838412927d9235714cb29f` |
| Snapshot editor   | `0bbe760cf233d66f27725639c5dba8a9da7fe595b92ff4942cb9aadcb9b7f370` |
| XP CONNECT reader | `a04d5ecd8d931095e557bcad4339af87f69fed21cb5a1b1d4eb090368d17d0f1` |
| Host proxy shim   | `0ff7f14d5aca46306347f3ce6d5713cedefe95a0162085a46ad0f0bad6641270` |
| Tokoloshe sidecar | `5d4cc8b4f7896dfe59dccfe0bb149708b297ed40adc8cf4502d7f80fa3498e78` |

The original runtime observer failed to count the non-dumpable VMM through
`/proc/<pid>/exe`; its failed main unit and absent result remain unchanged. A
separate review accepts the actual controller exit zero, launch-chain UID
inheritance and guest S5. Stopped raw collection and the ordinary verifiers then
qualify the transfers, accounting, generation polls and stored bytes. This does
not turn the original observer failure into a pass. The Viosock package remains
an external test driver; these results establish neither WHQL status nor normal
production signing. They do not qualify arbitrary Internet targets, TLS,
generation-change notifications or later changed source, binaries or helpers. XP
supports generation polling only; its notification request is unsupported.

A pre-installation XP console baseline, with VgaSave running, separately
verifies the 256x128 bitmap, four pointer-selected pixel changes and a saved
194-byte file containing all 95 printable ASCII characters with Caps Lock off
and on. It uses raw console CLI commands and 640x480 frames. The original bitmap
and staged payloads remain unchanged. This qualifies that installed reference
and input sequence, not browser or VNC input, every layout, mode, guest or
restored input path. The baseline's full PnP inventory retains the unconfigured
VGA device at Code 1 and VMClock at Code 28; VGEN reports Code 0 with service
`gencounter`.

A later isolated VGA installation completed its normal driver wizard and
required a restart. The display remained black on the subsequent cold boot; that
experiment does not qualify the display driver. Legacy VMClock recognition and a
clean device inventory also remain open.

Current Windows installation, the remaining graphical input and display matrix,
security, the other guests' TAP routes and performance qualification remain
open. The historical results below use format 31 and their matching binaries.
They do not qualify formats 32 or 33. Do not load an older saved state with the
candidate.

The variable helper is Rust and reuses the upstream AWS-LC versions and
checksums: `aws-lc-rs` 1.18.0 and `aws-lc-sys` 0.44.0. Their existing backend
contains C/assembly. No QEMU library or new linked native backend is used. The
TPM backend is the separate swtpm/libtpms service; Firecracker does not link
libtpms. Firmware remains external, with the supplied patches and original
licences.

## Historical AMD64 guest results

These tests used VMM SHA-256
`4424dc69822e105abee765621549e38fe898d8a0e70a7c31076c931fff435171` and format-31
snapshots. Each installed-reference network result covers cold boot, full
restore and differential restore.

| Guest                                 | Build      | TAP  | Tokoloshe CONNECT over vsock | Scope                                                         |
| ------------------------------------- | ---------- | ---- | ---------------------------- | ------------------------------------------------------------- |
| XP Professional SP3 x86               | 2600       | Pass | Pass                         | Installed reference; `Unsafe` disk cache; HTTP only           |
| Windows 7 Enterprise SP1 x64          | 7601       | Pass | Pass                         | Installed reference; `Writeback`                              |
| Windows 10 Enterprise LTSC 2021       | 19044.1288 | Pass | Pass                         | Installed reference; `Writeback`                              |
| Windows 11 IoT Enterprise LTSC        | 26100.1742 | Pass | Pass                         | Installed reference; `Writeback`                              |
| Windows 11 Enterprise Evaluation 26H2 | 26300.9457 | Pass | Pass                         | Fresh direct installation and protected platform; `Writeback` |

TAP has no guest vsock device; CONNECT has no guest NIC. TAP checks DNS and a 2
MiB equal echo in each direction. Guests other than XP also check HTTPS with
certificate validation and TLS 1.2. CONNECT uses the actual Tokoloshe proxy,
with three allowed transfers and three zero-payload denials per run. Host
accounting and offline disk checks bind the results to the three phases. All
listed runs finish with clean guest S5 and VMM exit zero.

XP and Windows 7 use the documented external test-signed Viosock packages. The
format-31 Windows 7 inventory retains three ACPI Code 28 devices. Current
Windows 7 generation-counter binding, polling and TAP restore results are
described above. Current XP driver-query, TAP and CONNECT restore results are
also described above, with their separate installed references, tested inputs
and failure qualifications. XP results establish polling only. Historical egress
results do not prove six generation reads across restores. VMClock recognition
on modern Windows establishes enumeration only. These installed references do
not establish fresh XP, Windows 7, Windows 10 or IoT installation, or every
graphical input path. Process/snapshot cache checks do not establish host
power-loss durability. XP HTTP does not establish TLS support.

The historical format-31 Enterprise run uses variable helper SHA-256
`0b2b94dbd74f84d9befa0f2747d812f580fc01e760843676b46b4a0373d45f3c` for the
protected runtime checks, and secure firmware SHA-256
`370a2a969758d361008decb59bdf59072664e273c3402f9b4f931d7511515e43`. Setup stages
the signed storage driver automatically, without a hardware-check bypass, and
the installed guest boots without its DVD. Secure Boot and TPM readiness pass.
The final inventory reports 53 present devices without problem codes after the
unmodified signed VMClock recognition INF and Viosock package are installed.

Its unattended SYSTEM task passes both network routes after full and
differential restores. The protected checks retain PCR7, a persistent TPM object
and PK/KEK/db/dbx digests; authenticated private-variable updates, deletion,
tamper, wrong-owner and replay checks also pass. The stopped disk and
TPM/variable stores form a matched set. An earlier four-second variable-service
stall remains unexplained; the helper fix and subsequent passing boots do not
establish its cause. The host tests do not establish an untraced storage-latency
or reclaim/OOM bound.

## Reproduce

Follow the [normal test environment](../tests/README.md) and run the maintained
tool and platform tests:

```sh
./tools/devtool test -- \
  integration_tests/build/test_unittests.py::test_tool_unittests \
  integration_tests/build/test_clippy.py::test_tool_clippy
./tools/devtool test -- \
  integration_tests/build/test_unittests.py::test_unittests \
  integration_tests/build/test_clippy.py::test_rust_clippy
./tools/devtool test -- integration_tests/functional/test_platform_devices.py
```

Omit `--binary-dir` for the platform module so its fixtures build both the
Windows and default counterparts. See the
[test guide](../tests/README.md#standalone-tools-and-platform-devices) for the
required swtpm/ACPICA inputs and standalone workspaces.

For Windows runtime tests, prepare authorised media with the
[media tools](../tools/windows/README.md), then use the maintained
[egress controllers and offline verifiers](../tools/windows/egress/README.md).
Use an independent disk and matching stopped TPM/variable stores. Complete
driver installation before testing. Record actual VMM, firmware, media and
driver hashes, guest build, snapshot format and host kernel. Preserve phase
logs, proxy accounting, public security baselines and offline data hashes.
Screenshots and process exit status alone do not establish network or restore
success.

## Open qualification

Standard Windows 11 Pro, Server Core and Desktop, and the remaining fresh
installations remain unqualified. The current format-33 guest matrix is
incomplete. The earlier memory comparison against fork baseline `658ff27e`
exceeded the normal performance gates. The final candidate must pass unchanged
gates against the registered consumer base `c6655e5`, with default and Windows
builds measured separately.

The native AMD64 host runs Linux 7.0.0-60009-generic. Its results do not qualify
the supported host-kernel matrix. [ARM64 results](windows-arm64.md) are
historical emulated tests; current cross-builds do not qualify native ARM64
Windows installation, input, security or egress. Cross-host migration and
firmware-stage snapshots also remain unvalidated. Tokoloshe's Windows job
runtime is outside this guest-platform and egress scope.
