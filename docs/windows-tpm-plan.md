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
- [ ] Verify UEFI TPM measurement and normal Windows 11 installation.
- [x] Verify persistent TPM identity, PCRs, full/differential restore and cold boot in Linux.
- [ ] Repeat the persistence checks with the Windows firmware path.
- [ ] Run default sandbox, headless regressions and final duplication checks.

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

A combined `MicrovmState` serialization/reader regression uses 16 MiB of varied
VRAM, a valid 64 KiB VGA ROM, the aggregate 8 MiB TPM blob limit, the 3968-byte
CRB buffer, the maximum 4 MiB-minus-256 variable checkpoint and 32 default vCPU
records. Its encoded snapshot is 29,473,960 bytes and passes the 32 MiB reader
limit. This measures the optional buffer boundary, not a universal upper bound
on host-dependent vCPU state or every possible virtio configuration.
