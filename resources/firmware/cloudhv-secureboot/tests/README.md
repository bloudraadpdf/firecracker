# Isolated Secure Boot probe

Use a new test directory and disposable keys. Never provision this policy into
a Windows or production store. The probe does not call ExitBootServices.

Start with the pinned, patched EDK2 tree described in the parent README.
Copy `SecureBootProbe.c` and `SecureBootProbe.inf` from this directory into
`MdeModulePkg/Application/HelloWorld/`. Create
`OvmfPkg/CloudHv/CloudHvSecureProbeX64.dsc` with:

```text
[Defines]
!include OvmfPkg/CloudHv/CloudHvConsoleSecureX64.dsc
[Components]
  MdeModulePkg/Application/HelloWorld/SecureBootProbe.inf
```

After EDK2 environment setup, build the application and firmware:

```sh
build -p OvmfPkg/CloudHv/CloudHvSecureProbeX64.dsc -a X64 -t GCC -b DEBUG -n 2
```

Copy `Build/CloudHvConsoleSecureX64/DEBUG_GCC/X64/SecureBootProbe.efi` to the
test directory as `unsigned.efi`. Use the corresponding `FV/CLOUDHV.fd` as
`firmware.fd`. Record both SHA-256 digests. Install OpenSSL, sbsigntool,
efitools, dosfstools, mtools and jq for the following preparation commands.
Run them inside the new test directory:

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
od -An -v -tu1 probe.esl | jq -s '.' >esl.json
jq -n --slurpfile esl esl.json '
  def variable($guid; $name):
    {guid:$guid,name:$name,data:$esl[0],digest:[],attributes:39,
     time:[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]};
  [97,223,228,139,202,147,210,17,170,13,0,224,152,3,43,140] as $global |
  [203,178,25,215,58,61,150,69,163,188,218,208,14,103,101,111] as $image |
  {flags:0,policies:[],variables:[
    variable($global;[80,0,75,0,0,0]),
    variable($global;[75,0,69,0,75,0,0,0]),
    variable($image;[100,0,98,0,0,0])]} ' >seed.json
firecracker-uefi-vars provision store.json seed.json
mkdir -m 700 tpm
```

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

Start the two helpers in separate terminals in the same directory, then the
VMM. Do not send TPM2_Startup from a host tool; firmware owns it.

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

The signed probe must report SecureBoot=1, SetupMode=0, unsigned PK deletion
`800000000000001a` (Security Violation), derived-state write
`8000000000000008` (Write Protected), unsigned-image load
`800000000000000f` (Access Denied), TPM present=1 and event-log present=1.
The nonvolatile counter increases and the guest shuts down through ACPI S5
after 30 seconds. Repeat with the same disk and stopped helper stores: the
counter must increase again. Stop and reap both helpers after each VM exit.

For full/differential restore, pause during the 30-second heartbeat and use
the matched staging procedure in [the Secure Boot guide](../../../../docs/windows-secure-boot.md).
After resume, heartbeat reads and S5 must succeed. For a failure test, stop
only the variable helper during the heartbeat: the next read must stop the
VMM with GenericError exit 1. Remove snapshot RAM and disposable private keys
after preserving the logs and public digests.
