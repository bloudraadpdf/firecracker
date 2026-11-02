# Isolated Secure Boot probe

Use a new test directory and disposable keys. Never provision this policy into a
Windows or production store. The probe does not call ExitBootServices.

Start with the [console checkout](../../cloudhv-console/README.md#build) and
apply the secure firmware patches described in the parent README. The probe
application and build profile are supplied as a patch to that external tree.
From the Firecracker repository root, apply it once:

```sh
git -C build/windows/console/edk2 apply --ignore-whitespace \
  "$PWD/resources/firmware/cloudhv-secureboot/tests/firmware-tests.patch"
```

After EDK II environment setup, build the application and firmware from that
checkout:

```sh
build -p OvmfPkg/CloudHv/Tests/SecureBootProbe.dsc -a X64 -t GCC -b DEBUG -n 2
```

Copy `Build/CloudHvConsoleSecureX64/DEBUG_GCC/X64/SecureBootProbe.efi` to the
test directory as `unsigned.efi`. Use the corresponding `FV/CLOUDHV.fd` as
`firmware.fd`. Record both SHA-256 digests. Install OpenSSL, sbsigntool,
efitools, dosfstools, mtools, Python 3 and jq for the following preparation
commands. Run them inside the new test directory:

```sh
umask 077
openssl req -new -x509 -newkey rsa:2048 -nodes -sha256 -days 7 \
  -subj /CN=Firecracker-Test-Only -keyout probe.key -out probe.pem
cert-to-efi-sig-list -g cb36c34d-ea2c-4d88-83d5-259cfa6b4911 probe.pem probe.esl
sbsign --key probe.key --cert probe.pem --output signed.efi unsigned.efi
sbverify --cert probe.pem signed.efi
truncate -s 64M esp.raw
mkfs.vfat esp.raw
mmd -i esp.raw ::/EFI ::/EFI/BOOT
mcopy -i esp.raw signed.efi ::/EFI/BOOT/BOOTX64.EFI
mcopy -i esp.raw unsigned.efi ::/EFI/unsigned.efi
python3 - <<'PYTHON'
import hashlib
import pathlib
import struct

data = bytes.fromhex("2616c4c14c509240aca941f936934328")
data += struct.pack("<III", 67228, 0, 48)
data += b"".join(bytes(16) + struct.pack("<I", index) + bytes(28)
                 for index in range(1400))
assert len(data) == 67228
assert hashlib.sha256(data).hexdigest() == (
    "60facbb0d32aba50aae0c5e671a3d874df0cb92bd7159ed0d1c4d64ee32abee0"
)
pathlib.Path("dbx.esl").write_bytes(data)
PYTHON
od -An -v -tu1 probe.esl | jq -s '.' >esl.json
od -An -v -tu1 dbx.esl | jq -s '.' >dbx.json
jq -n --slurpfile esl esl.json --slurpfile dbx dbx.json '
  def variable($guid; $name; $data):
    {guid:$guid,name:$name,data:$data,digest:[],attributes:39,
     time:[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]};
  [97,223,228,139,202,147,210,17,170,13,0,224,152,3,43,140] as $global |
  [203,178,25,215,58,61,150,69,163,188,218,208,14,103,101,111] as $image |
  {flags:0,policies:[],variables:[
    variable($global;[80,0,75,0,0,0];$esl[0]),
    variable($global;[75,0,69,0,75,0,0,0];$esl[0]),
    variable($image;[100,0,98,0,0,0];$esl[0]),
    variable($image;[100,0,98,0,120,0,0,0];$dbx[0])]} ' >seed.json
firecracker-uefi-vars provision store.json seed.json
mkdir -m 700 tpm
```

The 67,228-byte dbx is the deterministic synthetic `EFI_CERT_SHA256_GUID`
signature list used by the helper's large-database regression. It contains 1,400
entries, with a zero owner GUID and an index in the first four hash bytes. It is
a transport fixture, not a Microsoft revocation policy. The maintained
`seed-microsoft` encoder produces a 23,103-byte dbx for its pinned inputs,
SHA-256 `8c88c19dc4d7d7213362d8e9e2e4ffe9d312a79e01bb701a568405c9dd30194f`; that
policy cannot exercise the 64 KiB boundary. Do not substitute it for this
fixture or deploy the synthetic list as a trust policy. The signed probe remains
trusted by the disposable certificate in db; its hash is not a synthetic
revocation entry.

The test profile also embeds a small DXE observer. It waits for both variable
architectural protocols and calls the same `LargeDbx` check as the signed
application. It reports through serial before consoles exist. It does not change
image verification, enrolment or Secure Boot policy. This observer is present
only when `SecureBootProbe.dsc` sets `FIRECRACKER_VARIABLE_PROBE`.

Use this VM configuration, replacing `vgabios.bin` with the console ROM path:

```json
{
  "boot-source": {"kernel_image_path":"firmware.fd","boot_args":""},
  "machine-config": {"vcpu_count":1,"mem_size_mib":512,"track_dirty_pages":true},
  "console": {"socket_path":"console.sock","vga_rom_path":"vgabios.bin"},
  "drives": [{"drive_id":"probe","path_on_host":"esp.raw","is_root_device":true,"is_read_only":true}],
  "tpm": {"socket_path":"tpm/ctrl"},
  "uefi-vars": {"socket_path":"vars.sock"}
}
```

Start the two helpers in separate terminals in the same directory, then the VMM.
Do not send TPM2_Startup from a host tool; firmware owns it.

```sh
timeout 120 firecracker-uefi-vars serve store.json vars.sock
```

```sh
timeout 120 swtpm socket --tpm2 --tpmstate dir=tpm \
  --ctrl type=unixio,path=tpm/ctrl --flags not-need-init,disable-auto-shutdown
```

```sh
timeout 120 firecracker --enable-pci --api-sock api.sock \
  --config-file config.json >serial.log 2>&1
```

The observer (`dbx-observer=1`) and signed application (`dbx-application=1`)
must each report the following values; sizes and statuses are hexadecimal:

```text
dbx-query-status=8000000000000005
dbx-query-size=1069C
dbx-read-status=0
dbx-read-size=1069C
dbx-attributes=27
dbx-sha256=60facbb0d32aba50aae0c5e671a3d874df0cb92bd7159ed0d1c4d64ee32abee0
dbx-bytes-match=1
dbx-sha256-match=1
dbx-valid=1
```

The check calls `gRT->GetVariable` first with a zero-sized NULL buffer, then
reads the complete dbx and compares both its bytes and SHA-256 digest. A direct
MM request does not exercise this path. The signed application must also pass
all checks below.

The signed probe must report SecureBoot=1, SetupMode=0, unsigned PK deletion
`800000000000001a` (Security Violation), derived-state write `8000000000000008`
(Write Protected), unsigned-image load `800000000000000f` (Access Denied), TPM
present=1 and event-log present=1. The nonvolatile counter increases and the
guest shuts down through ACPI S5 after 30 seconds. Repeat with the same disk and
stopped helper stores: the counter must increase again. Stop and reap both
helpers after each VM exit.

For full/differential restore, pause during the 30-second heartbeat and use the
matched staging procedure in
[the Secure Boot guide](../../../../docs/windows-secure-boot.md). After resume,
heartbeat reads and S5 must succeed. For a failure test, stop only the variable
helper during the heartbeat: the next read must stop the VMM with GenericError
exit 1. Remove snapshot RAM and disposable private keys after preserving the
logs and public digests.

## Transport regression control

Use two immutable firmware outputs from the same external test tree and the same
signed application, disk and synthetic seed. Build the test profile first with
the pinned header's 64 KiB `MAX_BUFFER_SIZE`. If the current secure patch is
already applied, reverse only its header hunk from the Firecracker root:

```sh
git -C build/windows/console/edk2 apply -R --ignore-whitespace \
  --include=OvmfPkg/VirtMmCommunicationDxe/VirtMmCommunication.h \
  "$PWD/resources/firmware/cloudhv-secureboot/cloudhv-secureboot.patch"
```

Build `SecureBootProbe.dsc` with the command above. Preserve that firmware and
its source digest. Keep the VMM, helper, signed application, seed and disk
identical for both controls; provision separate fresh helper and TPM stores. The
observer must report the required size `1069C` after the zero-buffer query. In
the pinned DEBUG build, the subsequent oversized read hits the variable driver's
communication-error assertion and does not return. After the external time
limit, stop and reap the VMM and both helpers. Record this as a read that did
not return, not as a returned EFI error or `dbx-valid=0`. Do not disable the
assertion or bypass image verification to launch the application.

For assertion diagnostics, rebuild with the existing EDK II flag
`-D DEBUG_ON_SERIAL_PORT`. Preserve this as a separate firmware artifact; use
the original old/new outputs for the header-only comparison.

Change only `OvmfPkg/VirtMmCommunicationDxe/VirtMmCommunication.h` to the 2 MiB
limit supplied by the secure firmware patch:

```sh
git -C build/windows/console/edk2 apply --ignore-whitespace \
  --include=OvmfPkg/VirtMmCommunicationDxe/VirtMmCommunication.h \
  "$PWD/resources/firmware/cloudhv-secureboot/cloudhv-secureboot.patch"
```

Rebuild the same test profile and provision a fresh disposable store from the
same seed. The observer and signed application must both pass the complete read
checks. Preserve old/new firmware, application, seed and dbx hashes plus serial
logs. The build alone is not a passing result.

Both observer and application run before ExitBootServices. Neither result
qualifies Windows runtime-variable calls after ExitBootServices and virtual
address conversion; those require the separate Windows runtime probe.

## Physical presence

Use the same disposable directory and the physical-presence firmware patch.
Never submit a clear against an installed guest's TPM. Build the host submission
client with `rustc --edition=2024 ppi-submit.rs -o ppi-submit`.

After a clean probe shutdown, start the variable helper alone. Run
`./ppi-submit vars.sock 5`, then wait for the helper to exit. This sends cold
initialisation followed by PPI function 2 through the normal helper interface.
Start both helpers and the VM again. The firmware must show the clear warning
before launching the EFI application.

Press Escape to reject the request. The application must report:

```text
ppi-flags=700E2
ppi-flags-write=8000000000000008
ppi-request=0
ppi-last=5
ppi-response=FFFFFFF0
```

Submit operation 5 again and press F12 at the prompt. The VMM exits on the
firmware reset. Restart with the same closed TPM and variable stores. The
application must report pending request 0, last request 5 and response 0, then
shut down. The TPM remains usable after the clear.

For the combined-operation regression, provision another disposable store with
`Tcg2PhysicalPresenceFlags=0x10` and request 14. Use the GUID
`aeb9c5c1-94f1-4d02-bfd9-4602db2d3c54`, attributes 7, a four-byte little-endian
flags value and the 16-byte request structure documented in the helper README.
The clear bit is absent; the turn-on bit is present. A confirmation prompt is
still required by TCG PPI 1.30 Table 2. Escape must produce last request 14 and
response `0xfffffff0`. This catches the pinned EDK2 library's clear-only
confirmation test.

A separately provisioned unsupported request 17 must be consumed with BIOS
failure `0xfffffff1`, without a confirmation prompt or TPM action. Provisioning
these fixtures is a host test operation; do not edit a live store or expose
flag-changing operations through the guest interface.

The signed probe also defines owner-authorised NV index `0x01500020` once in its
disposable TPM. A separate nonvolatile UEFI marker prevents recreation. Before
clear and after rejected clear, `nv-read-public` must be 0. After accepted clear
it must be `18B` (`TPM_RC_HANDLE` for the first handle). This verifies the TPM
effect independently of the writable PPI result variable. Never run this probe
against an installed or production TPM.
