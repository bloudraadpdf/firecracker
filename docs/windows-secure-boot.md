# Standard Windows 11 platform

Standard Windows 11 Pro and Enterprise are in the acceptance scope. Installation
and guest status checks are still in progress. The prior IoT Enterprise LTSC
result does not qualify these editions. Do not bypass Setup's hardware checks.

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
requires the TPM device, the protected variable service and the matching firmware.
The helper processes are started by the launcher, before Firecracker connects.
They receive no guest memory descriptor. Use a private directory and one helper
instance per VM. There is no network listener and no host-device passthrough.

## Configuration

Add these fields to the ordinary AMD64 VM configuration:

```json
{
  "tpm": { "socket_path": "tpm-control.sock" },
  "uefi-vars": { "socket_path": "variables.sock" }
}
```

The TPM socket is swtpm's control socket, not its data channel. Firecracker passes
an owned socketpair endpoint for TPM commands before entering its sandbox.
Commands progress asynchronously and have a bounded deadline. The ACPI TPM2
table selects CRB start method 7; `MSFT0101` describes locality zero.

The variable helper implements authenticated updates and firmware variable
policies using a pinned QEMU engine. Its source, patches, build and distribution
requirements are in [the helper package](../tools/uefi-vars-helper/README.md).
The separate helper is GPL-2.0-or-later; it is not linked into the Apache-licensed
VMM. Firmware integration is in
[the secure profile](../resources/firmware/cloudhv-secureboot/README.md).

The transport copies a checked message of at most 64 KiB and freezes its return
address until completion. Guest RESET resets transport registers only. It cannot
clear authenticated variables, policies or boot lifecycle locks. A helper failure
stops the VM with an error instead of acknowledging an uncommitted update.

## Checkpoints

Snapshot format 25.0.0 adds the complete TPM and variable-service state. Display
memory, TPM state and variables remain within the bounded 32 MiB snapshot reader.
Guest disks remain separate artifacts. Snapshots contain secrets and require the
same access control as the VM's TPM store and guest disk.

A snapshot requires quiescent device commands. If a request is in flight, resume
the guest, let it complete, then pause and retry. Do not copy live helper files as
a substitute for the device checkpoint protocol.

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

## Acceptance evidence

The TPM core has passed real swtpm startup, PCR extension, checkpoint export and
fresh-helper import tests. A Linux guest has passed discovery, identity and PCR
checks across cold boot, full restore and differential restore with the default
sandbox enabled, including the combined format-25 platform build.

The variable transport has focused malformed-frame, partial-response, deadline,
DMA-bound, frozen-destination and RESET-isolation tests. The helper has signed
update, tamper, replay and policy tests. A signed EFI probe has verified
`SecureBoot=1`, `SetupMode=0`, unsigned-image rejection, TPM discovery and its
event log. Unauthenticated PK deletion and writes to derived SecureBoot state
were rejected. Persistent variables survived a second cold boot and a full
restore into isolated stores. Disconnecting the helper caused the next variable
request to stop Firecracker with exit code 1.

The EFI probe runs before `ExitBootServices`. It does not establish Windows
runtime variable access after the OS switches the firmware's virtual addresses;
the installed-guest checks must establish that separately.

A deliberately failed restore advanced only its staging store; the original
store stayed unchanged. The failed staging copy was discarded. A rebased
differential restore also passed with separate stores, continued variable reads
and clean guest shutdown. Normal Windows 11 installation remains required before
declaring support complete.
