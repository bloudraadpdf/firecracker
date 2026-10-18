# Windows guest validation

Windows support is experimental and incomplete. A pass below applies only to its
tested guest, installed drivers, firmware, VMM and route. Use the
[AMD64 guide](windows.md), [console guide](windows-console.md) and
[secure platform guide](windows-secure-boot.md) to prepare a test.

## Current integration candidate

The current combined AMD64 runtime is built from `95aaccfb`; subsequent changes
only affect documentation. Its Windows-feature binary is `6f63cd8d` and its
default counterpart is `1c68a888`, using snapshot format 34. Normal Linux and
host checks below qualify these binaries. The Windows edition, installation,
input and egress matrix still requires separate guest qualification.

The latest Windows guest-qualified VMM remains `93434437`. Its source is the
recorded `9eb13bb7` base plus the Rust USB tablet zero-coordinate correction; it
is not an unchanged build of that Git commit. The preceding `f90f364e` and
combined `8b8c6932` binaries also use format 34. Neither format compatibility
nor a successful build carries a runtime result forward to changed code.

Reuse a recorded result when the inputs relevant to that check are unchanged.
Retain the guest configuration, driver and firmware hashes, VMM capability and
source identity with each result. A documentation change does not require a new
VMM build or guest run. Repeat a check for changed behaviour or an unresolved
failure, and retain the earlier failed record.

| Area                                        | Latest qualified result                                                                                                                                                                                      | Remaining gate                                                                                                                                    |
| ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| XP SP3 x86 display and direct console input | VMM `93434437`, unsigned XPDM miniport 0.1.2.0: all 16 advertised 32-bit modes pass full-frame RGB comparison, five pointer targets per mode and saved printable ASCII with Caps Lock on.                    | Normal Windows Keep/Revert/timeout UI, browser/VNC input, other layouts and guests.                                                               |
| XP mode and input persistence               | The helper restores the original 640x480x32, 60 Hz mode, exits zero and the guest shuts down through normal S5.                                                                                              | Nonzero-mode cold boot and full/differential restore of display and held input state.                                                             |
| XP TAP and CONNECT                          | Earlier installed-reference results exist.                                                                                                                                                                   | Both routes on the latest candidate, with current drivers and generation/storage verification.                                                    |
| Windows 7 TAP                               | Preceding VMM `f90f364e`: installed Enterprise SP1 x64 reference passes cold/full/differential egress, storage and generation checks.                                                                        | Qualify the current candidate `6f63cd8d`; the installed-reference result does not qualify a fresh installation.                                   |
| Windows 7 CONNECT                           | Normal VMM `93434437`: installed-reference cold/full/differential transfers, denials, storage, public verifier and final S5 pass. The outer caller fails firewall-rule preservation.                         | Explain the two removed Docker rules and the earlier normal `f90f364e` failure; qualify the combined candidate separately.                        |
| Modern installation and security            | Earlier binary/format-specific installation, Secure Boot, TPM and variable-service results exist.                                                                                                            | Current Windows 10/11/Server installation, security, persistence and route matrix; Windows 11 Pro and remaining editions remain unqualified.      |
| Host regression checks                      | Current default/Windows unit, documentation, example and benchmark checks, standalone tools, platform module and Linux PMEM/bzImage checks pass; default API, jailer, seccomp and snapshot checks also pass. | Results cover the tested Ryzen 5950X/Linux 7.0 host and Linux 6.1.188 guest. CPU-template skips and the supported-host matrix remain unqualified. |
| ARM64 and performance                       | Final default/Windows boot and restored-memory measurements complete. The maintained analyser flags increased executable text and restored memory overhead; no boot metric is flagged.                       | Native ARM64/KVM qualification remains open. Preserve the measured footprint costs below; the performance comparison is not an overall pass.      |

The XP pass uses one installed Professional SP3 x86 reference, SeaBIOS,
`vmclock: false`, one vCPU and 2 GiB RAM. Its normal unsigned-driver warning was
accepted; no signing-policy or registry repair bypass was used. All 16 saved
files contain exactly bytes 32 through 126, without a BOM or newline. The
stopped probe log confirms all mode tests/applications, 80 accepted pointer
coordinates including each top-left `(0,0)`, successful writes/flushes/closes,
restoration and final status zero. These checks complement the RGB frames; input
acknowledgements or screenshots alone do not prove saved data.

A separate normal Windows 7 CONNECT run on `93434437` passes all three phases
with a guest probe update that adds failure diagnostics. Six sealed proxy
entries confirm three allowed 2 MiB transfers in each direction and three
zero-payload denials. The stopped transcript, final disk bytes,
full/differential restore logs, both format-34 headers, public verifier and
normal S5 agree. This qualifies fresh connections on the installed reference,
with generation checks disabled. The overall caller remains failed: two Docker
PREROUTING DROP rules present before the run are absent afterwards. The
responsible actor and cause are not established; owned VM, proxy, mount and loop
closure passes.

The earlier normal `f90f364e` failure remains unresolved. Its cold transfer
passes, but after full restore the proxy accounts 162,816 bytes in each
direction before the guest reports a receive failure and requests S5. The older
probe does not record enough detail to classify EOF or timeout. A separate
Debug-logging run passes; neither that result nor the later normal pass
establishes a cause or a repair. The new vsock correction has no guest-cause
attribution from these runs.

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

Performance acceptance remains open: earlier memory comparisons exceeded the
unchanged thresholds and do not measure the current default Linux profile.
Measure default and Windows builds separately against the registered consumer
base `c6655e5`, using the existing gates and supported host matrix. Egress
passes and process/snapshot consistency do not establish power-loss durability,
a latency bound or production readiness. Tokoloshe's Windows job runtime is
outside this guest-platform and egress scope.
