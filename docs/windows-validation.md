# Windows guest validation

Windows support is experimental. Use the [AMD64 guide](windows.md),
[installation console](windows-console.md) and
[secure platform guide](windows-secure-boot.md) for configuration.

## Current integration candidate

The candidate uses snapshot format 32. Windows hardware and firmware require the
default-off `windows` Cargo feature. Both builds retain the saved-state fields;
the default build rejects Windows configuration before opening its backends.
Ordinary Linux snapshots must restore between the two builds.

The following checks passed with Rust 1.98.1 on native AMD64 Linux/KVM, using
implementation commit `1007215d9bd6641d7a00298f0636dcb88d8bec90`:

| Check                                                  | Result                                          |
| ------------------------------------------------------ | ----------------------------------------------- |
| Default VMM unit suite                                 | 891 passed; none ignored                        |
| Windows VMM unit suite                                 | 1,092 passed; none ignored                      |
| Privileged jailer mount and retained-namespace parents | Both passed, including their child protocols    |
| Variable helper                                        | 67 passed; none ignored                         |
| Console helper                                         | 31 passed; none ignored                         |
| Strict workspace Clippy                                | Default and all features, GNU and musl          |
| Strict ARM64 cross-Clippy                              | Default and all features, GNU; compilation only |
| Standalone tool Clippy and Rust/Markdown style gates   | Passed                                          |

The VMM suites execute the real swtpm restore, ACPICA evaluation and maximum
optional-buffer snapshot-reader tests. The console backlog regression passes;
replacing only its connector with the preceding blocking call reproduces the
five-second failure. Normal discovery ignores two privileged jailer parents and
their two child entrypoints. Both parents were then run explicitly with the
required privileges; they invoke the children. These results do not imply that
every ignored documentation example was executed.

Current platform/API, Linux snapshot, Windows installation, egress and
performance qualification remains open. The historical results below use format
31 and their matching binaries. They do not qualify format 32. Do not load an
older saved state with the candidate.

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

XP and Windows 7 use the documented external test-signed Viosock packages.
Windows 7 retains three ACPI Code 28 devices. Real legacy generation-counter
driver binding, polling and restore qualification remain open. VMClock
recognition on modern Windows establishes enumeration only. These installed
references do not establish fresh XP, Windows 7, Windows 10 or IoT installation,
or every graphical input path. Process/snapshot cache checks do not establish
host power-loss durability. XP HTTP does not establish TLS support.

The fresh Enterprise run uses variable helper SHA-256
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

Standard Windows 11 Pro, Server Core and Desktop, the remaining fresh
installations and the current format-32 guest matrix remain unqualified. The
earlier memory comparison against fork baseline `658ff27e` exceeded the normal
performance gates. The final candidate must pass unchanged gates against the
registered consumer base `c6655e5`, with default and Windows builds measured
separately.

The native AMD64 host runs Linux 7.0.0-60009-generic. Its results do not qualify
the supported host-kernel matrix. [ARM64 results](windows-arm64.md) are
historical emulated tests; current cross-builds do not qualify native ARM64
Windows installation, input, security or egress. Cross-host migration and
firmware-stage snapshots also remain unvalidated. Tokoloshe's Windows job
runtime is outside this guest-platform and egress scope.
