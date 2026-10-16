# Standard Windows 11 platform

Standard Windows 11 Pro and Enterprise are in the acceptance scope. Support is
experimental. Historical Enterprise Evaluation results do not qualify this
rewritten source boundary or every edition. Do not bypass Setup's hardware
checks.

The opt-in platform adds two separate services:

```text
Windows / UEFI
    |
    +-- TPM CRB, 0xfed40000 --> Firecracker --> private socket --> swtpm
    |                                                        permanent state
    |
    +-- variable MMIO, 0xfed50000 --> copied MM request --> variable helper
                                                          auth + policy
                                                          persistent store
```

The normal console profile keeps its minimal device set. The secure profile
requires the TPM device, the protected variable service and the matching
firmware. The helper processes are started by the launcher, before Firecracker
connects. They receive no guest memory descriptor. Use a private directory and
one helper instance per VM. There is no network listener and no host-device
passthrough.

## Configuration

Add these fields to the ordinary AMD64 VM configuration:

```json
{
  "tpm": { "socket_path": "tpm-control.sock" },
  "uefi-vars": { "socket_path": "variables.sock" }
}
```

Use swtpm 0.10.2 with libtpms 0.10.x. The development container builds both from
pinned sources. Older distribution packages can lack checkpoint storage locking;
swtpm 0.10.2 also fixes a storage-header length check. These run as an external
native service; Firecracker does not link libtpms. See the
[swtpm release notes](https://github.com/stefanberger/swtpm/releases/tag/v0.10.2).

The TPM socket is swtpm's control socket, not its data channel. Firecracker
passes an owned socketpair endpoint for TPM commands before entering its
sandbox. Commands progress asynchronously and have a bounded deadline. The ACPI
TPM2 table selects CRB start method 7; `MSFT0101` describes locality zero.

The variable helper implements authenticated updates and firmware variable
policies in Rust. RustCrypto parses CMS/DER/X.509; the VMM's existing AWS-LC
backend verifies RSA signatures. The helper uses the repository's Apache-2.0
licence and runs as a separate process. Its build and operator instructions are
in [the helper package](../tools/uefi-vars-helper/README.md). Firmware
integration is in
[the secure profile](../resources/firmware/cloudhv-secureboot/README.md).

The transport copies a checked message of at most 2 MiB and freezes its return
address until completion. Guest RESET resets transport registers only. It cannot
clear authenticated variables, policies or boot lifecycle locks. A helper
failure stops the VM with an error instead of acknowledging an uncommitted
update.

## Checkpoints

Snapshot format 26.0.0 includes complete TPM and variable-service state,
physical-presence registers and the firmware profile/system UUID. Earlier
snapshots require the matching earlier VMM; there is no implicit conversion.
Display memory, TPM state and variables fit the bounded 32 MiB snapshot reader.
Guest disks remain separate artifacts. Snapshots contain secrets and require the
same access control as the VM's TPM store and guest disk.

A snapshot requires quiescent device commands. If a request is in flight, resume
the guest, let it complete, then pause and retry. Do not copy live helper files
as a substitute for the device checkpoint protocol.

Snapshot loading requires explicit destination services:

```json
{
  "snapshot_path": "vm.state",
  "mem_backend": { "backend_type": "File", "backend_path": "vm.memory" },
  "tpm": { "socket_path": "destination-tpm.sock" },
  "uefi_vars": {
    "socket_path": "destination-variables.sock",
    "expected_generation": 42
  }
}
```

`expected_generation` is the current destination store generation, supplied by
the launcher. The helper checks it and the store identity before replacement.
Supplying a service for a snapshot without that device is an error; omitting a
required service is also an error.

Prepare isolated destination stores and the matching paused disk state before
restore. Publish the restored VM only after all device imports succeed. If load
fails, discard the destination helpers and staging stores. A successful import
into one helper cannot make changes to separate disks and other helpers atomic.
The launcher owns that publication boundary.

An intentional restore also restores old revocation policy and TPM state. It
cannot preserve a later dbx update while restoring an older matched checkpoint.
A software TPM and host-managed variables do not protect against the host owner.

## Physical presence

The ACPI physical-presence interface uses a bounded mailbox in the existing
variable-service page and the standard EDK II physical-presence library.
Requests and results persist in `Tcg2PhysicalPresence`; firmware locks the
confirmation flags against OS writes after initialisation. The mandatory TPM2
operations are 0, 5, 14, 21 and 22. Firmware requires console confirmation
before executing destructive operations. A privileged guest can submit requests
and update runtime results; confirmation flags and TPM authorisation remain the
security boundary.

Malformed calls, helper failure, busy snapshots and request persistence have
host regressions. Disposable firmware tests retain an owner NV index after
refused clear and remove it after confirmed clear. Historical Windows Get-Tpm
and WMI transition results require their original binaries and matched guest
state.

References:

- [Cloud Hypervisor TPM interfaces](https://github.com/cloud-hypervisor/cloud-hypervisor/blob/c7dd43dbc3b625a7a21b9cb84d4a49b1663e902c/docs/tpm.md).
- [swtpm control protocol](https://github.com/stefanberger/swtpm/blob/v0.10.1/man/man3/swtpm_ioctls.pod).
- [TCG PC Client TPM profile](https://trustedcomputinggroup.org/wp-content/uploads/PC-Client-Specific-Platform-TPM-Profile-for-TPM-2p0-v1p05p_r14_pub.pdf).
- [EDK II variable protection](https://tianocore-docs.github.io/edk2-TrustedBootChain/release-1.00/3_TCG_Trusted_Boot_Chain_in_EDKII.html).
