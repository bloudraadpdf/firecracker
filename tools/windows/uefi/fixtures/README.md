# Public authenticated-variable fixtures

These first-party Apache-2.0 test inputs were generated with OpenSSL 4.0.3 on 16
October 2026. `owner.pem` and `other.pem` are self-signed RSA-2048/SHA-256
public test certificates with subjects `Firecracker-Runtime-Test-owner` and
`Firecracker-Runtime-Test-other`. Their private keys were discarded. Do not use
these public, replayable test updates to protect production data.

Each `.auth` file contains an `EFI_VARIABLE_AUTHENTICATION_2`: a 16-byte
`EFI_TIME`, then `WIN_CERTIFICATE_UEFI_GUID` (revision 0x0200, type 0x0ef1,
PKCS#7 certificate GUID), detached DER CMS SignedData, then the payload. The
certificate length includes its 24-byte header and CMS bytes. The signed bytes
are the UTF-16LE name without a terminator, the UEFI-encoded vendor GUID,
little-endian 32-bit attributes, the 16-byte timestamp and the payload.

All updates target `FirecrackerRuntimeTest`, GUID
`7bc4c67c-16f4-4df6-a24e-a3caa7904f09`, attributes `0x27` (nonvolatile,
boot-service, runtime and time-based authenticated access). Timestamps are
2026-10-16 00:00 with the seconds below; all other EFI_TIME fields are zero.
Payload files contain the exact ASCII bytes with no newline.

| Update        | Second | Signer                       | Payload                       |
| ------------- | ------ | ---------------------------- | ----------------------------- |
| create        | 10     | owner                        | `firecracker-runtime-state-1` |
| replace       | 20     | owner                        | `firecracker-runtime-state-2` |
| replace-again | 30     | owner                        | `firecracker-runtime-state-3` |
| wrong-owner   | 40     | other                        | `firecracker-runtime-state-4` |
| tampered      | 45     | owner signature from replace | changed final payload byte    |
| delete        | 50     | owner                        | empty                         |

The tampered copy changes the replacement timestamp to second 45 and its final
payload byte without replacing the signature. The wrong-owner update has a valid
signature from the other certificate. The replay case resubmits `create.auth`.
Detached signatures and their exact signed content were checked with OpenSSL;
that check does not establish certificate trust or Windows runtime acceptance.
Only the nine buffers consumed by the probe and the two public certificates are
retained here. No private key, generated executable or duplicate CMS file is
required.
