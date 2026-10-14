# Authenticated UEFI variable service

This separately built GPL-2.0-or-later helper reuses QEMU's variable service.
It is not linked into Firecracker. The firmware uses EDK2's BSD-licensed
`VirtMmCommunicationDxe` transport and `VariableSmmRuntimeDxe` client.

Implementation checklist:

- [x] Inspect upstream authentication, policy and firmware transport boundaries.
- [x] Pin and preserve the upstream engine, notices and reproducible build inputs.
- [x] Add bounded IPC and persistent state without replacing upstream authentication.
- [x] Test authentication failures, replay, policy locks and checkpoint consistency.
- [x] Enable the console firmware client and test signed and unsigned EFI images.
- [ ] Verify standard Windows 11 installation and measured boot.

## Boundary

The helper receives copied messages, never guest memory mappings or arbitrary
host paths from the guest. The VMM validates MMIO access and guest DMA ranges.
Only the host management process can provision a store or restore a checkpoint.
Guest messages cannot reset the boot phase or import state. The helper owns
PKCS#7 verification, authenticated timestamps, PK/KEK/db/dbx hierarchy and
variable policy. Secure Boot is forced and custom mode is disabled after host
provisioning. Host administrators and the VMM are trusted.

The transport occupies one reserved page at `0xfed50000`. Registers follow
`OvmfPkg/Include/IndustryStandard/QemuUefiVars.h`; firmware uses DMA messages.
The TPM CRB page at `0xfed40000` is separate.

## IPC version 1

One connected Unix stream carries serial requests. Each header is 16 bytes:
`FCUV`, little-endian u16 version `1`, u16 opcode, u32 request ID and u32
payload length. Replies repeat the ID and set opcode bit 15. The reply starts
with a u32 service status: zero success, one invalid request, two invalid
state, three persistence failure, four authentication/provisioning failure.
Malformed frames, wrong versions and lengths above 4 MiB close the connection.
No pipelining or unsolicited messages are allowed.

| Opcode | Request | Successful reply after service status |
|---|---|---|
| 1 | Complete EFI MM message, at most 65536 bytes | u32 UEFI transport status, then complete modified message |
| 2 | Empty | Complete checkpoint, at most 4 MiB minus 256 bytes |
| 3 | Checkpoint, only before the first guest message | Empty |
| 4 | Empty; host cold-boot transition only | Empty |
| 5 | Four u32: PPI function, revision, operation, argument | Three u32: PPI return code, first result, second result |

Provisioning is a separate host command before a socket is exposed. It is not
a guest-facing IPC operation. A cold boot clears volatile variables and boot
policies while retaining authenticated nonvolatile state. Firmware MMIO RESET
only resets transport registers; it must not clear security lifecycle state.

## TPM physical presence

Opcode 5 carries little-endian integers in exactly 16 request bytes and 12
reply bytes after the service status. It requires cold-boot initialisation or
checkpoint restore and closes the management restore boundary like opcode 1.

| PPI function | Accepted revisions | Result fields after the PPI code |
|---|---|---|
| 2: submit | 1 | Zero, zero |
| 3: pending | 1, 2 | Request, parameter (zero for revision 1) |
| 5: response | 1 | Last request, firmware result |
| 7: submit | 1, 2 | Zero, zero |
| 8: confirmation | 1 | Zero, zero |

Functions 3 and 5 require an operation of zero. The argument must be zero,
except function 7 revision 2 permits the PCR-bank argument for operation 23,
which this implementation reports as unsupported. Unknown functions,
revisions, unused arguments or payload lengths receive service status 1.
The VMM implements the static ACPI functions separately.

The helper accepts only TPM2 mandatory operations 0, 5, 14, 21 and 22. Other
operations return PPI code 1 on submission and code 0 on confirmation query.
Only firmware executes requests and obtains user confirmation. The helper
never clears a TPM or treats request acceptance as execution success.

Requests use the standard EDK2 `Tcg2PhysicalPresence` variable under GUID
`aeb9c5c1-94f1-4d02-bfd9-4602db2d3c54`, attributes 7. Its 16-byte record has
request at byte 0, u32 parameter at byte 4, last request at byte 8 and u32
response at byte 12. Submission preserves the previous response and uses
the normal SetVariable engine, including registered policies. Firmware must
create this record and the four-byte `Tcg2PhysicalPresenceFlags`, then protect
the flags with its variable lock policy before handing control to the OS.
Opcode 5 cannot change those flags. Confirmation queries use the clear mask
0x2 and, for combined enable/clear operations, the turn-on mask 0x10.

Missing or invalid records fail rather than creating synthetic firmware state:
submission returns PPI code 2, pending/response reads return code 1, and a
confirmation query returns service status 2. The VMM must stop on a helper
service failure. Failed SetVariable requests return PPI code 2 without
bypassing a variable lock.

References: [TCG PPI 1.30](https://trustedcomputinggroup.org/wp-content/uploads/Physical-Presence-Interface_1-30_0-52.pdf)
and [EDK2 variable layout](https://github.com/tianocore/edk2/blob/master/SecurityPkg/Include/Guid/Tcg2PhysicalPresenceData.h).

## Persistence and restore

A store has a unique identity and monotonically increasing generation. An
exclusive host lock prevents concurrent writers. Every successful persistent
mutation is committed with a file sync, atomic replacement and directory sync
before the guest receives success. A failed commit terminates the connection;
the VMM must stop rather than continue with uncommitted security state.

A checkpoint contains the identity, generation, all variable bytes,
authenticated timestamps and signer digests, all variable policies, and boot
lifecycle flags. The VMM must capture it while vCPUs are stopped together with
TPM, memory and disk state. Ordinary resume requires the expected generation.
Intentional rollback must be an explicit management operation that installs
the complete matched checkpoint. Management generation advances from the live
generation even during rollback; it never returns to the snapshot generation.
The 256-byte encoding reserve leaves room for the restore request envelope.
Clones require separate stores and ownership.
Restoring an old checkpoint also restores its old revocation policy; a snapshot
is not a means to retain later dbx updates while rolling back everything else.

The VMM cannot atomically commit changes across the variable helper, TPM helper
and disk files. Restore into isolated staging copies of all three. If any load
step fails, stop both helpers and discard those copies. Publish the matched
set only after the whole VM load succeeds. Never restore directly over the
only live store. The generation check protects against an accidental overwrite;
it does not turn separate files into a transaction.

## Distribution

The helper and its engine are GPL-2.0-or-later. Distributions must supply the
corresponding source, local changes, build instructions and licence notices.
The Apache-licensed VMM uses only the documented process protocol. The helper
requires the real GnuTLS PKCS#7 implementation; QEMU's authentication stub must
never be built. Firmware code remains under its upstream BSD terms.

The pinned sources remain byte-for-byte intact in `vendor/qemu`; the manifest
records their revision and SHA-256 digests, checked at build time. Explicit patches in `patches/`
remove QEMU migration descriptors and their two unused callbacks, retaining
the restore validators. The build applies them with zero fuzz. The Rust codec replaces QEMU migration serialisation.
The following corrections apply to generated copies:

- Recognise the ASN.1 ContentInfo context tag `0xa0`, not `0x0a`, before
  adding an envelope to bare SignedData. A valid enveloped signature otherwise
  receives a second envelope and fails GnuTLS import.
- Reject equal timestamps for non-append authenticated updates. UEFI 2.10
  section 8.2.6 requires a strictly later timestamp. Replay after restore is
  covered by a signed fixture regression.
- Retain the greater timestamp during an authenticated append, and reject
  nonzero reserved timestamp fields. Both have signed fixture regressions.
- Reject deletion of an authenticated variable without an authentication
  descriptor. A zero-length delete must not remove PK or its peers.
- Make derived SecureBoot, SignatureSupport and VendorKeys state read-only
  to guest requests; only the service derives these values.
- Accept valid namespace-wide policies with no variable name, including after
  restore. EDK2 emits this policy form for protected namespaces.

A signed platform-key deletion updates `SetupMode` and `SecureBoot` in the same
transaction. The regression uses a signed empty PK payload and checks that the
result survives restore. This follows the immediate mode transition described in
[UEFI 2.10 section 32](https://uefi.org/specs/UEFI/2.10/32_Secure_Boot_and_Driver_Signing.html).

The fixture certificate is an untrusted test key. Never enrol it in a guest
used outside these tests. Its detached signature was generated by OpenSSL
over the UEFI name/GUID/attributes/timestamp/data byte sequence; tests use
GnuTLS through the actual upstream engine to verify it.

## Build

Install a C compiler, patch, pkg-config, GLib development headers and GnuTLS development
headers. On Debian these packages are `build-essential patch pkg-config libglib2.0-dev
libgnutls28-dev`. Build and test the separate workspace:

```sh
mise exec -- cargo +1.98.1 test --locked --manifest-path tools/uefi-vars-helper/Cargo.toml
mise exec -- cargo +1.98.1 clippy --locked --manifest-path tools/uefi-vars-helper/Cargo.toml --all-targets -- -D warnings
mise exec -- cargo +1.98.1 build --release --locked --manifest-path tools/uefi-vars-helper/Cargo.toml
```

Provision with `firecracker-uefi-vars provision STORE SEED_JSON`, then serve with
`firecracker-uefi-vars serve STORE SOCKET`. The seed is the serialised `State`
from `src/engine.rs`: zero lifecycle flags, no policies, and PK, KEK and db
variables containing public EFI signature lists. Use attributes `0x27`, UTF-16LE
names including NUL, raw EFI GUID bytes and a 16-byte authenticated timestamp.
Do not include private keys. Provisioning rejects absent required keys or a
resulting disabled Secure Boot state. Checkpoint and restore field definitions
are in `src/service.rs`; restore wraps the checkpoint with `expected_generation`.

## Microsoft policy for AMD64 acceptance

`seed-microsoft` builds a public-key seed from Microsoft `secureboot_objects`
revision `daa32713651f2ffe97b18df5aaf93ffe238aa464`. This is an explicit,
versioned policy, not an automatic revocation-update service. The command
checks SHA-256 for every input before encoding EFI signature lists. Download
these files from the pinned revision into one source directory, retaining
their basenames:

- `PreSignedObjects/PK/Certificate/WindowsOEMDevicesPK.der`
- `PreSignedObjects/KEK/Certificates/microsoft corporation kek 2k ca 2023.der`
- `PreSignedObjects/DB/Certificates/windows uefi ca 2023.der`
- `PreSignedObjects/DB/Certificates/MicWinProPCA2011_2011-10-19.der`
- `PreSignedObjects/DBX/dbx_info_msft_latest.json`

The raw-download prefix is
`https://raw.githubusercontent.com/microsoft/secureboot_objects/daa32713651f2ffe97b18df5aaf93ffe238aa464/`.
Percent-encode spaces in URLs. Retain the upstream BSD-2-Clause-Patent licence
if distributing these inputs. Then run:

```sh
firecracker-uefi-vars seed-microsoft SOURCE_DIRECTORY seed.json
firecracker-uefi-vars provision store.json seed.json
```

The resulting policy uses Microsoft's platform key, 2023 KEK and 2023 Windows
boot CA. It includes all pinned AMD64 image revocations, the revoked 2011
Windows boot CA and the published SVN records. It does not trust the EFI probe
certificate or arbitrary third-party EFI programs. The old 26100.1742 Toets
installation medium was rejected by this policy during acceptance. Use current
signed installation media; do not remove revocations to boot an old image.

See [Microsoft key management guidance](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/windows-secure-boot-key-creation-and-management-guidance)
and the [upstream objects](https://github.com/microsoft/secureboot_objects).
