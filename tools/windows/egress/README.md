# Windows egress regression

Use a disposable, closed guest disk. These tests check fresh connections after
cold boot, full restore and differential restore. They do not preserve live TCP
connections. Keep one VM writer and one snapshot sequence per disk. The Linux
controllers require Bash, jq, curl, iproute2 and sudo. Network setup also uses
iptables and sysctl; offline verification needs util-linux and ntfs-3g. Both
controllers check every writable config drive for open host file descriptors
before starting. Keep `assert-disk-closed.sh` and `secure-state.sh` beside the
TAP controller, or set `WINDOWS_DISK_GUARD` and `WINDOWS_SECURE_HELPER` to their
absolute paths when copying controllers separately. The secure helper is sourced
but starts no process unless `WINDOWS_SECURE_SOURCE` is set.

## Build

Build the Linux echo/report server with the selected Rust toolchain:

```sh
rustc --edition 2024 -D warnings server.rs -o tap-server
```

Build `https.rs` for `x86_64-pc-windows-msvc`, using `rust-lld` from the same
toolchain as `rustc`:

```sh
rustc --edition 2024 --target x86_64-pc-windows-msvc \
  -C panic=abort -C opt-level=2 -C lto=fat -C linker=rust-lld \
  -C link-arg=/entry:mainCRTStartup -C link-arg=/subsystem:console,6.01 \
  -C link-arg=/nodefaultlib -D warnings https.rs -o https.exe
```

The probe uses raw Windows import libraries, requires HTTP 200 and explicitly
selects TLS 1.2. It keeps certificate validation enabled. A fresh Windows 7 SP1
image needs the [documented trust root](../../../docs/windows.md). XP uses
`xp.rs` for HTTP, storage and TCP; this HTTPS executable does not establish XP
TLS support. Both entry points share `http.rs`; keep the files together. Build
the XP import library and executable as follows, with the lab host IPv4 address
in `WINDOWS_EGRESS_HOST`:

```sh
rust-lld -flavor link /lib /machine:x86 \
  /def:../legacy-vsock/msvcrt.def /out:msvcrt.lib
WINDOWS_EGRESS_HOST="$HOST_IPV4" rustc --edition 2024 \
  --target i686-pc-windows-msvc -C panic=abort -C opt-level=2 -C lto=fat \
  -C linker=rust-lld -C link-arg=/entry:mainCRTStartup \
  -C link-arg=/subsystem:console,5.01 -C link-arg=/nodefaultlib \
  -C link-arg=msvcrt.lib -D warnings xp.rs -o restore-probe.exe
rustc --edition 2024 -D warnings verify-xp.rs -o verify-tap-xp
```

Copy `restore-probe.exe` to `C:\` and `xp-matrix.cmd` to the common Startup
folder with CRLF line endings. Install the XP NetKVM driver and configure its
static address, gateway and DNS before the matrix; XP has no PowerShell
dependency. It reports progress through COM1. Use `WINDOWS_GUEST_TAG=XP` with
the same controller and set `WINDOWS_GUEST_ADDRESS` in the verifier if its
address differs from `172.30.254.2`. Set `WINDOWS_XP_VERIFIER` to the compiled
host verifier. The XP verifier checks every byte against the seed in the guest
transcript, including the seed stored in the first four bytes.

## TAP

Install the matching signed NetKVM driver. On 64-bit Windows use the native
`pnputil.exe`, not a 32-bit device installer. Windows 7 uses
`pnputil.exe -i -a C:\Drivers\NetKVM\netkvm.inf`; newer Windows uses
`pnputil.exe /add-driver C:\Drivers\NetKVM\netkvm.inf /install`. Require device
error code zero after installation.

Copy `tap.ps1` to `C:\Users\Public\fc-network.ps1` and `https.exe` to the same
directory. Run the script elevated at login, with `-HostAddress` set to the
reachable Linux host address, `-DnsAddress` to the lab DNS server, and `-Label`
to `WIN7`, `WIN10`, `WIN11` or `SERVER`. A graphical guest needs consent for its
elevated startup script. The script sets its NetKVM address, gateway and DNS.
Its defaults use MAC `06:00:ac:1e:fe:07`, guest `172.30.254.2/30` and gateway
`172.30.254.1`; override the parameters if needed.

Prepare a dedicated Linux network namespace with a TAP at that gateway, routing
and NAT to the uplink. The guest must reach the host on TCP 40569 (echo) and
40570 (reports), DNS and HTTPS. The host needs forwarding enabled. Keep this
namespace and its interface MAC addresses intact throughout the three stages;
recreating them invalidates restored guest ARP state. The controller uses an
existing namespace and does not change host routing or firewall rules. The
caller owns their setup and teardown.

Create a fresh test directory with `config.json`: absolute disk, firmware and
logger paths, `track_dirty_pages: true`, the TAP interface, no vsock, and an
optional console at `<test-directory>/console.sock`. Set these environment
variables to absolute paths before running `bash tap-check.sh`:

| Variable                  | Value                                           |
| ------------------------- | ----------------------------------------------- |
| `WINDOWS_CHECK_BASE`      | Test directory                                  |
| `WINDOWS_CHECK_FC`        | Immutable Firecracker binary                    |
| `WINDOWS_CHECK_REBASE`    | Matching `rebase-snap` binary                   |
| `WINDOWS_CHECK_SERVER`    | Compiled `tap-server`                           |
| `WINDOWS_CHECK_NAMESPACE` | Existing network namespace                      |
| `WINDOWS_CHECK_ADDRESS`   | Host IPv4 address used by the guest probe       |
| `WINDOWS_GUEST_TAG`       | Guest label                                     |
| `WINDOWS_CHECK_RUN`       | New result directory name; default `run1`       |
| `WINDOWS_CHECK_INPUT`     | Optional serial input FIFO; default `/dev/null` |

Keep the full API and console socket paths below Linux's 108-byte Unix socket
limit, including the result directory and `/cold.sock` suffix.

The controller copies itself into the result directory before running, so edits
for a later test cannot change a live shell's input. It bounds API, startup,
progress and shutdown waits, and stops owned processes on failure. Run it under
an outer `timeout --signal=TERM --kill-after=15s 900` as well. It retains logs
and snapshot files for inspection. Each guest stage verifies a 64 KiB
write-through file, DNS, validated HTTPS and byte equality for a 2 MiB
bidirectional TCP exchange. The host independently counts echo bytes.

After S5 and Firecracker exit zero, run `tap-verify.sh` as root with the same
base, run and tag, plus `WINDOWS_CHECK_DISK` and `WINDOWS_PARTITION_OFFSET`
(partition offset in bytes). `WINDOWS_TRANSCRIPT_ENCODING` defaults to UTF-8;
Windows 7 needs UTF-16LE. The verifier rejects an open disk, mounts it
read-only, checks all three guest/host results and the final file hash. Delete
full/differential RAM files after retaining compact evidence.

## Tokoloshe CONNECT

Reuse the
[legacy Viosock build and proxy instructions](../legacy-vsock/README.md). The
same x64 `legacy-connect.exe` and `win7-connect.ps1` can run on newer Windows
with the matching signed Viosock driver. The PowerShell script accepts `-Prefix`
for its output filenames and `-Tag` for pass markers; its defaults preserve the
Windows 7 test. Keep the guest NIC absent for this test.

Use `../legacy-vsock/check.sh` with the documented environment variables and an
optional console at `console.sock` in the test directory. It also copies its
controller before execution. Require three passes, six sealed accounting
entries, clean S5 and exit zero.

Run `connect-verify.sh` as root after shutdown. Set `WINDOWS_CONNECT_BASE`,
`WINDOWS_CONNECT_PREFIX`, `WINDOWS_CONNECT_TAG`, `WINDOWS_CONNECT_DISK`,
`WINDOWS_CONNECT_OFFSET`, `WINDOWS_CONNECT_IDENTITY`, and, for Windows 7,
`WINDOWS_CONNECT_ENCODING=UTF-16LE` and
`WINDOWS_CONNECT_IDENTITY_LABEL=Username`. Newer transcripts use `RunAs User`.
The verifier checks three allowed 2 MiB transfers in both directions, three
policy denials with zero payload, all guest passes and the final disk hash. It
accepts PowerShell's wrapped hexadecimal hash output. Preserve the guest
transcript and sealed accounting before deleting snapshot RAM and the disposable
disk. Record the VMM hash, snapshot version and firmware hash with the evidence;
directory names alone do not establish the tested version.

For XP CONNECT, use `WINDOWS_CONNECT_TAG=XPLEGACY`, prefix `xp-legacy`, UTF-8
encoding and `WINDOWS_CONNECT_XP_VERIFIER` pointing to the compiled
`../legacy-vsock/verify-xp.rs` host tool. It checks the legacy 2 MiB pattern;
the TAP verifier checks a different 64 KiB pattern.

Both controllers record phase timestamps and host-log watermarks in
`stages.log`. The host echo server records Unix millisecond receive times for
each completed transfer and report. A restored phase must produce new output.
After pausing and before starting the next VMM, the controller requires exactly
the completed number of reports (CONNECT denials or TAP transfers and reports).
A guest that advances before the snapshot fails the run; its earlier output
cannot satisfy a restored phase. Preserve this file with the API and guest logs.

For a Secure Boot/TPM guest, also copy `secure-state.sh` beside the disk guard.
Set `WINDOWS_SECURE_SOURCE` to a closed, matched helper directory containing
`store.json` and `tpm/`, and `WINDOWS_SECURE_VARS` to the variable-service
binary. `WINDOWS_SECURE_SWTPM` defaults to `swtpm`. The test disk, variable
store and TPM state must come from the same stopped VM. Keep the configured
secure firmware; do not replace it with the non-secure test firmware.

For secure guest acceptance, stage `secure-check.ps1` and the executable built
from `../tpm/probe.rs` as `tpm-runtime.exe` beside the guest probe script. Pass
`-SecureChecks` to `tap.ps1` or `win7-connect.ps1`. Each cold/full/diff
iteration then requires Secure Boot, a ready TPM through `Get-Tpm`, and an
actual TPM2 GetRandom command through Windows TBS before egress. Preserve all
three `PASS SECURE_STATE` markers in the guest transcript and set
`WINDOWS_SECURE_CHECKS=1` when running either offline verifier. These checks do
not clear the TPM or change encryption.

The controller freezes its script, secure helper and disk guard in the run
directory before execution. It copies helper state into each phase directory,
stops the VMM before its helpers, and creates the next copy only after both
helpers stop. It reads the copied store generation for the explicit restore
request; it never sends a host TPM startup command. Snapshot creation still uses
the normal API: format 25 embeds both checkpoints. On failure, the destination
phase state is discarded. A failed phase must not be resumed. Use a short
absolute test path: each generated helper directory must be at most 95 bytes so
its Unix socket paths fit. Run helpers and the CONNECT VMM as the same user.
Preserve the final disk and final phase stores as a matched set if the secure
guest will be used again.

The tracked `network.sh` creates the TAP, namespace, veth pair and exact NAT
rules used by this test. Host IPv4 forwarding must already be enabled. It
refuses existing names or routes for its two test subnets. Choose an unused
short prefix and set the corresponding TAP name in `config.json`:

```sh
state="$PWD/build/egress-network"
sudo env WINDOWS_NETWORK_UPLINK=eth0 WINDOWS_NETWORK_UID="$(id -u)" \
  WINDOWS_NETWORK_PREFIX=fce bash tools/windows/egress/network.sh setup "$state"
# config.json: host_dev_name = fcetap; WINDOWS_CHECK_NAMESPACE=fce
# Run one guest/controller; stop it before cleanup.
sudo bash tools/windows/egress/network.sh cleanup "$state"
```

The guest address is `172.30.254.2/30`, gateway `172.30.254.1`; the veth subnet
is `172.30.252.0/30`. Supply the actual host uplink and reachable DNS server.
Keep the network intact across both restores. The private state directory
records each created resource; cleanup removes only those resources and refuses
to remove a namespace that still contains a process. It neither changes host
forwarding nor flushes firewall tables. Put cleanup in the caller's exit trap
after its bounded controller has stopped.

## Read-only BitLocker verification

Keep automatic Windows device encryption enabled. Stage probes through a
read-only ISO and run them inside Windows. After clean S5, close the VMM and
both security helpers before copying or inspecting the matched state.

`bitlocker-verify.sh tap|connect` explicitly wraps the existing verifier with a
read-only BitLocker view. It supports a volume with an accessible clear-key
protector, including Windows device encryption with protection not yet enabled.
It takes no password, recovery key or raw-key argument. An inaccessible volume
fails; the wrapper never changes encryption or protectors. Do not enable verbose
libbde output, which can expose sensitive metadata.

Use upstream **libbde 20260916**, from
[the pinned release](https://github.com/libyal/libbde/releases/tag/20260916).
The source archive `libbde-alpha-20260916.tar.gz` has SHA-256
`22a27c81f7e8a513235c18d642889b690fefd34aa5ee687929103be62246da68`. The older
distro version 20240502 cannot read the used-space-only format found on the
standard Windows 11 acceptance disk. Build the unmodified archive under an
ignored tools directory with a C compiler, make and FUSE3 development headers:

```sh
curl --fail --location --output libbde-alpha-20260916.tar.gz \
  https://github.com/libyal/libbde/releases/download/20260916/libbde-alpha-20260916.tar.gz
printf '%s\n' '22a27c81f7e8a513235c18d642889b690fefd34aa5ee687929103be62246da68  libbde-alpha-20260916.tar.gz' | sha256sum -c -
tar xzf libbde-alpha-20260916.tar.gz
cd libbde-20260916
./configure --prefix="$PWD/../installed" --disable-python --disable-shared
make -j2
make install
```

Require `FUSE support: libfuse3` in the configure output. Acceptance extracted
Ubuntu `libfuse3-dev` and `libfuse3-4` packages under the tools directory
instead of installing system packages. With that arrangement, set
`PKG_CONFIG_PATH` to `root/usr/lib/x86_64-linux-gnu/pkgconfig`,
`PKG_CONFIG_SYSROOT_DIR` to the absolute `root`, and pass
`CPPFLAGS=-I/absolute/root/usr/include` to make. Retain the archive and its
licence notices when distributing the built tool.

Set the normal TAP or CONNECT verification environment plus:

| Variable                     | Value                                       |
| ---------------------------- | ------------------------------------------- |
| `WINDOWS_BITLOCKER_DISK`     | Absolute closed encrypted disk path         |
| `WINDOWS_BITLOCKER_OFFSET`   | OS partition byte offset from the GPT       |
| `WINDOWS_BITLOCKER_SIZE`     | OS partition byte length from the GPT       |
| `WINDOWS_BITLOCKER_STATE`    | New absolute directory for mount state/logs |
| `WINDOWS_BITLOCKER_BDEMOUNT` | Absolute pinned `bdemount` executable       |

Run within a private mount namespace and a timeout:

```sh
sudo --preserve-env=WINDOWS_CHECK_BASE,WINDOWS_CHECK_RUN,WINDOWS_GUEST_TAG,WINDOWS_BITLOCKER_DISK,WINDOWS_BITLOCKER_OFFSET,WINDOWS_BITLOCKER_SIZE,WINDOWS_BITLOCKER_STATE,WINDOWS_BITLOCKER_BDEMOUNT \
  timeout --signal=TERM --kill-after=10s 120 \
  unshare --mount --propagation private \
  bash tools/windows/egress/bitlocker-verify.sh tap
```

For CONNECT, preserve its documented verification environment instead of the TAP
variables and select `connect`. The wrapper checks the disk is closed, refuses
an existing state directory, creates a read-only partition loop and opens libbde
in unattended read-only mode. The existing verifier then checks its plaintext
partition at offset zero using another read-only loop and NTFS mount. Exit traps
remove the owned mounts and loops even when verification fails. Preserve compact
logs and file hashes; no decrypted disk copy is made.
