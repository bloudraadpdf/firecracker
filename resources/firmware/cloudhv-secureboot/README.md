# Secure graphical firmware

This experimental profile passes the recorded standard Windows 11 checks. It
adds TPM2 measured boot and a required host-authenticated variable service to
the graphical CloudHv profile. It does not provide SMM, a writable flash device
or a RAM-variable fallback.

Use EDK2 revision `97eeb7b0901da9f7b5d98a2e6a5e8694978298e9`. Apply these
patches in order with `git apply --ignore-whitespace`:

1. `resources/firmware/cloudhv-firecracker.patch`
1. `resources/firmware/cloudhv-console/cloudhv-console.patch`
1. `resources/firmware/cloudhv-secureboot/cloudhv-secureboot.patch`
1. `resources/firmware/cloudhv-secureboot/cloudhv-physical-presence.patch`
1. `resources/firmware/cloudhv-pvh-memory-map.patch`

After the normal EDK2 BaseTools setup, build:

```sh
build -p OvmfPkg/CloudHv/CloudHvConsoleSecureX64.dsc -a X64 -t GCC -b RELEASE -n 2
```

The image is `Build/CloudHvConsoleSecureX64/RELEASE_GCC/FV/CLOUDHV.fd`. Use a
new immutable filename and record its digest before each validation run. The
previous secure image has SHA-256
`ff6fc396b5eb4daa3de992611814ab9d8084c6981e6bb07a3f44d58a2b948c43`. The
[memory-map guide](../pvh-memory-map.md) records current firmware validation.

The secure profile enables `TPM2_ENABLE` and `SECURE_BOOT_ENABLE`, disables TPM1
support, and uses `VirtMmCommunicationDxe` with `VariableSmmRuntimeDxe`. The
latter name describes the standard MM protocol; this profile sends requests to a
separate host helper, not guest SMM. The variable device is at `0xfed50000`; the
TPM2 CRB is at `0xfed40000`. These pages must be reserved by the VMM. The host
service must be available before firmware starts. Initialisation failure stops
firmware; it cannot silently fall back to unprotected variables.

The shared `VirtMmCommunicationDxe` buffer limit is 2 MiB, matching the VMM and
helper transport. The same header controls runtime-page allocation,
communication bounds and the MMIO buffer-size register. A helper-side large
variable test alone does not cover this firmware limit. The
[isolated probe](tests/README.md) reads a deterministic 67,228-byte synthetic
dbx through the EFI runtime-services interface and checks its exact bytes and
SHA-256 digest before ExitBootServices. Its test-only DXE observer permits a 64
KiB firmware negative control without weakening image verification. This is
separate from Windows runtime-variable qualification after ExitBootServices.

Firmware polls the variable service with a five-second bound, allowing host IPC
and durable writes. The VMM's shorter backend deadline stops the guest if the
service fails. Firmware MMIO reset resets only transport state. Cold boot and
full checkpoint restore are separate host operations.

Enrol a platform owner's PK and the intended KEK/db/dbx trust policy through the
[host helper](../../../tools/uefi-vars-helper/README.md). A build flag alone
does not establish Secure Boot. Acceptance requires a signed EFI application to
run, an unsigned application to be rejected, authenticated update/replay tests,
durable variables across cold boot, and a matched TPM/variable/disk restore. A
TPM backend must allow firmware to perform Startup and PCR measurements rather
than preinitialising the TPM in the host launcher.

The pre-PPI DEBUG build completed with digest
`8e76a0dba5b069e19a300d766d482d486113c276a97d41d22721600b8626351b`. This DEBUG
image passed the isolated EFI probe with a test-only PK/KEK/db:

- The signed application ran with SecureBoot=1 and SetupMode=0.
- Unsigned image loading returned Access Denied. Unsigned PK deletion returned
  Security Violation; writes to derived SecureBoot state returned Write
  Protected.
- TPM2 capability and event-log queries succeeded.
- A nonvolatile counter advanced across two cold boots.
- Full restore from staged variable/TPM stores resumed and shut down cleanly.
- A stopped variable helper caused the next variable request to stop the VMM
  with GenericError exit 1. No request received fabricated success.

These checks used the `SecureBootProbe` application now supplied in
`tests/firmware-tests.patch`, a separately signed application and an untrusted
test CA. They do not establish Windows installation acceptance or a production
trust policy. Never reuse the probe key for deployed guests. The
[probe procedure](tests/README.md) gives the build, signing, provisioning and
bounded execution commands.

The pre-PPI RELEASE image digest is
`c5e2208aa0c8a00ce45dc77d5c2aa24bad1cca92f9a6aab842a2d9766c90bc3e`. It repeated
the signed/unsigned, protected-variable, TPM/event-log and durable counter
checks with the reviewed format-25 VMM and helper containing the large
signature-database restore fix. The probe shut down with exit code 0.
Differential restore also passed: the resumed probe read its persistent counter
through the variable service and completed ACPI S5. Test RAM files were removed.

## Current Windows installation media

The official Windows 11 Enterprise evaluation ISO 26300.9457 has SHA-256
`bc3f24086ebadc94489066b5ad78089e2cf5c3491e90e790bb81a2b199c10e38`, published in
[Microsoft KB5130867](https://aka.ms/VerifyWindowsISO). Its default CD boot
image still uses the revoked 2011 CA. The source WIM also contains the
corresponding 2023-signed boot files.

The official standard Windows 11 26H2 English AMD64 multi-edition ISO has
SHA-256 `bd4307df32bc8af33b39ccecb1174aeb345386630f89a2b86c7a4e36b55ea650`,
checked against Microsoft's
[download verification](https://www.microsoft.com/software-download/windows11).
Its actual Professional image is install.wim index 6, build 26300.9457. Select
`prepare-nt6 win11-pro-26h2`; `win11` continues to select IoT Enterprise LTSC.

`prepare-nt6 win11-enterprise-26h2` and graphical `prepare-nt6 win11-pro-26h2`
follow Microsoft's
[Make2023BootableMedia procedure](https://support.microsoft.com/en-us/servicing/os/windows/2025/02/updating-windows-bootable-media-to-use-the-pca2023-signed-boot-manager):
read boot.wim index 1, copy EFI_EX/bootmgfw_EX.efi, the optional bootmgr_EX.efi,
DVD_EX efisys_EX.bin, FONTS_EX and boot.stl, and select efisys_EX.bin in the El
Torito catalogue. The preparer records each replacement's source and digest. It
does not modify boot.wim or install.wim, remove revocations, disable Secure Boot
or bypass Windows hardware requirements. The other media profiles retain their
original boot images.

Only Pro with an answer file selects the original
`DVD_EX/EFI/en-US/efisys_noprompt_EX.bin`, copied to the same
`efi/microsoft/boot/efisys_ex.bin` catalogue target. Microsoft's reference
script does not select this alternative. Its actual 1,118,000-byte embedded EFI
loader has SHA-256
`03a07689dd54201a69d38e0b62898509267f80f8f078ba8e84aff231b649873d`. The unique
Microsoft Windows leaf matches the prompting CD loader. Isolated
CA2023/code-signing verification with time checks disabled passed; PCA2011 was
rejected, and separate PE-region digest/PKCS7 verification passed. These checks
do not establish current-time, timestamp, firmware or installation acceptance.
The leaf expired in May 2026; PK/KEK/db/dbx remain unchanged.

The Pro boot.wim index 1 contains Windows PE 26100.9457 and the required EX boot
manager, EX EFI CD image, fonts and boot.stl. The EFI catalogue selects
efisys_ex.bin; its FAT image contains EFI/BOOT/BOOTX64.EFI. The external
efi/boot/bootx64.efi is replaced by bootmgfw_EX.efi. The optional bootmgr_EX.efi
is absent, so the original root bootmgr.efi remains unchanged. This follows the
reference procedure's optional-copy rule; it does not prove that the root file
is reached, or that the complete boot chain passes current firmware policy. Keep
PK/KEK/db/dbx unchanged. The EX CD loader's certificate expired in May 2026;
time-disabled chain checks do not qualify firmware or timestamp policy.

The pinned Pro image at index6 now completes a
[fresh installation and cold boot](../../../docs/windows-validation.md#standard-windows-11-pro-installation)
with fresh variable and TPM stores, Secure Boot enabled and installed signed
drivers. No trust-policy or time-policy change was needed. The original
install.wim listing reported "Some files have incorrect reference count";
successful installation of index6 does not establish integrity of every image in
that archive. Support remains experimental. Earlier isolated boot-file checks
are not independent current-time or timestamp-policy qualification.

The reference procedure is pinned to `scripts/windows/Make2023BootableMedia.ps1`
at Microsoft secureboot_objects revision
`daa32713651f2ffe97b18df5aaf93ffe238aa464`. The Rust preparer uses existing
7-Zip extraction for these file copies and the explicit unattended Pro
alternative on Linux, then verifies every file in the rebuilt UDF image against
the prepared tree.

The EFI probe runs before ExitBootServices. It does not substitute for Windows
runtime-variable checks after virtual-address conversion.

## Persistent physical presence interface

The secure profile uses EDK2's variable-backed physical presence library. It
connects the default consoles before processing requests, then preserves the
existing ReadyToLock ordering that protects the TPM platform hierarchy. Flags
are initialised before their variable policy locks them. Missing policy support,
malformed flags or failed initialisation stops firmware.

Only operations 0, 5, 14, 21 and 22 are accepted. Direct writes of other
requests receive BIOS failure without changing the TPM. The flags remain locked
through runtime. TCG PPI 1.30 Table 2 requires confirmation for combined clear
operations when either the clear or turn-on flag is set; the secure profile
applies that condition in addition to the pinned library's clear-only test.

Validation uses disposable TPM and variable stores, never an installed guest's
TPM. The required sequence is:

- [x] Cold boot initialises flags and rejects runtime flag writes.
- [x] A pending clear displays the confirmation prompt before OS boot.
- [x] Escape records user abort without executing clear.
- [x] F12 executes clear, records success, consumes the request and resets.
- [x] Combined clear with only the turn-on flag set still requires confirmation.
- [x] Unsupported direct requests record BIOS failure without TPM action.
- [x] Headless and ordinary graphical firmware builds remain unchanged.

The PPI RELEASE image has SHA-256
`8686fa6c62eb6ec808efe353a04ec1f86bb71df9c93d7623caf89b4a4dad58d1`. With the
format-26 VMM and PPI helper it passed the disposable-store sequence above. The
TPM effect was checked independently of the writable result variable: owner NV
index `0x01500020` remained readable after Escape, then returned `TPM_RC_HANDLE`
after F12 and cold restart. The separate UEFI marker remained, so the probe did
not recreate the index. Secure Boot remained enabled and the signed probe
completed ACPI S5. This does not authorise clearing an installed Windows guest's
TPM.

Installed Windows 11 Enterprise Evaluation 26300.9457 also booted this firmware
with its existing matched disk and TPM state. `Get-Tpm` returned present, ready,
enabled, activated and owned; its PPI version and WMI physical-presence version
were 1.3. `Confirm-SecureBootUEFI` returned true, and the public TBS
`TPM2_GetRandom` probe succeeded. These installed-guest checks only queried the
TPM; no clear request was submitted.
