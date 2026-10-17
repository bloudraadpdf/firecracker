# Windows guests (ARM64, experimental)

The guest results below are historical emulated tests with the recorded
format-12, format-19 and format-20 binaries. They do not qualify the current
corrective candidate. Native ARM64 installation, console input, Secure Boot, TPM
and egress checks remain required on a suitable Linux/KVM host.

The ARM64 implementation loads patched EDK II CloudHv firmware and supplies ACPI
tables for GICv3/ITS, architectural timers, PCI and the serial port. Firmware
boots from modern virtio PCI storage. Earlier tests installed Windows and booted
its desktop under QEMU. Historical Firecracker binaries reached Windows service
startup with release firmware and default seccomp. A SYSTEM startup probe
verified 2 MiB payloads through the real Tokoloshe CONNECT proxy without a guest
NIC, including after full, differential and subsequent full snapshot restore.
Adjacent-port requests receive HTTP 403. An in-flight connection interrupted by
restore fails; fresh connections succeed. This validates the transport, not a
complete Windows port of the Tokoloshe guest agent. The emulated cold boot takes
about 30 minutes to reach the probe and reports several service-start timeouts.

## Host requirements

Use an ARM64 Linux host with KVM, GICv3/ITS and PMUv3 support. The Windows
loader reads `PMCR_EL0`. On the tested Apple nested-KVM appliance, that
instruction faults because KVM exposes no PMU. A working Linux guest or EFI
shell on that host does not establish Windows compatibility.

The historical PMU-capable validation environment was an isolated Tokoloshe
Linux appliance under QEMU TCG with virtualisation and PMUv3 exposed.
Firecracker ran inside its Linux KVM. This emulated environment does not
establish hardware performance or a QEMU dependency for native deployment. The
original Tokoloshe appliance was not modified.

## Prepare an ARM64 disk

Obtain the English-US ARM64 edition of
[Windows 11 IoT Enterprise LTSC 2024 Evaluation](https://www.microsoft.com/en-us/evalcenter/download-windows-11-iot-enterprise-ltsc-eval)
and the virtio-win driver ISO. Image 1 of the tested Windows ISO is ARM64. Its
SHA-256 is:

```text
3dcdba9c9c0aa0430d4332b60c9afcb3cd613d648a49cbba2d4ef7b5978f32e8
```

The current preparer has a separate `win11-iot-arm64` staging profile for the
Toets OEM ARM64 ISO, whose hash differs from the historical Evaluation ISO
above. It creates an extracted payload tree and stages ARM64 drivers; it does
not service `boot.wim` or build a bootable installer. See the
[ARM64 staging procedure](../tools/windows/README.md#arm64-installation).

The earlier disk was prepared under QEMU/AAVMF. That result is historical; the
QEMU preparation generator has been removed. There is no qualified direct
Firecracker ARM64 installation procedure. Use an authorised, already-installed
raw disk for firmware-path investigation and preserve its original copy. ARM64
uses its separate MMIO display and combined virtio input profile; see
[console configuration](windows-console.md#arm64). The AMD64 option ROM and
ATAPI preparation commands do not apply.

## Firecracker boot configuration

Build the VMM and matching snapshot editor on an ARM64 Linux build host with the
Windows capability enabled:

```sh
cargo build --release --locked --target aarch64-unknown-linux-musl \
  -p firecracker -p jailer -p snapshot-editor \
  --features firecracker/windows,snapshot-editor/windows
```

The binaries are in `build/cargo_target/aarch64-unknown-linux-musl/release/`. A
cross-build does not qualify guest execution; retain the native host
requirements above.

Build the
[ARM64 CloudHv profile](../resources/firmware/cloudhv-secureboot/arm64/README.md)
that matches the configured devices: `ArmVirtCloudHvConsole.dsc` for graphical
input, or `ArmVirtCloudHvSecure.dsc` with TPM and protected variables. The
original `ArmVirtCloudHv.dsc` is headless; its earlier build is described in the
[firmware index](../resources/firmware/README.md#arm64-firmware). Each produces
a raw `CLOUDHV_EFI.fd` in its own output directory. Use that image as the boot
source. Do not supply an initrd. Unlike AMD64 PVH boot, this path loads raw
firmware at `0x80200000`, with the FDT at `0x80000000` and the ACPI root at
`0x801f0000`.

Declare the CloudHv ABI with `firmware.type: "cloud-hv"` and a stable
`firmware.system_uuid`. Firecracker reserves one 4 KiB page at `0x801e0000` and
publishes the SMBIOS system UUID there before constructing ACPI tables. Use the
same UUID for the guest throughout its lifetime. Snapshot restore retains the
saved identity and RAM. Without this declaration, Firecracker does not reserve
or write the SMBIOS page. SeaBIOS is supported only on AMD64. This identity path
does not qualify ARM Secure Boot or a Windows installation.

The following configuration limits the 52-bit validation host to 48 bits:

```json
{
  "boot-source": {
    "kernel_image_path": "CLOUDHV_EFI.fd",
    "boot_args": "",
    "firmware": {
      "type": "cloud-hv",
      "system_uuid": "11223344-5566-7788-99aa-bbccddeeff01"
    }
  },
  "machine-config": {"vcpu_count": 2, "mem_size_mib": 4096, "track_dirty_pages": true, "vmclock": false},
  "cpu-config": {
    "vcpu_features": [{"index": 0, "bitmap": "0b1xxx"}],
    "reg_modifiers": [{"addr": "0x603000000013c038", "bitmap": "0b0101"}]
  },
  "drives": [{
    "drive_id": "windows",
    "path_on_host": "windows.raw",
    "is_root_device": false,
    "is_read_only": false
  }],
  "vsock": {"guest_cid": 3, "uds_path": "vsock.sock"}
}
```

Start with
`firecracker --enable-pci --api-sock windows.sock --config-file windows.json`.
CPU-template bit 3 enables `KVM_ARM_VCPU_PMU_V3`. Firecracker assigns PPI 23 and
initialises the PMU after creating the interrupt controller. Restore recreates
the PMU from the saved vCPU feature flags. VMClock is explicitly disabled in
this example; its default remains enabled. Restore retains the saved clock
presence, independently of VM Generation ID. The AMD64 `hyperv` machine field
does not apply to ARM64.

The register modifier sets `ID_AA64MMFR0_EL1.PARange` to 48 bits. On the 52-bit
validation host, firmware creates five-level translation tables. The tested
Windows loader then selects four levels while retaining that table address and
faults. Exposing 48 bits makes firmware use four levels and allows Windows
kernel execution. On hosts already exposing 48 bits or less, omit this modifier;
it must not increase the host's address width.

## Optional security devices

The ARM64 security path uses the same private TPM and variable-helper sockets as
AMD64. Its addresses differ because AMD64's security addresses can overlap ARM
RAM. The ARM pages are outside dynamic MMIO and PCI apertures and below the GIC
regions. They leave ordinary Linux allocations unchanged. Restore rejects saved
device mappings that overlap a selected security page:

| Device                                    | ARM64 address | Size  |
| ----------------------------------------- | ------------- | ----- |
| TPM CRB locality zero                     | `0x10000000`  | 4 KiB |
| Protected variables and physical presence | `0x10001000`  | 4 KiB |

ACPI describes the TPM with start method 7 and control area `0x10000040`. The
variable page also has the platform-bus device-tree binding consumed by the
external firmware. These firmware ABI names do not add a QEMU runtime
dependency. ARM retains its hardware-reduced FADT and PSCI shutdown path.

A secure guest requires the secure ARM firmware profile and dedicated helpers;
the ordinary ARM image does not establish Secure Boot or TPM initialisation.
Preserve the matched disk, TPM, variable store and memory checkpoint as
described in the [Secure Boot guide](windows-secure-boot.md). A snapshot with a
security device requires its corresponding destination helper and explicit
variable-store generation. Failed restoration must discard the isolated staging
copies.

These source changes require separate native ARM64 firmware, Windows runtime,
installation and restore qualification. The earlier emulated egress results
below do not qualify this security path.

## Egress and validation

Signed ARM64 NetKVM drivers are required for TAP networking. The vsock CONNECT
path needs the ARM64 viosock driver and Winsock provider. Match every driver and
boot image to the selected Windows edition and architecture. The following
results are historical; current runtime qualification remains pending.

### Historical emulated results

The ARM64 virtio-win ISO contains `viosock/w11/ARM64`. The preparation test
staged that signed driver and a native ARM64 Winsock probe. The initial SYSTEM
task incorrectly required an interactive token. Its replacement uses
`ServiceAccount`; its transcript confirms SYSTEM execution before login. The
probe uses host CID 2, port 4750 and HTTP CONNECT, as described in the
[AMD64 transport guide](windows.md#vsock). The actual Tokoloshe ARM64 Linux
sidecar passes allowed 2 MiB echo requests and denies the adjacent port. Guest
byte comparisons and sealed sidecar accounting confirm these results before and
after full, differential and subsequent full snapshot restore. No guest network
device is configured. Applications must reconnect after restore; a connection
captured during a transfer fails, then the probe establishes a fresh connection
successfully.

Conventional virtio-net/TAP egress also passes in Firecracker with default
seccomp and no vsock device. The SYSTEM probe configures NetKVM at
`172.30.253.2/30`, with TAP gateway `172.30.253.1` and DNS `10.0.2.3`. The Linux
appliance enables forwarding and masquerades that subnet through `enp0s3`. The
guest resolves `www.microsoft.com` and receives HTTPS 200 with certificate
validation enabled and proxies disabled. It then checks every byte of a
2,097,152-byte TCP echo exchange with the LAN host. Independent host accounting
confirms the same count. The transcript completes at 17:39:59 UTC on 2026-10-13;
metrics contain no seccomp faults, VMM panics or vCPU failures. Test processes,
disposable disk and network resources are removed after evidence collection.
Logs and the PowerShell probe are under `build/windows/arm64-host` in the
validation worktree.

Completed checks include firmware PCI disk discovery, musl/seccomp firmware
boot, PMU initialisation and register restore, firmware-loader rejection tests,
and Linux direct boot with full and differential snapshot restore. ARM firmware
boot rejects an initrd before copying it into guest memory. Snapshots contain
memory and device state, not the disk. Keep a disk copy from the same paused
state and restore that copy with the memory snapshot. Only one VM may write a
disk at a time.

The complete evidence and remaining work are in the
[validation record](windows-validation.md).

The historical format-20 source rejected earlier snapshots. The current snapshot
contract is documented in [snapshot versioning](snapshotting/versioning.md).
ARM64 accepts only modern PCI. The format-19 binary
`068748352323dd47b17949862c9497cade8623a0bf4fc230db18dd0077530b7c` passes cold
boot, full restore and differential restore with a SYSTEM probe that flushes and
compares 2 MiB of random disk data, verifies an allowed Tokoloshe CONNECT echo
and checks a denied destination. Sealed accounting records exactly three allowed
2 MiB transfers in each direction and three denied requests with no payload.
After the final probe, Windows requests shutdown and Firecracker exits zero.
Read-only disk inspection verifies the final file against its recorded hash and
confirms that hibernation is disabled. Evidence is in
`build/windows/arm19/run/`; see the [format-19 result](windows-validation.md).

The earlier TAP and subsequent-full results above used format 12. The later
format-19 TAP run passes cold, full and differential storage, DNS, validated
HTTPS and byte-checked 2 MiB TCP workloads with the PCI reset and eventfd
seccomp fixes. Offline disk verification passes, but the controller stops the VM
after its five-minute shutdown limit; a clean shutdown is not established.
Standard online activation succeeds and the guest reports licensed state before
these workloads. Earlier retries ended in evaluation-licence shutdowns. The
format-20 TAP rerun passes all three workloads and clean shutdown. Firecracker
receives the guest shutdown event and exits zero; offline verification confirms
three guest passes, three complete TCP echoes and the final storage hash.
Evidence is in `build/windows/arm20-tap/run/`. The final format-20 CONNECT run
also passes cold, full and differential storage and proxy checks, followed by
guest shutdown and Firecracker exit zero. Offline verification confirms SYSTEM
identity, three guest passes, six sealed accounting entries and the final 2 MiB
storage hash. Each allowed tunnel transfers 2 MiB in each direction; each denied
request transfers no payload. Evidence is in `build/windows/arm-connect20/run/`.
Both final runs use binary SHA-256
`f7f80394d1a8ebe3d6319107862b63ad63780b8e4b75793f601c49e2ecc879a4`. These tests
do not establish native ARM performance. The same format-20 binary passes Linux
direct boot and full/differential restore with a counter probe. Each restore
continues the counter without repeating its boot marker; evidence is in
`build/windows/arm-linux20/`. See the validation record for the tested scope.
