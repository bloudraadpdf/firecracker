# Optional AMD64 TPM 2.0 implementation plan

The standard Windows 11 acceptance path requires a virtual TPM and Secure Boot.
The existing IoT Enterprise LTSC result does not establish that support.

## Interfaces and ownership

- Root owns `TpmConfig`, device attachment, ACPI tables, API and snapshot plumbing.
- Display agent owns `devices/tpm/`, the swtpm client, bounded CRB core and tests.
- Optical agent owns EDK II TPM measurement, Secure Boot and variable persistence.
- The supervisor owns one private swtpm process and state directory per VM.
  Firecracker connects to it before the sandbox; it does not launch helpers.

Configuration: optional `tpm: {"socket_path": "/private/vm/tpm.sock"}`. The socket
is swtpm's control socket. A Unix socketpair supplies the TPM data channel through
`CMD_SET_DATAFD`; descriptors remain owned Rust values. Neither socket is exposed
to the guest or a network listener.

The guest interface follows Cloud Hypervisor's TPM CRB layout:

| Interface | Value |
|---|---|
| MMIO base and size | `0xfed40000`, `0x1000` |
| Supported locality | 0 |
| Shared command/response buffer | `0xfed40080`, 3968 bytes |
| ACPI device ID | `MSFT0101` |
| ACPI TPM2 table | Revision 3, control area `0xfed40040`, start method 7 |

Buffer addresses remain fixed. Commands use the TPM header's big-endian length;
repeated or out-of-order MMIO writes do not accumulate an artificial length.
Invalid access widths, offsets, command lengths and backend replies fail closed.
The data socket is nonblocking. An explicit sending/receiving state machine
advances on an event-loop timer and has a bounded deadline. This permits cancellation
and prevents a TPM operation from blocking a vCPU inside a socket read.

## Sequence

- [x] Inspect Cloud Hypervisor CRB, swtpm client and ACPI implementations.
- [x] Inspect the upstream swtpm control protocol and state transfer commands.
- [x] Agree interfaces with platform and firmware owners.
- [x] Add bounded control-channel codec and real swtpm protocol tests.
- [x] Implement CRB registers, locality and request/response tests.
- [x] Complete cancellation tests.
- [x] Add malformed access, partial stream, timeout and disconnected-backend tests.
- [x] Add state export/import and tampered-state tests.
- [x] Reject snapshots with interrupted commands; finish through timer progress.
- [x] Integrate optional configuration, reserved MMIO and ACPI discovery.
- [x] Verify Linux TPM discovery and commands against an actual swtpm instance.
- [x] Verify UEFI TPM measurement and normal Windows 11 installation.
- [x] Verify persistent TPM identity, PCRs, full/differential restore and cold boot in Linux.
- [x] Repeat the persistence checks with the Windows firmware path.
- [x] Read SHA-256 PCR 7 and an existing persistent TPM object's public identity
      through TBS in Windows. Require the same values after full and differential
      restore; missing identity or blocked commands leave this gate open. Do not
      create keys, extend PCRs or change BitLocker protectors for this check.
- [x] Run default sandbox, headless regressions and final duplication checks.

## Persistence and failure boundaries

Cold boot initialises the backend with stale volatile state deleted, preserving
permanent TPM state. Firmware performs `TPM2_Startup`; the host must not issue a
second startup or clear PCRs behind the firmware.

A snapshot first pauses vCPUs, rejects a pending command so the caller can retry after it completes.
Permanent, volatile and savestate blobs are captured from the running backend
with their transfer flags. The volatile export stores live state internally. Their combined size is bounded at
8 MiB before allocation. The same VMM snapshot contains the CRB registers and
buffers. The source backend storage lock is reacquired after savestate export.

Restore requires a fresh dedicated swtpm instance. Transfer all saved blobs into
its stopped backend, initialise without clearing volatile state, validate CRB
state, then publish the device and resume the VM. A failed transfer aborts
restore; the supervisor discards that helper rather than using partially restored
state. Full and differential snapshots both contain complete TPM device state.
Guest disk, firmware variable store and TPM state must represent the same pause
point. A host-level copy of live swtpm files is not a valid checkpoint.

TPM state includes secrets. Keep socket directories, state and snapshot artifacts
private. Host software TPM emulation provides isolation through host permissions;
it does not provide a hardware root of trust or protection from the host owner.
Migration encryption keys, if used, belong to the supervisor and must not be
embedded in the VM configuration or snapshot.

## Provenance and references

Cloud Hypervisor source revision
`c7dd43dbc3b625a7a21b9cb84d4a49b1663e902c`, `devices/src/tpm.rs`, `tpm/src/`
and the TPM2 ACPI table use Apache-2.0 with Microsoft copyright. Preserve those
notices for adapted code and describe substantive changes. Its existing client
has no complete snapshot transfer path and uses synchronous command I/O, so it
cannot be copied unchanged.

- [Cloud Hypervisor TPM documentation](https://github.com/cloud-hypervisor/cloud-hypervisor/blob/c7dd43dbc3b625a7a21b9cb84d4a49b1663e902c/docs/tpm.md)
- [swtpm control protocol](https://github.com/stefanberger/swtpm/blob/v0.10.1/man/man3/swtpm_ioctls.pod)
- [TCG PC Client TPM profile](https://trustedcomputinggroup.org/wp-content/uploads/PC-Client-Specific-Platform-TPM-Profile-for-TPM-2p0-v1p05p_r14_pub.pdf)
- [Microsoft Windows 11 requirements](https://learn.microsoft.com/en-us/windows/whats-new/windows-11-requirements)

## Core validation

Native tests use swtpm 0.10.1 and libtpms 0.10.2. Eleven tests pass, including an
actual TPM2 startup, SHA-256 PCR extension, state export, fresh-helper import and
matching PCR reads on both source and destination. Malformed blob headers are
rejected before allocation. The backend rejects non-2.0 implementations through
bounded `CMD_GET_INFO`. Debug output excludes command, response and state bytes.

The real-backend test needs `swtpm` on PATH. Run:

```sh
cargo test -p vmm devices::tpm:: --lib -- --include-ignored
```

Each fixture owns and stops its helper, then removes its private state directory.

Linux integration passed with `firecracker-tpm24b` under the default sandbox:
ACPI `MSFT0101` and TPM2 discovery, the kernel CRB driver, RSA primary creation,
PCR extension, full and differential restore into fresh helpers, and cold restart
of the restored state. The primary public key matched the original. Guest memory
checkpoints were removed after validation.

The combined format-25 binary `firecracker-secure25-first` repeated these checks
under the default sandbox (SHA-256
`94f80c2a1e621ef2fbbabee70f65920944203d3bd651f414335ba8333cacd469`).
The real Linux CRB driver discovered `MSFT0101`, validated the TPM2 control
address/start method, and created an RSA primary and extended PCR 0. Full and
differential restores into fresh helpers preserved the PCR and continued the
guest loop. A cold boot of the restored permanent state produced the original
primary public key. A snapshot taken during a command was rejected explicitly;
resuming, allowing completion and retrying succeeded. Logs are retained in
`build/windows/console/tpm-linux/{cold25,full25,diff25,reboot25}.log`; temporary
helpers, RAM checkpoints and the Linux fixture were removed. This validates
Linux TPM persistence, not Windows measured boot or Secure Boot acceptance.

The shared Unix connector rejects a full listener backlog without blocking.
Its isolated regression checks that repeated failed connections leak no file
descriptors. CRB cancellation retains its register value until software clears
it, as required by TCG PTP 1.05 Table 33.

The format-25 combined `MicrovmState` serialization/reader regression used 16 MiB of varied
VRAM, a valid 64 KiB VGA ROM, the aggregate 8 MiB TPM blob limit, the 3968-byte
CRB buffer, the maximum 4 MiB-minus-256 variable checkpoint and 32 default vCPU
records. Its encoded snapshot was 29,473,960 bytes and passed the 32 MiB reader
limit. This measures the optional buffer boundary, not a universal upper bound
on host-dependent vCPU state or every possible virtio configuration.

## Windows physical-presence completion

The initial installed standard Windows 11 guest passed Secure Boot and a public
TBS TPM2_GetRandom command, but Get-Tpm failed with `0x80284001` when it queried
the missing physical-presence interface. The format-26 implementation resolves
that error: Get-Tpm and the WMI transition query pass on the installed guest.
Windows enabled BitLocker; preserve that guest's TPM and encrypted disk.

The TCG PPI 1.30 mandatory TPM2 operations are 0, 5, 14, 21 and 22. A permanent
reject-all interface does not meet this requirement. Requests and results use
EDK II's persistent Tcg2PhysicalPresence variable. Firmware confirms destructive
operations through its console before execution. The protected flags variable
must remain immutable to the OS after firmware initialisation.

- [x] Isolate the failure with public Windows TPM APIs and WMI.
- [x] Check mandatory operations against TCG PPI 1.30 Table 2.
- [x] Add bounded helper opcode 5 for requests, results and supported operations.
- [x] Add a serialised ACPI _DSM and bounded mailbox in the existing variable page.
- [x] Bind the standard EDK II physical-presence library in secure firmware.
- [x] Verify flag initialisation and locking on every firmware return path.
- [x] Verify malformed calls, helper failure, pending-command snapshot rejection
      and request persistence in host regression tests.
- [x] Verify full/differential restore with the final Windows firmware path.
- [x] Confirm and reject clear operations on a disposable TPM through firmware.
- [x] Repeat Get-Tpm and secure cold/full/differential Windows acceptance.
- [x] Complete secure TAP and CONNECT checks with matching encrypted-disk state.

Helper opcode 5 uses four little-endian u32 request fields: function, revision,
operation and argument. Its response has three u32 fields: PPI return code and
two function-specific values. This uses the existing framed connection and
persistent store; it does not add a guest-accessible host management interface.

The format-26 transport passes 13 focused tests, including concurrent DMA/PPI
requests in both orders, frozen request data, malformed access/replies, helper
failure and snapshots of idle registers rewritten after a previous reply. The
AML tests include 14 ACPICA execution cases for revision negotiation, invalid
arguments, bounded timeout and response packages. Strict Clippy passes on AMD64
and ARM64. The combined maximum-buffer snapshot fixture now encodes 29,473,976
bytes and passes the 32 MiB reader limit. These host checks do not replace the
remaining Windows and firmware acceptance steps.

Opcode 5 preserves the previous result when it submits a request. The standard
runtime request/result variable remains writable by a privileged guest; this is
not global result immutability. Confirmation flags and actual TPM authorisation
are the security boundary. Check destructive effects against the TPM itself,
not only the reported PPI response. The disposable clear test must retain an
owner-created NV index after rejection and remove it after accepted clear.
See [EDK2's variable protection model](https://tianocore-docs.github.io/edk2-TrustedBootChain/release-1.00/3_TCG_Trusted_Boot_Chain_in_EDKII.html)
and TCG PPI 1.30 section 8.1.5.

The corrected secure firmware passes format-27 standard Windows 11 TAP and
CONNECT checks after cold boot, full restore and differential restore. Every phase reports
Secure Boot enabled, a present/ready/enabled/activated TPM and successful TBS
GetRandom. Independent offline checks confirm identical SHA-256 PCR 7 and
persistent-object handle, public area and Name across all three phases. The
existing object is read, not provisioned. These checks do not prove possession
of its private key or TPM-sealed BitLocker recovery.

The complete public baseline also matches across the intervening cold boot.
Both paths pass read-only encrypted-disk data checks and clean S5 shutdown.
