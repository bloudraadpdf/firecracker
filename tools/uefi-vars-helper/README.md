# Authenticated UEFI variable service

This separately built Apache-2.0 Rust helper implements authenticated UEFI
variables. It is not linked into Firecracker. The firmware uses EDK2's
BSD-licensed `VirtMmCommunicationDxe` transport and `VariableSmmRuntimeDxe`
client.

## Boundary

The helper receives copied messages, never guest memory mappings or arbitrary
host paths from the guest. The VMM validates MMIO access and guest DMA ranges.
Only the host management process can provision a store or restore a checkpoint.
Guest messages cannot reset the boot phase or import state. The helper owns
PKCS#7 verification, authenticated timestamps, PK/KEK/db/dbx hierarchy and
variable policy. Secure Boot is forced and custom mode is disabled after host
provisioning. Host administrators and the VMM are trusted.

## IPC version 1

One connected Unix stream carries serial requests. Each header is 16 bytes:
`FCUV`, little-endian u16 version `1`, u16 opcode, u32 request ID and u32
payload length. Replies repeat the ID and set opcode bit 15. The reply starts
with a u32 service status: zero success, one invalid request, two invalid state.
Persistence or engine failures close the connection; provisioning is a separate
host command. Malformed frames, wrong versions and lengths above 4 MiB close the
connection. No pipelining or unsolicited messages are allowed.

| Opcode | Request                                               | Successful reply after service status                     |
| ------ | ----------------------------------------------------- | --------------------------------------------------------- |
| 1      | Complete EFI MM message, at most 2 MiB                | u32 UEFI transport status, then complete modified message |
| 2      | Empty                                                 | Complete checkpoint, at most 4 MiB minus 256 bytes        |
| 3      | Checkpoint, only before the first guest message       | Empty                                                     |
| 4      | Empty; host cold-boot transition only                 | Empty                                                     |
| 5      | Four u32: PPI function, revision, operation, argument | Three u32: PPI return code, first result, second result   |

Provisioning is a separate host command before a socket is exposed. It is not a
guest-facing IPC operation. A cold boot clears volatile variables and boot
policies while retaining authenticated nonvolatile state. Firmware MMIO RESET
only resets transport registers; it must not clear security lifecycle state.

## TPM physical presence

Opcode 5 carries little-endian integers in exactly 16 request bytes and 12 reply
bytes after the service status. It requires cold-boot initialisation or
checkpoint restore and closes the management restore boundary like opcode 1.

| PPI function    | Accepted revisions | Result fields after the PPI code         |
| --------------- | ------------------ | ---------------------------------------- |
| 2: submit       | 1                  | Zero, zero                               |
| 3: pending      | 1, 2               | Request, parameter (zero for revision 1) |
| 5: response     | 1                  | Last request, firmware result            |
| 7: submit       | 1, 2               | Zero, zero                               |
| 8: confirmation | 1                  | Zero, zero                               |

Functions 3 and 5 require an operation of zero. The argument must be zero,
except function 7 revision 2 permits the PCR-bank argument for operation 23,
which this implementation reports as unsupported. Unknown functions, revisions,
unused arguments or payload lengths receive service status 1. An integrating VMM
supplies the static ACPI functions separately.

The helper accepts only TPM2 mandatory operations 0, 5, 14, 21 and 22. Other
operations return PPI code 1 on submission and code 0 on confirmation query.
Only firmware executes requests and obtains user confirmation. The helper never
clears a TPM or treats request acceptance as execution success.

Requests use the standard EDK2 `Tcg2PhysicalPresence` variable under GUID
`aeb9c5c1-94f1-4d02-bfd9-4602db2d3c54`, attributes 7. Its 16-byte record has
request at byte 0, u32 parameter at byte 4, last request at byte 8 and u32
response at byte 12. Submission preserves the previous response and uses the
normal SetVariable engine, including registered policies. Firmware must create
this record and the four-byte `Tcg2PhysicalPresenceFlags`, then protect the
flags with its variable lock policy before handing control to the OS. Opcode 5
cannot change those flags. Confirmation queries use the clear mask 0x2 and, for
combined enable/clear operations, the turn-on mask 0x10.

Missing or invalid records fail rather than creating synthetic firmware state:
submission returns PPI code 2, pending/response reads return code 1, and a
confirmation query returns service status 2. The VMM must stop on a helper
service failure. Failed SetVariable requests return PPI code 2 without bypassing
a variable lock.

References:
[TCG PPI 1.30](https://trustedcomputinggroup.org/wp-content/uploads/Physical-Presence-Interface_1-30_0-52.pdf)
and
[EDK2 variable layout](https://github.com/tianocore/edk2/blob/master/SecurityPkg/Include/Guid/Tcg2PhysicalPresenceData.h).

## Persistence and restore

The persisted checkpoint format is **version 2**. Version 1 stores and snapshot
checkpoints are rejected on open and management restore before changing engine
state or committed bytes. The earlier format used a different private
authenticated-variable signer digest; its 32-byte value cannot identify its
encoding. There is no migration. Do not edit a version field, remove private
variables or combine a fresh store with an old matched disk/TPM reference.
Preserve old matched sets; provision a new store and qualify its new matched
guest state. The `FCUV` frame remains version 1; its checkpoint payload carries
the separate persisted-format version.

A store has a unique identity and monotonically increasing generation. An
exclusive host lock prevents concurrent writers. Every successful persistent
mutation is committed with a file sync, atomic replacement and directory sync
before the guest receives success. A failed commit terminates the connection;
the VMM must stop rather than continue with uncommitted security state.

An update that leaves the complete variable record unchanged still runs
authentication and policy checks. It does not replace the checkpoint or advance
its generation.

The helper appends `.lock` and `.pending` to the complete store filename. After
restart, it acquires the lock and validates the committed store before removing
an orphaned pending write. It never imports uncommitted pending data. The
removal is synced before serving requests. Keep these files in the store's
private directory.

A checkpoint contains the identity, generation, all variable bytes,
authenticated timestamps and signer digests, all variable policies, and boot
lifecycle flags. The VMM must capture it while vCPUs are stopped together with
TPM, memory and disk state. Ordinary resume requires the expected generation.
Intentional rollback must be an explicit management operation that installs the
complete matched checkpoint. Management generation advances from the live
generation even during rollback; it never returns to the snapshot generation.
The 256-byte encoding reserve leaves room for the restore request envelope.
Clones require separate stores and ownership. Restoring an old checkpoint also
restores its old revocation policy; a snapshot is not a means to retain later
dbx updates while rolling back everything else.

The VMM cannot atomically commit changes across the variable helper, TPM helper
and disk files. Restore into isolated staging copies of all three. If any load
step fails, stop both helpers and discard those copies. Publish the matched set
only after the whole VM load succeeds. Never restore directly over the only live
store. The generation check protects against an accidental overwrite; it does
not turn separate files into a transaction.

## Implementation and licence

The helper uses the repository's Apache-2.0 licence. Its variable engine, policy
checks and durable state are Rust. CMS/PKCS#7, DER and X.509 parsing use
RustCrypto crates. RSA verification uses the same AWS-LC C/assembly backend and
pinned versions as the VMM: aws-lc-rs 1.18.0 and aws-lc-sys 0.44.0. Building the
helper needs the existing AWS-LC C compiler and assembly toolchain. The helper
has no QEMU source, C shim, GLib, GnuTLS or OpenSSL dependency. Normal linking
uses the system linker driver. Firmware remains a separate dependency under its
existing upstream licence.

Aggregate variable/policy storage is bounded to 1 MiB. The MM transport permits
2 MiB so a complete stored variable, including a growing dbx database, remains
readable with its protocol headers. QueryVariableInfo reports storage capacity
and per-variable overhead, not the larger transport limit. Its authenticated
maximum reserves the private-variable signer digest as a safe class bound.

The engine implements the 64-bit EDK II MM variable and variable-policy
interfaces. Policies are registered during trusted firmware initialisation and
are enforced after EndOfDxe, matching the pinned EDK II variable-check path.
ReadyToBoot or ExitBootServices also activates enforcement if an earlier
notification is omitted. Locking policy registration does not end firmware
initialisation. Derived Secure Boot state and authenticated updates retain their
separate protection before and after that boundary. The saved lifecycle flags
preserve policy activation across restore; cold boot clears volatile state and
policies so firmware can initialise them again. This fixes activation within the
existing version-2 checkpoint fields; it does not change their encoding or the
version-1 FCUV frame. The protocol definitions come from the pinned firmware
headers `MdeModulePkg/Include/Guid/SmmVariableCommon.h`,
`MdeModulePkg/Include/Guid/VarCheckPolicyMmi.h` and
`MdeModulePkg/Include/Protocol/VariablePolicy.h`. Authentication follows
[UEFI variable services](https://uefi.org/specs/UEFI/2.11/08_Services_Runtime_Services.html#using-the-efi-variable-authentication-2-descriptor):

- Verify RSA/SHA-256 detached signatures over the name, GUID, attributes,
  timestamp and new data. Accept ContentInfo and bare SignedData encodings.
- Require a strictly later timestamp for replacement. Append retains the greater
  timestamp and removes duplicate signature-database entries.
- Require the current PK key itself for PK/KEK updates. Image-database updates
  accept that key or a certificate chain to KEK. Recovery variables accept that
  key or a chain to KEK/dbr; their payload is opaque, not a signature database.
- Bound certificate-chain search to 16 certificates, visiting each once. Retain
  private authenticated-variable signer identity across checkpoint restore.
- Reject tampered updates, unauthenticated deletion and guest writes to derived
  Secure Boot state. Signed PK deletion updates the mode in the same
  transaction.
- Preserve variable policy and lifecycle locks across checkpoint restore; cold
  boot clears volatile variables and boot-lifecycle policy.

Optional platform default-key variables are read-only. AuditMode and
DeployedMode transitions are unsupported; writes and stores containing those
modes are rejected. The engine does not implement the optional UEFI
audit/deployed modes.

The fixture certificate is an untrusted test key. Never enrol it in deployed
guests. The existing signed fixtures are verified directly by the Rust engine.

## Build

Build and test the separate workspace with the repository's Rustup toolchain.
Follow the shared
[Rust build prerequisites](../windows/README.md#build-prerequisites).

```sh
cargo test --locked --manifest-path tools/uefi-vars-helper/Cargo.toml
cargo clippy --locked --manifest-path tools/uefi-vars-helper/Cargo.toml --all-targets -- -D warnings
cargo build --release --locked --manifest-path tools/uefi-vars-helper/Cargo.toml
```

The executable is `build/cargo_target/release/firecracker-uefi-vars`. Commands
below use its name; add that directory to `PATH` before changing directories:

```sh
export PATH="$PWD/build/cargo_target/release:$PATH"
```

Provision with `firecracker-uefi-vars provision STORE SEED_JSON`, then serve
with `firecracker-uefi-vars serve STORE SOCKET`. The seed is the serialised
`State` from `src/engine.rs`: zero lifecycle flags, no policies, and PK, KEK and
db variables containing public EFI signature lists. Use attributes `0x27`,
UTF-16LE names including NUL, raw EFI GUID bytes and a 16-byte authenticated
timestamp. Do not include private keys. Provisioning rejects absent required
keys or a resulting disabled Secure Boot state. Checkpoint and restore field
definitions are in `src/service.rs`; restore wraps the checkpoint with
`expected_generation`.

## Microsoft policy for AMD64 acceptance

`seed-microsoft` builds a public-key seed from Microsoft `secureboot_objects`
revision `daa32713651f2ffe97b18df5aaf93ffe238aa464`. This is an explicit,
versioned policy, not an automatic revocation-update service. The command checks
SHA-256 for every input before encoding EFI signature lists. Download these
files from the pinned revision into one source directory, retaining their
basenames:

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

See
[Microsoft key management guidance](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/windows-secure-boot-key-creation-and-management-guidance)
and the [upstream objects](https://github.com/microsoft/secureboot_objects).
