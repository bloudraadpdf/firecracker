# Windows egress probes

These standalone probes check fresh connections, DNS, TLS and byte equality. Use
disposable guest state. Passing a build does not establish guest operation or
preservation of live TCP sessions across snapshots.

## Build

Follow the [Rust build prerequisites](../README.md#build-prerequisites). Run
these commands from the repository root. Keep build products in the ignored
build directory. Build the Linux echo/report server with the selected Rust
toolchain:

```sh
mkdir -p build/windows/egress
rustc --edition 2024 -D warnings tools/windows/egress/server.rs \
  -o build/windows/egress/tap-server
```

Select the Windows 7 x64 profile and prepare `windows_rustflags` using the
[shared probe link procedure](../README.md#windows-probe-linking), then build
the HTTPS probe in that Bash session:

```sh
rustc --edition 2024 "${windows_rustflags[@]}" \
  -D warnings tools/windows/egress/https.rs \
  -o build/windows/egress/https.exe
```

The probe uses the pinned compatibility and SDK libraries, requires HTTP 200 and
explicitly selects TLS 1.2. It keeps certificate validation enabled. A fresh
Windows 7 SP1 image needs the [documented trust root](../../../docs/windows.md).

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

The echo/report server listens on TCP 40569 and 40570. Start it with the
reachable Linux IPv4 address and a bounded timeout in seconds. Keep one guest
probe running and inspect all guest/host pass markers.

`network.sh setup STATE` prepares a dedicated namespace, TAP, routes and NAT.
Run it as root with a new absolute private state directory,
`WINDOWS_NETWORK_UPLINK`, `WINDOWS_NETWORK_UID` and an unused
`WINDOWS_NETWORK_PREFIX`. Host IPv4 forwarding must already be enabled. It
refuses existing network objects. Run `network.sh cleanup STATE` after all
namespace processes stop; do not change an active restore run's network
identity. The script needs Bash, jq, iproute2, iptables and sysctl.

## XP HTTP and storage probe

XP uses `xp.rs` for HTTP, storage and TCP; this HTTPS executable does not
establish XP TLS support. Both entry points share `http.rs`; keep the files
together. Build the XP executable after selecting the XP x86 profile and
regenerating `windows_rustflags` through the same procedure. Put the reachable
lab host IPv4 address in `HOST_IPV4` before this block:

```sh
: "${HOST_IPV4:?Set the reachable Linux host IPv4 address}"
WINDOWS_EGRESS_HOST="$HOST_IPV4" rustc --edition 2024 \
  "${windows_rustflags[@]}" -D warnings \
  tools/windows/egress/xp.rs -o build/windows/egress/restore-probe.exe
rustc --edition 2024 -D warnings tools/windows/egress/verify-xp.rs \
  -o build/windows/egress/verify-tap-xp
```

Copy `restore-probe.exe` to `C:\` and `xp-matrix.cmd` to the common Startup
folder with CRLF line endings. Install the XP NetKVM driver and configure its
static address, gateway and DNS before the matrix; XP has no PowerShell
dependency. It reports progress through COM1. Use `WINDOWS_GUEST_TAG=XP` with
the same controller and set `WINDOWS_GUEST_ADDRESS` in the verifier if its
address differs from `172.30.254.2`. Set `WINDOWS_XP_VERIFIER` to the compiled
host verifier. The XP verifier checks every byte against the seed in the guest
transcript, including the seed stored in the first four bytes.
