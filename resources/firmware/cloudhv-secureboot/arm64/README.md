# ARM64 CloudHv firmware profiles

The secure external EDK II profile selects the existing host variable transport,
TPM CRB, Bochs MMIO graphics and virtio input drivers. Cross builds establish
source integration only. ARM Secure Boot, PPI interaction, Windows installation
and post-ExitBootServices runtime qualification remain pending. The ordinary
`ArmVirtCloudHv.dsc` profile retains its original providers.

Use EDK II revision `97eeb7b0901da9f7b5d98a2e6a5e8694978298e9`. Use the
[checkout, submodule and BaseTools setup](../../cloudhv-console/README.md#build)
with the EDK II prerequisites, including NASM and IASL on `PATH`. The following
ARM patch order replaces that guide's AMD64 patch/build selection. From the
Firecracker root, apply each patch once to the separate pinned checkout with
`git -C build/windows/console/edk2 apply --ignore-whitespace` and its absolute
patch path:

1. `resources/firmware/cloudhv-firecracker.patch`
1. `resources/firmware/cloudhv-console/cloudhv-console.patch`
1. `resources/firmware/cloudhv-console/virtio-input.patch`
1. `resources/firmware/cloudhv-secureboot/cloudhv-secureboot.patch`
1. `resources/firmware/cloudhv-secureboot/cloudhv-physical-presence.patch`
1. `resources/firmware/cloudhv-pvh-memory-map.patch`
1. `resources/firmware/cloudhv-firecracker-arm64.patch`
1. `resources/firmware/cloudhv-secureboot/cloudhv-secureboot-arm64.patch`

Do not reapply patches already present in a retained checkout. Preserve prior
firmware outputs before building another profile. Keep external notices and
licences with distributed firmware.

## Select a profile

| Profile                     | Graphics/input                         | TPM/host variables                      |
| --------------------------- | -------------------------------------- | --------------------------------------- |
| `ArmVirtCloudHv.dsc`        | Original headless profile              | Original providers                      |
| `ArmVirtCloudHvConsole.dsc` | Bochs MMIO GOP + combined virtio input | Original providers                      |
| `ArmVirtCloudHvSecure.dsc`  | Same graphics/input                    | Required TPM + protected host variables |

Console JSON does not select the firmware build. Select its matching profile
explicitly. From the Firecracker root, enter the EDK II checkout, initialise the
build environment and build with the installed AArch64 GNU cross compiler:

```sh
cd build/windows/console/edk2
. ./edksetup.sh
make -C BaseTools -j2
export GCC_AARCH64_PREFIX=/usr/bin/aarch64-linux-gnu-
build -p ArmVirtPkg/ArmVirtCloudHvSecure.dsc \
  -a AARCH64 -t GCC -b DEBUG -n 2
```

Use `-p ArmVirtPkg/ArmVirtCloudHvConsole.dsc` for graphics without TPM/protected
variables, or `-p ArmVirtPkg/ArmVirtCloudHv.dsc` for the original headless
profile. The executed DEBUG builds produced these raw images:

- Headless: `Build/ArmVirtCloudHv-AARCH64/DEBUG_GCC/FV/CLOUDHV_EFI.fd`.
- Graphical: `Build/ArmVirtCloudHvConsole-AARCH64/DEBUG_GCC/FV/CLOUDHV_EFI.fd`.
- Secure: `Build/ArmVirtCloudHvSecure-AARCH64/DEBUG_GCC/FV/CLOUDHV_EFI.fd`.

All three images are 3 MiB and fit the existing 4 MiB raw firmware loader.
Record the image, source, compiler and patch hashes before guest validation. Use
the explicit CloudHv UUID profile and matching ARM TPM/variable devices for the
secure profile; a build flag does not enrol a Secure Boot trust policy.

```text
0x10000000 TPM CRB -> PEI discovery -> Tcg2ConfigPei -> Tcg2Pei Startup
                                      |
                                  Tcg2Dxe / host ACPI TPM2
0x10001000 variable MMIO -> VirtMmCommunicationDxe -> host helper
PCI0380 BAR0/BAR2 -> QemuVideoDxe -> graphics console
virtio input PCI  -> VirtioInputDxe -> trusted console input
connected consoles -> physical presence -> hierarchy configuration -> boot
```

The secure profile selects one host-backed variable provider and requires it.
Its shared transport buffer is 2 MiB; the register page is 4 KiB. The page must
have uncached runtime attributes and survive virtual-address conversion. The TPM
is discovered at its fixed CRB address, without a false TIS device-tree binding.
Windows uses the supplied ACPI TPM2 table. No x86 chipset, legacy VGA aperture,
port I/O, option ROM or QEMU runtime is required.

The input driver binds text input and absolute pointer protocols independently
on the combined device. Its status queue reports real Num/Caps/Scroll LED state
after initialisation, reset, lock-key makes and `SetState`. Status descriptors
remain owned by the device until reclaimed; a full queue defers the latest state
to the existing poll timer. Modifier translation, including logo keys, and
partial-key `SetState` semantics remain intact. This is source support;
interactive firmware input still needs runtime tests.

Physical presence is processed only after console connection and before
platform-hierarchy configuration or any boot image. The existing operation
whitelist, confirmation and protected flags remain in force. TPM clear testing
requires a new disposable TPM, store and disk; never use an installed reference.

The security pages lie outside PCI apertures and device allocators. The ARM
layout patch must map peripheral MMIO from `0x00400000` to `0x80000000`; the old
mapping ended at the TPM address.

## EFI probe

After the production patches, apply the shared and ARM test patches once. From
the Firecracker root:

```sh
git -C build/windows/console/edk2 apply --ignore-whitespace \
  "$PWD/resources/firmware/cloudhv-secureboot/tests/firmware-tests.patch"
git -C build/windows/console/edk2 apply --ignore-whitespace \
  "$PWD/resources/firmware/cloudhv-secureboot/arm64/firmware-tests.patch"
cd build/windows/console/edk2
. ./edksetup.sh
export GCC_AARCH64_PREFIX=/usr/bin/aarch64-linux-gnu-
build -p ArmVirtPkg/Tests/CloudHvSecureProbeAArch64.dsc \
  -a AARCH64 -t GCC -b DEBUG -n 2
```

This executed wrapper builds the ARM application at
`Build/ArmVirtCloudHvSecure-AARCH64/DEBUG_GCC/AARCH64/SecureBootProbe.efi` and
the test firmware at
`Build/ArmVirtCloudHvSecure-AARCH64/DEBUG_GCC/FV/CLOUDHV_EFI.fd`. The
application and DXE observer have the AArch64 PE machine type `0xaa64`. Preserve
the production image before building the test wrapper into its output directory.

Use the
[shared disposable key, seed and assertion procedure](../tests/README.md) with
these ARM outputs: copy `SecureBootProbe.efi` as `unsigned.efi` and
`FV/CLOUDHV_EFI.fd` as `firmware.fd`. After signing, replace that procedure's
AMD64 fallback copy with:

```sh
mcopy -i esp.raw signed.efi ::/EFI/BOOT/BOOTAA64.EFI
```

Keep `EFI/unsigned.efi` and all shared signed/unsigned, protected-variable, TPM
and large-variable assertions. The test-only observer is selected only by this
probe wrapper; it is absent from the production profiles. These probes run
before ExitBootServices and do not establish Windows runtime-variable
correctness. ARM boot, interactive input/PPI, signature enforcement and the 100
ms LED acknowledgement timing still require runtime qualification.
