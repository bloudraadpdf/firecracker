# Windows TPM runtime probe

`probe.rs` uses the public Windows TPM Base Services API to create a TPM2-only
context and issue four commands: `GetRandom` for 16 bytes, `PCR_Read` for
SHA-256 PCR 7, `GetCapability` for the lowest existing persistent handle, and
`ReadPublic` for that handle. It closes the context on success and command
failure. It does not clear the TPM or change its persistent objects, ownership,
PCRs or disk protectors. It never creates or provisions a key when no persistent
handle exists.

Responses are bounded to 1 KiB. The probe checks TBS status, TPM response
headers, lengths and return codes, the exact PCR selection and 32-byte digest,
the persistent handle range, and public-key/Name framing. It accepts RSA or ECC
public objects with SHA-1, SHA-256, SHA-384, SHA-512 or SM3 Names.
Algorithm-specific public parameters are retained as opaque bytes; this probe
does not verify a signature or recompute the Name. Blocked commands, absent
handles, unsupported key types and invalid response framing cause a nonzero
exit. No policy bypass is attempted.

Successful output contains exactly four canonical, lowercase hexadecimal lines:

```text
TPM_STATE PCR7_SHA256=<32-byte digest>
TPM_STATE HANDLE=<4-byte handle>
TPM_STATE PUBLIC=<TPMT_PUBLIC bytes, without the TPM2B size>
TPM_STATE NAME=<Name algorithm and digest, without the TPM2B size>
```

Random bytes and the PCR update counter are excluded. Require exit zero and both
`PASS Windows TBS TPM2_GetRandom` and
`PASS Windows TBS TPM2_PCR_Read TPM2_ReadPublic`. Save these four lines at cold
stage 1; compare them exactly at full and differential restore stages 2 and 3.
The same existing handle and public bytes establish retained public identity;
the PCR digest establishes retained PCR 7. Keep the source baseline with the
host evidence. A separate cold reboot may change PCR 7 when measured firmware,
policy or boot inputs change, so it needs its own measured-boot context.

This supplements Get-Tpm, physical-presence and Secure Boot checks. Public reads
do not prove possession of a private key or replace TPM attestation.

Follow the [Rust build prerequisites](../README.md#build-prerequisites) and
select the Windows 7 x64 library profile in the
[shared probe link procedure](../README.md#windows-probe-linking). This selects
the loader/CRT floor; the TPM2-only TBS context also requires Windows 8 or later
and an available TPM2 device. Run from the repository root in the same Bash
session. Keep generated files in the ignored build directory:

```sh
mkdir -p build/windows/tpm
rustc --edition 2024 "${windows_rustflags[@]}" -D warnings \
  tools/windows/tpm/probe.rs -o build/windows/tpm/tpm-runtime.exe
```

Copy the executable through read-only driver media or the guest's normal network
path. Do not mount a live guest disk to stage it.

Run the parser fixtures on the build host with strict warnings:

```sh
rustc --edition 2024 --test -D warnings tools/windows/tpm/probe.rs \
  -o build/windows/tpm/tpm-parser-tests
build/windows/tpm/tpm-parser-tests
```

The fixtures reject truncated, oversized, wrong-tag, failed-command and trailing
responses, incorrect PCR selections, missing/nonpersistent handles and malformed
public-key/Name lengths. They also check that a changed PCR update counter
leaves the canonical digest unchanged.

Public API references:
[context creation](https://learn.microsoft.com/en-us/windows/win32/api/tbs/nf-tbs-tbsi_context_create),
[TPM2 context parameters](https://learn.microsoft.com/en-us/windows/win32/api/tbs/ns-tbs-tbs_context_params2)
and
[command submission](https://learn.microsoft.com/en-us/windows/win32/api/tbs/nf-tbs-tbsip_submit_command).
