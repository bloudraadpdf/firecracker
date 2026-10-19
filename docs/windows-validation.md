# Windows guest validation

Windows support is experimental and incomplete. A pass below applies only to its
tested guest, installed drivers, firmware, VMM and route. Use the
[AMD64 guide](windows.md), [console guide](windows-console.md) and
[secure platform guide](windows-secure-boot.md) to prepare a test.

## Current integration candidate

The current AMD64 Windows-feature binary is `6f63cd8d`, built from runtime
source `95aaccfb` and using snapshot format 34. Its default counterpart is
`1c68a888`. Later documentation and secure-helper cleanup changes do not change
these binaries. The table records completed installed-reference checks on this
runtime separately from unchanged component results and remaining edition gates.

Reuse a recorded result when the inputs relevant to that check are unchanged.
Retain the guest configuration, driver and firmware hashes, VMM capability and
source identity with each result. A documentation change does not require a new
VMM build or guest run. Repeat a check for changed behaviour or an unresolved
failure, and retain the earlier failed record.

| Area                                       | Latest qualified result                                                                                                                                                                                                                                                                                                                                                                                                        | Remaining gate                                                                                                                           |
| ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------- |
| XP SP3 x86 display modes                   | Unchanged miniport 0.1.2.0 on recorded `93434437`: all 16 advertised 32-bit modes pass full-frame RGB, five pointer targets per mode and saved printable ASCII with Caps Lock.                                                                                                                                                                                                                                                 | Normal Keep/Revert/timeout UI, browser/VNC input, other layouts and guests.                                                              |
| XP current input and persistence           | Current `6f63cd8d`: nonzero 1024x768x32 cold mode, Windows Run/absolute pointer/raw Caps Lock input, held Shift across full/differential restore, explicit release and exact saved `aAaAAaAAa` pass.                                                                                                                                                                                                                           | Broader input/UI coverage beyond the tested paths.                                                                                       |
| XP TAP and CONNECT                         | Current `6f63cd8d`: both routes pass cold/full/differential transfers, stored data, public verification, both format-34 headers and final S5.                                                                                                                                                                                                                                                                                  | Legacy XP HTTP does not qualify modern TLS; fresh installation and other images remain separate.                                         |
| Windows 7 SP1 x64                          | Current `6f63cd8d`: Windows Run/absolute pointer/raw Caps Lock and saved `aAa`; both TAP and CONNECT pass cold/full/differential transfers, storage, public verification, format-34 headers and S5. TAP generation notifications also pass.                                                                                                                                                                                    | Held-key restore on this guest and fresh installation remain unqualified.                                                                |
| Windows 10 Enterprise LTSC 2021 x64        | Current `6f63cd8d`, 19044.1288: signed driver setup, RTC-policy reboot, normal SYSTEM task, saved `aAa` input and both transport routes pass.                                                                                                                                                                                                                                                                                  | Held-key restore is separately tested on XP; fresh installation remains separate.                                                        |
| Windows 11 IoT Enterprise LTSC x64         | Current `6f63cd8d`, 26100.1742: signed driver setup, RTC-policy reboot, normal SYSTEM task, saved `aAa` input and both transport routes pass.                                                                                                                                                                                                                                                                                  | Fresh installation, other editions and broader input paths remain separate.                                                              |
| Windows 11 Pro x64                         | Current `6f63cd8d`, 26300.9457/26H2: input, Secure Boot/TPM, authenticated UEFI cold deletion and TAP/CONNECT cold/full/differential checks pass.                                                                                                                                                                                                                                                                              | Enabled BitLocker protection, TPM-sealed recovery, attestation and fresh installation are not qualified.                                 |
| Windows 11 Enterprise Evaluation x64       | Current `6f63cd8d`, 26300.9457/26H2: signed drivers, policy reboot/input/task, Secure Boot/TPM and both TAP/CONNECT cold/full/differential checks pass, including secure checks, public verifiers, two format-34 headers per route and S5.                                                                                                                                                                                     | Fresh installation and other editions remain separate.                                                                                   |
| Windows Server 2025 Core x64               | Current `6f63cd8d`, ServerStandardEval 26100.32230: fresh installation and ISO-free boot, signed 2k25 drivers, RTC-policy cold boot, normal SAC input with exact stored `aAa` plus CRLF, 42 healthy present devices and normal SYSTEM task pass. TAP and CONNECT pass cold/full/differential checks, stored data, public verifiers, both format-34 headers per route and normal S5/VMM exit zero.                              | Graphical input and Secure Boot/TPM are not qualified by this ordinary firmware profile.                                                 |
| Windows Server 2025 Desktop Experience x64 | Current `6f63cd8d`, ServerStandardEval 26100.32230: fresh installation and ISO-free boot, signed 2k25 drivers, RTC-policy cold boot, 48 healthy present devices, Windows Run/absolute pointer/raw Caps Lock with exact stored `aAa`, and normal SYSTEM task/TAP staging pass. TAP and CONNECT pass cold/full/differential checks, stored data, public verifiers, both format-34 headers per route and normal S5/VMM exit zero. | Secure Boot/TPM are not qualified by this ordinary firmware profile.                                                                     |
| Host regression checks                     | Current default/Windows unit, documentation, example and benchmark checks, standalone tools, platform module and Linux PMEM/bzImage checks pass; default API, jailer, seccomp and snapshot checks also pass.                                                                                                                                                                                                                   | Results cover the tested Ryzen 5950X/Linux 7.0 host and Linux 6.1.188 guest; CPU-template skips and supported-host coverage remain open. |
| ARM64 and performance                      | Final default/Windows boot and restored-memory measurements complete; the maintained analyser flags executable-text and restored-memory costs.                                                                                                                                                                                                                                                                                 | Native ARM64/KVM qualification and the flagged footprint comparisons remain open.                                                        |

Both Secure Boot references identify Windows 11 x64 26300.9457/26H2; Enterprise
is the Evaluation edition. No activation or licence changes were made. Pro later
reached 100% encryption; Enterprise's recorded volume was 80% encrypted. Both
were unlocked with protection off. Clear-key read-only verification does not
qualify enabled BitLocker protection, TPM-sealed recovery or attestation.

Completed current transport rows require all three fresh phase proofs, stored
bytes, public offline verifier exit zero, both actual format-34 headers and
normal S5/VMM exit zero. CONNECT also requires three allowed bidirectional 2 MiB
transfers and three zero-payload denials with six sealed accounting entries.
These checks do not preserve live TCP connections or qualify Tokoloshe's Windows
job runtime. Original images and failed attempts remain preserved; disposable
working disks and snapshot RAM were retired only after compact proof fetch and
hash verification.

The retained XP all-mode component result uses one installed Professional SP3
x86 reference, SeaBIOS and the normally accepted unsigned XPDM warning, without
a signing-policy or registry repair bypass. Its 16 files contain exact printable
ASCII bytes 32 through 126; logs confirm 80 accepted pointer targets including
(0,0), mode application and restoration. The current persistence test separately
retains exact nine-byte saved input across cold/full/differential execution.

## Measured footprint

The maintained boot-time and memory-overhead tests ran once per binary set, with
their normal sampling loops, PCI and MMIO, one vCPU, 128 MiB RAM and Linux
6.1.188. The retained baseline is consumer revision `658ff27e`, not upstream
main. Both final capability builds passed all selected test bodies and teardown.
The normal analyser, with unchanged thresholds, failed both comparisons:

- Baseline to final default: executable text increases by about 250 KB (7.08%);
  restored memory overhead increases by about 200 KB for MMIO and 260 KB for
  PCI.
- Final default to Windows capability: executable text increases by about 291 KB
  (7.70%); restored memory overhead increases by about 267 KB for MMIO and 293
  KB for PCI.

No boot metric was flagged. These are measurements on a shared host, not
qualification on a dedicated performance host. The default build includes
transitional PCI/INTx and common API/snapshot decoding; optional live Windows
devices are feature-gated. Source review found no confirmed missing feature
gate, but does not attribute the measured bytes or prove that the implementation
cannot be smaller. The footprint failures remain recorded; no threshold change
or repeat run was used to obtain a pass.

## Run the maintained checks

Use the [normal test environment](../tests/README.md). The VMM unit suite needs
its documented swtpm/libtpms and ACPICA inputs; standalone tools have separate
workspaces. Run the existing build and platform entry points from the repository
root:

```sh
./tools/devtool test -- \
  integration_tests/build/test_unittests.py::test_tool_unittests \
  integration_tests/build/test_clippy.py::test_tool_clippy
./tools/devtool test -- \
  integration_tests/build/test_unittests.py::test_unittests \
  integration_tests/build/test_clippy.py::test_rust_clippy
./tools/devtool test -- integration_tests/functional/test_platform_devices.py
```

Omit `--binary-dir` for the platform module so its fixtures build both default
and Windows counterparts. These commands are regression entry points, not a
claim that the latest candidate has passed every test. See the
[test requirements](../tests/README.md#standalone-tools-and-platform-devices).

The affected USB and console tests require the Windows feature. On an AMD64
Linux host with the musl target and build prerequisites installed, reproduce the
focused checks and supported release profile with:

```sh
cargo test --locked --target x86_64-unknown-linux-musl --features windows \
  -p vmm --lib devices::usb:: -- --test-threads=1
cargo test --locked --target x86_64-unknown-linux-musl --features windows \
  -p vmm --lib device_manager::console_usb::tests:: -- --test-threads=1
cargo test --locked --target x86_64-unknown-linux-musl --features windows \
  -p vmm --lib device_manager::console_server:: -- --test-threads=1
cargo clippy --locked --target x86_64-unknown-linux-musl --features windows \
  -p vmm -p firecracker --all-targets -- -D warnings
cargo build --locked --target x86_64-unknown-linux-musl --features windows \
  -p firecracker --release
```

For the recorded source, the three test selectors run 18, 5 and 18 tests. Reject
zero selected tests. An all-features release build enables fuzzing and is not
the Windows release recipe. The
[XP display driver and mode probe](../tools/windows/display/README.md) include
their source, build inputs and a guest acceptance procedure. Their host unit
tests use the normal standalone-tool gate. The existing console screenshot
command and the probe's shared RGB model provide the full-frame comparison.
Retain all sixteen captures, stopped case files and the full probe log for a
fresh guest run. Host unit and console bridge tests do not replace that check.

### Guest egress and restore

Prepare authorised media with the [media tools](../tools/windows/README.md).
Complete driver installation and shut down before using an isolated test disk.
Follow the [egress procedure](../tools/windows/egress/README.md) for the exact
probe build, guest configuration, environment variables and offline-verifier
inputs. Legacy user-mode probes use the documented VC-LTL5/YY-Thunks link
profiles; a successful cross-link is not guest runtime qualification.

Choose one route. TAP uses the maintained network setup with a guest NIC and no
vsock; retain its network namespace through both restores. CONNECT uses the
[real Tokoloshe proxy](../tools/windows/legacy-vsock/README.md) and vsock with
no guest NIC. After setting the route's documented environment, run its
controller as the ordinary test user:

```sh
# TAP
timeout --signal=TERM --kill-after=120s 900 \
  bash tools/windows/egress/tap-check.sh
# CONNECT, in its separately prepared test directory/environment
timeout --signal=TERM --kill-after=120s 900 \
  bash tools/windows/legacy-vsock/check.sh
```

The TAP controller's public launcher uses sudo for its VMMs and editor. CONNECT
runs the VMM, proxy and sidecar as the same ordinary user. After the controller
has stopped and the disk is closed, run the matching offline verifier in a root
shell with the same documented verification environment:

```sh
# TAP
bash tools/windows/egress/tap-verify.sh
# CONNECT
bash tools/windows/egress/connect-verify.sh
```

Require cold, full and differential phase proofs, both actual snapshot version
headers, fresh guest output after each restore, byte-equal transfers, stored
data and final S5. CONNECT also requires six sealed accounting entries and
zero-payload policy denials. Enable the documented generation checks where
supported; XP uses polling, not notifications. For protected guests, use the
secure-state and read-only BitLocker procedures in the same egress guide. Retain
the final disk and its matching stopped variable/TPM stores together.

## Recorded format33 integration candidate

Format 33 and older results are historical. Their installed XP/Windows 7 TAP and
CONNECT passes, modern Windows installation and protected-runtime checks do not
qualify format 34 or the latest source. Earlier Secure Boot/TPM results do not
establish TPM-sealed recovery or attestation; recorded BitLocker protection was
off. The existing validation artifacts retain exact binary, firmware, media,
driver and source identities, failed runs and full logs. Rebuild snapshots for
the selected format; do not load an older saved state as an upgrade test.

### Earlier format-34 Windows 7 attempts

The earlier normal `f90f364e` receive failure and `93434437` caller's concurrent
Docker firewall-rule preservation failure remain retained as failed attempts.
Docker concurrency was observed, but the exact deletion actor and
receive-failure cause were not established. A Debug-logging pass did not repair
or qualify those failures. The separately completed current `6f63cd8d` runs are
the qualification entries above.

### Recorded Windows 7 reference

The historical format-33 Enterprise SP1 x64 reference retains signed NetKVM and
Microsoft generation-counter drivers after servicing and restart. Its TAP run
passes cold/full/differential transfer, storage and generation checks, including
both prearmed child completions. Wrong probe hashes and existing output paths
are rejected without changing the earlier data. A separate CONNECT clone passes
fresh connections in all three phases with the external test-signed Viosock
profile, six sealed accounting entries and final S5. These installed-reference
results do not qualify another policy, guest image or VMM.

### Recorded Windows XP reference

The historical format-33 Professional SP3 x86 reference passes cold queries with
Microsoft generation-counter 6.3.9600.17831 and running KMDF. Separate TAP and
CONNECT clones pass cold/full/differential transfers, polling with three
distinct generation IDs, final storage and S5. TAP uses normal NetKVM warning
consent; the separate default signature-policy check fails. Its original offline
collector also remains failed; a separate parser accepts the retained
trailing-space marker. CONNECT passes three bidirectional 2 MiB transfers and
three zero-payload denials with six sealed accounting entries. These results use
the retained original reader and host inputs; a rebuilt reader needs new
qualification.

The [ARM64 guide](windows-arm64.md) describes the separate firmware path and
historical emulated results. Native ARM64 installation, input, security and
network/restore qualification remain open. Cross-host migration and
firmware-stage snapshots are also unvalidated.

The completed default and Windows measurements retain the failed footprint
comparisons against the unchanged thresholds, as described above. They do not
qualify other supported hosts or erase the measured costs. Egress passes and
process/snapshot consistency do not establish power-loss durability, a latency
bound or production readiness. Tokoloshe's Windows job runtime is outside this
guest-platform and egress scope.
