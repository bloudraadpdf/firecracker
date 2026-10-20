# Actions API Request

Firecracker microVMs can execute actions that can be triggered via `PUT`
requests on the `/actions` resource.

Details about the required fields can be found in the
[swagger definition](../../src/firecracker/swagger/firecracker.yaml).

## InstanceStart

The `InstanceStart` action powers on the microVM and starts the guest OS. It
does not have a payload. It can only be successfully called once.

### InstanceStart Example

```bash
curl --unix-socket ${socket} -i \
     -X PUT "http://localhost/actions" \
     -d '{ "action_type": "InstanceStart" }'
```

## FlushMetrics

The `FlushMetrics` action flushes the metrics on user demand.

### FlushMetrics Example

```bash
curl --unix-socket /tmp/firecracker.socket -i \
    -X PUT "http://localhost/actions" \
    -d '{ "action_type": "FlushMetrics" }'
```

## [Intel and AMD only] SendCtrlAltDel

This action will send the CTRL+ALT+DEL key sequence to the microVM. By
convention, this sequence has been used to trigger a soft reboot and, as such,
most Linux distributions perform an orderly shutdown and reset upon receiving
this keyboard input. Since Firecracker exits on CPU reset, `SendCtrlAltDel` can
be used to trigger a clean shutdown of the microVM.

For this action, Firecracker emulates a standard AT keyboard, connected via an
i8042 controller. Driver support for both these devices needs to be present in
the guest OS. For Linux, that means the guest kernel needs `CONFIG_SERIO_I8042`
and `CONFIG_KEYBOARD_ATKBD`.

> [!NOTE]
>
> At boot time, the Linux driver for i8042 spends a few tens of milliseconds
> probing the device. This can be disabled by using these kernel command line
> parameters:
>
> ```console
> i8042.noaux i8042.nomux i8042.nopnp i8042.dumbkbd
> ```

The [AMD64 PC lifecycle contract](../windows-console.md#guest-lifecycle)
reports guest reboot separately from normal shutdown.

### SendCtrlAltDel Example

```bash
curl --unix-socket /tmp/firecracker.socket -i \
    -X PUT "http://localhost/actions" \
    -d '{ "action_type": "SendCtrlAltDel" }'
```

## [Intel and AMD, Windows feature only] SendPowerButton

This action signals the fixed ACPI power button (PM1 status bit 8) after the
microVM has booted. It requires an x86_64 build with the `windows` feature and
an attached ACPI power device. Other builds and architectures return HTTP 400
Bad Request.

The guest chooses how to handle the event under its power-button policy. The
action does not inject keyboard input or force the VMM to stop. A successful
API response confirms the button event was signalled, not that guest shutdown
has completed. Observe guest shutdown and VMM exit separately.

### SendPowerButton Example

```bash
curl --unix-socket /tmp/firecracker.socket -i \
    -X PUT "http://localhost/actions" \
    -d '{ "action_type": "SendPowerButton" }'
```
