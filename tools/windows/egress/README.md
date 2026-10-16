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
