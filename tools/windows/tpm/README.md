# Windows TPM runtime probe

`probe.rs` uses the public Windows TPM Base Services API to create a TPM2-only
context, submit TPM2_GetRandom for 16 bytes, and close the context. It checks the
TBS status, TPM response header and returned length. It prints status and length
only, never random bytes. It does not clear the TPM or change its persistent
objects, ownership, PCRs or disk protectors.

This probe establishes real Windows-to-TPM command transport. It does not replace
Get-Tpm, physical-presence, Secure Boot, measured-boot or snapshot identity checks.
Run it from the installed guest after cold boot, full restore and differential
restore. Require exit zero and `PASS Windows TBS TPM2_GetRandom` each time.

Build for AMD64 using the selected Rust toolchain and its matching rust-lld:

```sh
mise exec -- rust-lld -flavor link /lib /machine:x64 \
  /def:../legacy-vsock/msvcrt.def /out:msvcrt.lib
mise exec -- rustc --edition 2024 --target x86_64-pc-windows-msvc \
  -C panic=abort -C opt-level=2 -C lto=fat -C linker=rust-lld \
  -C link-arg=/entry:mainCRTStartup -C link-arg=/subsystem:console,6.01 \
  -C link-arg=/nodefaultlib -C link-arg=msvcrt.lib -D warnings \
  probe.rs -o tpm-runtime.exe
```

Use the actual rust-lld path from that toolchain if it is not on PATH. Copy the
executable through read-only driver media or the guest's normal network path.
Do not mount a live guest disk to stage it.

Public API references: [context creation](https://learn.microsoft.com/en-us/windows/win32/api/tbs/nf-tbs-tbsi_context_create),
[TPM2 context parameters](https://learn.microsoft.com/en-us/windows/win32/api/tbs/ns-tbs-tbs_context_params2)
and [command submission](https://learn.microsoft.com/en-us/windows/win32/api/tbs/nf-tbs-tbsip_submit_command).
