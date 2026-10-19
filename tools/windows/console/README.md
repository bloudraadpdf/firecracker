# Firecracker remote console bridge

This Linux host process converts the local Firecracker console protocol into RFB
3.3, 3.7 or 3.8. It runs outside the VMM sandbox. Guest networking and vsock are
not used.

```text
noVNC browser -> authenticated HTTPS gateway -> loopback WebSocket bridge
                                                        |
Native VNC viewer -> SSH tunnel -> loopback RFB bridge --+
                                                        |
                                            Firecracker Unix socket
```

## Build and check

Follow the [Rust build prerequisites](../README.md#build-prerequisites). From
the full repository checkout on the Linux VM host:

```sh
cargo build --locked --manifest-path tools/windows/console/Cargo.toml
cargo test --locked -p console
cargo clippy --locked -p console --all-targets -- -D warnings
cargo clippy --locked --manifest-path tools/windows/console/Cargo.toml --all-targets -- -D warnings
```

The shared library is `src/console`; its unit tests run in the root workspace.
Both workspaces retain their lock files. `Cargo.lock` pins the bridge dependencies. The WebSocket implementation uses
`tungstenite` 0.30.0. RFB follows
[RFC 6143](https://www.rfc-editor.org/rfc/rfc6143).

## Native VNC viewer

The password and token examples use OpenSSL. On the VM host:

```sh
build/cargo_target/debug/firecracker-console /run/vm/console.sock rfb 127.0.0.1:5900
```

In a separate terminal on the local computer, keep this tunnel running:

```sh
ssh -N -T -o ExitOnForwardFailure=yes \
  -L 127.0.0.1:5900:127.0.0.1:5900 user@vm-host
```

Connect the viewer to `127.0.0.1:5900` on the local computer. Without a password
file, RFB uses security type None. macOS Screen Sharing requires VNC password
authentication. Before starting its bridge, create the private password file on
the VM host. The socket's private parent directory must already exist. This
command refuses an existing password file:

```sh
(umask 077; set -C; openssl rand -hex 4 > /run/vm/vnc.password)
```

Read that file on the host to obtain the eight-character password. Start the
macOS bridge instead of the unauthenticated bridge:

```sh
build/cargo_target/debug/firecracker-console /run/vm/console.sock rfb-macos 127.0.0.1:5900 /run/vm/vnc.password
```

The file must contain 1 to 8 printable ASCII characters, optionally followed by
a newline. On the local Mac, run `open vnc://127.0.0.1:5900` and enter that
password in Screen Sharing. Mode `rfb-macos` maps Command to the Windows key and
retains Option as Alt. Other clients use mode `rfb` for standard RFB key
mappings; that mode also accepts the optional password file. The browser retains
standard RFB key mappings. VNC authentication is a legacy DES challenge-response
protocol, not transport encryption; retain the SSH tunnel. The bridge rejects
non-loopback bind addresses. Authenticated sessions remain connected while idle;
the 30-second read deadline applies to the handshake only.

## Browser console

Stop any existing RFB bridge for this VM before starting its WebSocket bridge.
Create a private token file containing 32 to 128 random ASCII letters, digits,
`-` or `_`. For example, on the VM host:

```sh
(umask 077; set -C; openssl rand -hex 32 > /run/vm/console.token)
```

Do not put the token in the bridge command line. Start the bridge:

```sh
build/cargo_target/debug/firecracker-console /run/vm/console.sock websocket 127.0.0.1:5901 /run/vm/console.token https://console.example
```

Serve noVNC from the authenticated gateway and forward `/console` WebSocket
requests to port 5901. The request must have both:

- The exact browser `Origin` specified above.
- The path `/console?token=TOKEN` with the token file's value.

Preserve the Origin header at the gateway. Use TLS for remote browser access. Do
not log the query string. Rotate the token by restarting the bridge. The
WebSocket mode does not serve HTTP assets. The separate `assets` mode serves the
vendored browser files. Hosted identity, TLS and per-VM user authorisation are
gateway responsibilities.

The noVNC 1.6.0 browser application is vendored in `web/` at commit
`a8dfd6a3ea3c74244f5ebdaa5a7f1023007a7820`. Its source, authors and licence
texts are included. Runtime files remain unmodified; icon generation inputs are
omitted. Browser code is loaded locally, without a CDN.

For an SSH-only session, run the asset server in one terminal on the VM host:

```sh
build/cargo_target/debug/firecracker-console tools/windows/console/web assets 127.0.0.1:6080
```

Run the WebSocket bridge in another terminal on the same host:

```sh
build/cargo_target/debug/firecracker-console /run/vm/console.sock websocket 127.0.0.1:5901 /run/vm/console.token http://127.0.0.1:6080
```

Forward both ports from the local computer:

```sh
ssh -N -T -o ExitOnForwardFailure=yes \
  -L 127.0.0.1:6080:127.0.0.1:6080 -L 127.0.0.1:5901:127.0.0.1:5901 user@vm-host
```

Open this local URL, replacing `TOKEN` with the token file's value:

```text
http://127.0.0.1:6080/vnc.html#autoconnect=true&host=127.0.0.1&port=5901&encrypt=false&path=console%3Ftoken%3DTOKEN
```

noVNC reads its configuration from the URL fragment. The page request therefore
does not contain the token. The WebSocket handshake still sends the token in its
query: gateway access logs must redact that query. The asset server does not log
requests. Loopback HTTP is a browser secure context; remote deployment must use
HTTPS and `encrypt=true`. The asset server never binds a public interface.

For a hosted gateway, serve the same files, route the WebSocket upgrade, and
supply matching host, port, TLS and Origin settings. Token secrecy does not
replace gateway user authorisation. Browser assets are not in the VMM binary.

## Behaviour and limits

- Run one bridge process per VM. It admits one active controller; extra
  connections close immediately. Separate bridge processes do not share a
  session lock.
- Raw framebuffer encoding, 32-bit RGB888 in either byte order, and DesktopSize.
- Maximum framebuffer size: 32 MiB. Maximum refresh rate: about 30 Hz.
- Keyboard mapping: US physical layout, function/navigation/keypad keys,
  modifiers, Pause and Print Screen. Printable ASCII keysyms retain their case
  and punctuation through Shift, Caps Lock and Num Lock handling. Guest LED
  state is queried before key presses, including after reconnect or restore.
  Unicode text injection and automatic guest-layout detection are not
  implemented.
- Three-button USB HID absolute pointer with vertical scrolling once the guest
  or UEFI firmware has configured UHCI/HID. Pointer events are ignored until USB
  is ready. There is no second mouse or relative-pointer fallback.
- Clipboard input is consumed and discarded, with a 1 MiB bound.
- A 30-second read timeout bounds authentication and negotiation. After
  authentication, idle sessions remain connected. Backend socket reads and
  writes retain a 30-second timeout. Backend connections fail immediately when
  the listener backlog is full.
- Authenticated session disconnect and protocol errors send release-all to the
  VMM. Failed WebSocket authentication does not send guest input. Forced process
  termination cannot perform this exchange; the next successful session sends
  release-all before use. If an input queue is full, the VMM retains and retries
  pending releases before accepting new input. This pending state survives
  snapshots.
- No video compression, audio, clipboard sharing or guest file transfer.

The host Unix protocol uses one request per connection. Each request is 12
bytes: `FCC1`, operation byte, then seven payload bytes. Unused payload bytes
must be zero. Operations are frame (0), key (1), release-all (3), eject (4),
input-state query (5), and absolute pointer (6). The bridge does not expose
eject to RFB clients. Operation 2 is reserved and rejected.

Frame replies are `FCF1`, little-endian u32 width/height/byte count, then BGRA
pixels. Other replies are 16 bytes: `FCA1`, little-endian u32 status, then eight
bytes. Status zero means success; nonzero means the request failed. Input
acknowledgements have zero bytes after the status. Input-state replies use:

| Response byte | Meaning                                                                 |
| ------------- | ----------------------------------------------------------------------- |
| 8             | Absolute pointer ready: 0 or 1                                          |
| 9             | Guest keyboard LEDs: Scroll Lock bit 0, Num Lock bit 1, Caps Lock bit 2 |
| 10            | Pending keyboard LED acknowledgements, using the same three bits        |
| 11..15        | Zero                                                                    |

The bridge waits up to 100 ms for a pending Caps Lock or Num Lock LED
acknowledgement before a non-lock key press. It fails the session if the guest
does not acknowledge; it does not guess the lock state. Scroll Lock does not
affect printable translation and does not delay input. Key releases do not wait.

Operation 6 uses request byte 5 for buttons (left bit 0, right bit 1, middle bit
2), bytes 6..7 for little-endian X, bytes 8..9 for little-endian Y, byte 10 for
signed vertical wheel movement, and byte 11 zero. Coordinates range from 0 to
32767; wheel movement ranges from -127 to 127. Positive wheel movement scrolls
up. The bridge scales RFB pixel coordinates to this absolute range.

Tests cover binary WebSocket transport and authentication, RFB negotiation,
frame clipping/resizing, pixel conversion, and keyboard/mouse requests against a
Unix-socket backend. These tests do not replace guest or noVNC browser tests.

## Unattended capture and input

Capture the current display as binary RGB PPM. Existing files are not replaced:

```sh
mkdir -p build/windows/console
build/cargo_target/debug/firecracker-console /run/vm/console.sock screenshot build/windows/console/boot.ppm
```

Input commands use hexadecimal PS/2 set-2 make codes, with explicit press and
release. For Enter:

```sh
build/cargo_target/debug/firecracker-console /run/vm/console.sock key 5a normal press
build/cargo_target/debug/firecracker-console /run/vm/console.sock key 5a normal release
```

Use `extended` for E0-prefixed keys. Special code `e1` is Pause; `e0` is Print
Screen. `release` releases all held keys and mouse buttons. `eject` requests
optical-media eject and fails if the guest has locked the medium. These commands
use the private local socket; run them over SSH for remote tests.

## Live transport probe

The Rust example tests an actual bridge endpoint. It authenticates, negotiates
RFB, reads two frames, and sends/releases the `W` key. Use a disposable guest
that is waiting for keyboard input:

```sh
cargo run --locked --manifest-path tools/windows/console/Cargo.toml --example probe -- 127.0.0.1:5901 /run/vm/console.token http://127.0.0.1:6080
```

The historical console diagnostic ISO test passed through the real Firecracker
Unix socket and WebSocket bridge: a 720x400 frame was received and `w` appeared
in both the guest serial log and the captured display. This establishes the
transport/input path; it is not a Windows installation or Windows PE storage/API
probe result. The [validation record](../../../docs/windows-validation.md)
separates current checks from historical binaries and snapshot formats.

The vendored noVNC application was historically tested in Chrome against Windows
10 Setup through an SSH-forwarded WebSocket bridge. Its screen refreshed after
guest activity, and physical browser key events executed `echo browser` in the
guest command prompt. Closing the test tab left the VM running.
