# Initial Windows patch review

This records the review before graphical installation and the secure platform
were added. The [final acceptance plan](windows-acceptance-plan.md) tracks the
current format-27 checks and remaining work.

Base: `658ff27eb0c8c60f1565f5bc197120cdd00a27f6`.
Validated implementation before review: `3c0b832c2`.

- [x] Inventory the complete diff and record a duplication baseline.
- [x] Review configuration, boot, CPU state and snapshot compatibility.
- [x] Review ACPI, clocks, interrupt routing and device reset lifecycles.
- [x] Review virtio queue, network, block and vsock bounds and ownership.
- [x] Review preparation, driver packaging and validation tools for reliable
  failures, cleanup and reproducibility.
- [x] Fix each confirmed defect with a focused reproducer and validation.
- [x] Refactor repeated logic and split unrelated responsibilities where the
  review establishes a concrete benefit.
- [x] Reconcile user documentation with the final behaviour and separate the
  historical investigation record from current instructions.
- [x] Run native tests and cross-architecture checks appropriate to the changes.
  Repeat guest tests where behaviour changes invalidate earlier evidence.
- [x] Produce a coherent patch series with logical boundaries and record its
  base, dependencies, checks and remaining limits.
- [x] Check the final diff, duplication, processes and disposable artefacts.

The initial scan covers VMM, ACPI, Firecracker and Windows tools: 151 clones,
2,955 duplicated lines. Generated source and patch files are excluded.
The existing investigation record and verified binaries remain available
while this review proceeds.

## Review results

The review covered configuration and snapshot state, firmware handoff,
ACPI and RTC state, shared INTx and MSI-X reset, virtio buffer ownership and
bounds, seccomp rules, and the guest preparation and validation tools. No
production VMM changes were needed in this pass. The verified AMD64 and
ARM64 binary hashes remain unchanged.

Confirmed tooling defects and refinements:

- The CONNECT controller waited indefinitely for a stalled child. A minimal
  fake-proxy reproducer reached the outer timeout before the fix. The bounded
  cleanup now terminates that run with failure and removes the proxy process
  group. API requests also have time limits.
- Host-specific user/group IDs were replaced with the invoking identity;
  an explicit identity switch initialises the selected user's groups.
- The proxy helper existed only in ignored build output. Its source is now
  tracked, formatted, and documented against a pinned Tokoloshe revision.
  Argument validation and bounded echo writes make its failure paths explicit.
- The cumulative driver diff is split into six ordered patches: four
  attributed upstream fixes, DMA completion ordering, and XP build support.
  Applying the series reconstructs the exact tested driver source.
- The preparation guide's stale ARM activation statement is corrected. The
  long implementation record is labelled as historical and links to the
  current guide.

## Verification

- Native VMM: 879 tests passed serially with the required KVM/TAP privileges.
- ACPI tables: 26 tests passed.
- AMD64 and ARM64: VMM, Firecracker and ACPI all-target Clippy checks passed
  with GDB enabled and warnings denied.
- Repository formatting passed with the pinned stable Rust 1.98.1 formatter;
  shell syntax and ShellCheck warning checks passed for the controller.
- The tracked proxy helper built against the audited Tokoloshe source and
  passed three 2 MiB bidirectional CONNECT exchanges and three denials.
  Exactly six accounting entries were sealed; the helper and sidecar exited.
- The original guest boot/restore/TAP/vsock evidence remains valid: no
  production VMM, firmware or guest driver code changed in this review.
- Duplication remains at 151 clones and 2,955 lines, with no new code clones.

The first native run used parallel tests without TAP privileges; its metric
and permission failures disappeared when run serially with those privileges.
Logs and focused reproducers are in `build/windows/review/`.

## Patch series

The development history is retained under
`archive/windows-development-20261014`. The review series preserves the
ordered implementation changes and consolidates investigation-only commits
into final documentation. Its final tree is checked against the reviewed
development tree. Generated patches, the commit mapping and application
verification are retained under `build/windows/review/series/`. No merge or
push is performed.
