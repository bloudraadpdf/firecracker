# CloudHv firmware for Firecracker

For BIOS disks, use the separate [SeaBIOS PVH handoff](seabios/README.md).

The stock x86-64 CloudHv firmware derives its 64-bit PCI aperture from RAM
and CPUID, and uses a 32-bit aperture ending at `0xfc000000`. Firecracker
allocates PCI BARs in `[0x4000000000, 0x8000000000)` and places ECAM at
`0xeec00000`. The stock firmware can therefore assign BAR addresses that
Firecracker cannot map.

`cloudhv-firecracker.patch` configures the firmware apertures to match
`src/vmm/src/arch/x86_64/layout.rs`. Its 32-bit aperture starts after the
reserved boot-device page. The patch changes only the CloudHv platform
branch of EDK II. Use the resulting firmware only with this Firecracker
memory map.

Firecracker assigns PCI BARs before boot. The patch also marks root-bridge
resources as assigned and selects lightweight PCI enumeration. This prevents
firmware from swapping the addresses of live disk and network mappings.

The patch reserves physical page zero in the UEFI memory map. The pinned
EDK II allocator rejects allocations at address zero; advertising it as free
causes Windows 7's initial heap allocation to fail with `0xc0000225`.

The patch also installs the FACS supplied through FADT. EDK II clears the
FADT firmware-control pointer if the structure is not installed, which
prevents Windows from loading conventional ACPI tables.

The patch applies to Cloud Hypervisor EDK II tag `ch-97eeb7b09`. The GCC
release build boots Windows with virtio PCI disk and network devices.
The tested firmware SHA-256 is
`8220806a388141650ca1c5280863a9085f2d969de9af839519d231cbd4c7286b`.
This build passes Windows Server 2025 boot, SAC authentication, disk
write/read and full snapshot restore with conventional ACPI. The
preceding page-zero build
`44e90992add578d5fe3ddf395c797289dde025c8e66e2f2f51f4f833412561cf`
passes the Windows 7 loader's memory initialisation and Windows Server
boot, console authentication, file write/read and S5 shutdown.
It does not establish a complete Windows 7 boot. Earlier network and snapshot
tests used firmware digest
`fc8cbaf16d77721623691670c9b152d40d446a01ae8217fa99519d2798fee91d`.

On a Linux build host with EDK II's build dependencies installed, run from
the Firecracker worktree:

```sh
git clone --depth 1 --branch ch-97eeb7b09 \
  https://github.com/cloud-hypervisor/edk2.git build/windows/edk2
git -C build/windows/edk2 submodule update --init --recursive
git -C build/windows/edk2 apply --ignore-whitespace \
  ../../../resources/firmware/cloudhv-firecracker.patch
cd build/windows/edk2
. edksetup.sh
make -C BaseTools -j2
build -p OvmfPkg/CloudHv/CloudHvX64.dsc -a X64 -t GCC -b RELEASE -n 2
```

The output is `Build/CloudHvX64/RELEASE_GCC/FV/CLOUDHV.fd`. Record its
SHA-256 digest with the source revision and patch before a boot test.
The PVH ELF loader, ACPI handoff and I/O ports remain those of CloudHv.

EDK II source and this source patch use `BSD-2-Clause-Patent`, as declared
in the modified upstream files. See the [upstream licence](https://github.com/cloud-hypervisor/edk2/blob/ch-97eeb7b09/License.txt).

## ARM64 firmware

`cloudhv-firecracker-arm64.patch` applies to the same EDK II tag. It uses
Firecracker's RAM, ECAM and PCI apertures, its MMIO 16550 UART, and a fixed
FDT/RSDP handoff. Firmware reserves the platform data and firmware image
in its UEFI memory map. PCI enumeration honours `linux,pci-probe-only`.

Apply the ARM64 patch to the pinned checkout, then build:

```sh
git apply --ignore-whitespace ../../../resources/firmware/cloudhv-firecracker-arm64.patch
build -p ArmVirtPkg/ArmVirtCloudHv.dsc -a AARCH64 -t GCC -b RELEASE -n 2
```

For a cross-build, set `GCC_AARCH64_PREFIX` to the AArch64 GCC tool prefix.
The raw firmware is `Build/ArmVirtCloudHv-AARCH64/RELEASE_GCC/FV/CLOUDHV_EFI.fd`.
Pass it as Firecracker's kernel image and enable PCI. The release firmware
boots Windows ARM64 under Firecracker with PMUv3 and default seccomp.
The tested firmware SHA-256 is
`93692b58448e1754c5f6cf89ca4e361c54d90985e1461130b5d76d7ea491a74a`.
See the [ARM64 guest guide](../../docs/windows-arm64.md) for host requirements
and the verified vsock and snapshot behaviour.
