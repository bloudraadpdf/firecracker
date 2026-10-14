# Preserve the PVH memory map

Apply `cloudhv-pvh-memory-map.patch` after the other Firecracker patches to
Cloud Hypervisor EDK II revision `97eeb7b0901da9f7b5d98a2e6a5e8694978298e9`.
It is required by the headless, graphical and Secure Boot CloudHv profiles.

The pinned firmware discarded non-RAM PVH entries and published one flat RAM
resource below 4 GiB. Windows could consequently allocate page tables inside
Firecracker's reserved 32–33 MiB platform region. Updating the VM clock's
disruption marker on snapshot restore then corrupted an OS page table.

The patch publishes each usable interval separately, rounds RAM inward and
reserved ranges outward to pages, and preserves the page-zero allocation
reservation. It validates bounded entry counts, arithmetic and overlapping
ranges before size discovery or HOB publication. Invalid maps stop firmware;
they cannot fall back to a flat CMOS-derived map. Non-CloudHv RAM discovery is
unchanged. EDK II's GCD converts reserved resource HOBs into reserved UEFI
memory descriptors; no competing system-memory HOB covers these ranges.

Existing snapshots retain their old guest memory map. Cold boot with corrected
firmware before creating replacement snapshots. Do not patch a running guest's
page tables or suppress VM clock updates.

## Reproduce the firmware check

Follow the relevant profile build instructions, including this patch. Copy
`cloudhv-console/tests/MemoryMapProbe.c` and `.inf` into `OvmfPkg/CloudHv/` in
the EDK II checkout. Create `OvmfPkg/CloudHv/CloudHvMemoryProbeX64.dsc`:

```ini
[Defines]
  DEFINE FIRECRACKER_CONSOLE = TRUE
!include OvmfPkg/CloudHv/CloudHvX64.dsc
[Components]
  OvmfPkg/CloudHv/MemoryMapProbe.inf
```

Build it with the same compiler and environment as the firmware:

```sh
build -p OvmfPkg/CloudHv/CloudHvMemoryProbeX64.dsc -a X64 -t GCC -b RELEASE -n 2
truncate -s 32M memory-probe-esp.raw
mkfs.vfat memory-probe-esp.raw
mmd -i memory-probe-esp.raw ::/EFI ::/EFI/BOOT
mcopy -i memory-probe-esp.raw \
  Build/CloudHvConsoleX64/RELEASE_GCC/X64/MemoryMapProbe.efi \
  ::/EFI/BOOT/BOOTX64.EFI
```

Use a new disposable ESP filename. Boot it as a read-only virtio disk with
4 GiB RAM and the explicit `cloud-hv` firmware profile with a fixed UUID.
The probe calls the real UEFI `GetMemoryMap`, checks all descriptors for
overlap and overflow, rejects allocatable memory in platform/legacy holes,
and requires complete reserved coverage of 32–33 MiB and the SMBIOS page.
Serial output must include both `PASS-reservations` and
`PASS-ram-below-above-4G`. For Secure Boot, sign this same application with a
disposable test key enrolled in a separate test variable store, following the
Secure Boot probe instructions. Never change installed guest keys or TPM state.

## Recorded validation

The old ordinary USB v4 image fails the same probe with
`FAIL-reserved-allocatable 88000 A0000`. Corrected ordinary and signed Secure
Boot runs pass, including explicit reserved descriptors for
`[0x2000000,0x2100000)` and `[0xf0000,0xf1000)` and conventional RAM above 4 GiB.
An independent reviewer inspected the complete descriptor lists.

Immutable ordinary image SHA-256:
`730de0fca7791d7623336b5c87d53e076c941e53f1118cb3b26d91445cbf7a16`.
Immutable Secure Boot image SHA-256:
`ff6fc396b5eb4daa3de992611814ab9d8084c6981e6bb07a3f44d58a2b948c43`.
These are firmware-map checks; OS cold/restore acceptance is recorded separately.

The headless `CloudHvX64.dsc` RELEASE build also passes with this patch. Run
exact-function malformed-map fixtures against the patched source:

```sh
bash resources/firmware/cloudhv-console/tests/test-memory-map.sh \
  build/windows/console/edk2/OvmfPkg/Library/PlatformInitLib/MemDetect.c \
  build/windows/memory-map-fixtures
```

The harness extracts the actual firmware functions and checks nine cases,
including wrapping ranges, overlapping raw/aligned reservations and fractional
legacy pages. It does not substitute a separately implemented map algorithm.
