# Secure graphical firmware

This opt-in profile is under validation. It adds TPM2 measured boot and a
required host-authenticated variable service to the graphical CloudHv profile.
It does not provide SMM, a writable flash device or a RAM-variable fallback.

Use EDK2 revision `97eeb7b0901da9f7b5d98a2e6a5e8694978298e9`. Apply these patches
in order with `git apply --ignore-whitespace`:

1. `resources/firmware/cloudhv-firecracker.patch`
2. `resources/firmware/cloudhv-console/cloudhv-console.patch`
3. `resources/firmware/cloudhv-secureboot/cloudhv-secureboot.patch`
4. `resources/firmware/cloudhv-secureboot/cloudhv-physical-presence.patch`
5. `resources/firmware/cloudhv-pvh-memory-map.patch`

After the normal EDK2 BaseTools setup, build:

```sh
build -p OvmfPkg/CloudHv/CloudHvConsoleSecureX64.dsc -a X64 -t GCC -b RELEASE -n 2
```

The image is `Build/CloudHvConsoleSecureX64/RELEASE_GCC/FV/CLOUDHV.fd`.
Use a new immutable filename and record its digest before each validation run.
The headless and ordinary console profiles keep their previous settings.
Both `CloudHvX64.dsc` and `CloudHvConsoleX64.dsc` completed RELEASE regression
builds with the physical-presence patch applied. The ordinary console image remained byte-for-byte
identical to the USB-only image with SHA-256
`b5a2d30c310d2027d83ed3536ba75d28e2c507dfe5830bb1e9372a50f9c8badd`.

The secure profile enables `TPM2_ENABLE` and `SECURE_BOOT_ENABLE`, disables
TPM1 support, and uses `VirtMmCommunicationDxe` with
`VariableSmmRuntimeDxe`. The latter name describes the standard MM protocol;
this profile sends requests to a separate host helper, not guest SMM.
The variable device is at `0xfed50000`; the TPM2 CRB is at `0xfed40000`.
These pages must be reserved by the VMM. The host service must be available
before firmware starts. Initialisation failure stops firmware; it cannot
silently fall back to unprotected variables.

Firmware polls the variable service with a five-second bound, allowing host
IPC and durable writes. The VMM's shorter backend deadline stops the guest
if the service fails. Firmware MMIO reset resets only transport state.
Cold boot and full checkpoint restore are separate host operations.

Enrol a platform owner's PK and the intended KEK/db/dbx trust policy through
the [host helper](../../../tools/uefi-vars-helper/README.md). A build flag alone
does not establish Secure Boot. Acceptance requires a signed EFI application
to run, an unsigned application to be rejected, authenticated update/replay
tests, durable variables across cold boot, and a matched TPM/variable/disk
restore. A TPM backend must allow firmware to perform Startup and PCR
measurements rather than preinitialising the TPM in the host launcher.

The pre-PPI DEBUG build completed with digest
`8e76a0dba5b069e19a300d766d482d486113c276a97d41d22721600b8626351b`.
This DEBUG image passed the isolated EFI probe with a test-only PK/KEK/db:

- The signed application ran with SecureBoot=1 and SetupMode=0.
- Unsigned image loading returned Access Denied. Unsigned PK deletion returned
  Security Violation; writes to derived SecureBoot state returned Write Protected.
- TPM2 capability and event-log queries succeeded.
- A nonvolatile counter advanced across two cold boots.
- Full restore from staged variable/TPM stores resumed and shut down cleanly.
- A stopped variable helper caused the next variable request to stop the VMM
  with GenericError exit 1. No request received fabricated success.

These checks used `tests/SecureBootProbe.c`, a separately signed application
and an untrusted test CA. They do not establish Windows installation acceptance
or a production trust policy. Never reuse the probe key for deployed guests.
The [probe procedure](tests/README.md) gives the build, signing, provisioning
and bounded execution commands.

The pre-PPI RELEASE image digest is
`c5e2208aa0c8a00ce45dc77d5c2aa24bad1cca92f9a6aab842a2d9766c90bc3e`.
It repeated the signed/unsigned, protected-variable, TPM/event-log and durable
counter checks with the reviewed format-25 VMM and helper containing the
large signature-database restore fix. The probe shut down with exit code 0.
Differential restore also passed: the resumed probe read its persistent counter
through the variable service and completed ACPI S5. Test RAM files were removed.

## Current Windows installation media

The official Windows 11 Enterprise evaluation ISO 26300.9457 has SHA-256
`bc3f24086ebadc94489066b5ad78089e2cf5c3491e90e790bb81a2b199c10e38`,
published in [Microsoft KB5130867](https://aka.ms/VerifyWindowsISO).
Its default CD boot image still uses the revoked 2011 CA. The source WIM also
contains the corresponding 2023-signed boot files.

`prepare-nt6 win11-enterprise-26h2` follows Microsoft's
[Make2023BootableMedia procedure](https://support.microsoft.com/en-us/servicing/os/windows/2025/02/updating-windows-bootable-media-to-use-the-pca2023-signed-boot-manager):
read boot.wim index 1, copy EFI_EX/bootmgfw_EX.efi, the optional bootmgr_EX.efi,
DVD_EX efisys_EX.bin, FONTS_EX and boot.stl, and select efisys_EX.bin in the
El Torito catalogue. The preparer records each replacement's source and digest.
It does not modify boot.wim or install.wim, remove revocations, disable Secure
Boot or bypass Windows hardware requirements. The other media profiles retain
their original boot images.

The reference procedure is pinned to `scripts/windows/Make2023BootableMedia.ps1`
at Microsoft secureboot_objects revision
`daa32713651f2ffe97b18df5aaf93ffe238aa464`. The Rust preparer uses existing 7-Zip
extraction to perform the same file copies on Linux, then verifies every file
in the rebuilt UDF image against the prepared tree.

The EFI probe runs before ExitBootServices. It does not substitute for Windows
runtime-variable checks after virtual-address conversion.

## Persistent physical presence interface

The secure profile uses EDK2's variable-backed physical presence library. It
connects the default consoles before processing requests, then preserves the
existing ReadyToLock ordering that protects the TPM platform hierarchy.
Flags are initialised before their variable policy locks them. Missing policy
support, malformed flags or failed initialisation stops firmware.

Only operations 0, 5, 14, 21 and 22 are accepted. Direct writes of other requests
receive BIOS failure without changing the TPM. The flags remain locked through
runtime. TCG PPI 1.30 Table 2 requires confirmation for combined clear operations
when either the clear or turn-on flag is set; the secure profile applies that
condition in addition to the pinned library's clear-only test.

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
`8686fa6c62eb6ec808efe353a04ec1f86bb71df9c93d7623caf89b4a4dad58d1`.
With the format-26 VMM and PPI helper it passed the disposable-store sequence
above. The TPM effect was checked independently of the writable result variable:
owner NV index `0x01500020` remained readable after Escape, then returned
`TPM_RC_HANDLE` after F12 and cold restart. The separate UEFI marker remained,
so the probe did not recreate the index. Secure Boot remained enabled and the
signed probe completed ACPI S5. This does not authorise clearing an installed
Windows guest's TPM.

Installed Windows 11 Enterprise Evaluation 26300.9457 also booted this firmware
with its existing matched disk and TPM state. `Get-Tpm` returned present, ready,
enabled, activated and owned; its PPI version and WMI physical-presence version
were 1.3. `Confirm-SecureBootUEFI` returned true, and the public TBS
`TPM2_GetRandom` probe succeeded. These installed-guest checks only queried the
TPM; no clear request was submitted.
