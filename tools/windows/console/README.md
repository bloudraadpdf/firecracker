# Firecracker remote console bridge

This host process converts the local Firecracker console protocol into RFB 3.8.
It runs outside the VMM sandbox. Guest networking and vsock are not used.

```text
noVNC browser -> authenticated HTTPS gateway -> loopback WebSocket bridge
                                                        |
Native VNC viewer -> SSH tunnel -> loopback RFB bridge --+
                                                        |
                                            Firecracker Unix socket
```

## Build and check

From the repository root:

```sh
mise exec -- cargo build --locked --manifest-path tools/windows/console/Cargo.toml
mise exec -- cargo test --locked --manifest-path tools/windows/console/Cargo.toml
mise exec -- cargo clippy --locked --manifest-path tools/windows/console/Cargo.toml --all-targets -- -D warnings
```

`Cargo.lock` pins the bridge dependencies. The WebSocket implementation uses
`tungstenite` 0.30.0. RFB follows [RFC 6143](https://www.rfc-editor.org/rfc/rfc6143).

## Native VNC viewer

On the VM host:

```sh
build/cargo_target/debug/firecracker-console /run/vm/console.sock rfb 127.0.0.1:5900
```

Forward that port through SSH and connect a VNC viewer to the forwarded port.
The local RFB protocol uses security type None. The SSH connection or an
external authenticated gateway supplies remote access control. The bridge
rejects non-loopback bind addresses.

## Browser console

Create a private token file containing 32 to 128 random ASCII letters, digits,
`-` or `_`. Do not put the token in the bridge command line. Start the bridge:

```sh
build/cargo_target/debug/firecracker-console /run/vm/console.sock websocket 127.0.0.1:5901 /run/vm/console.token https://console.example
```

Serve noVNC from the authenticated gateway and forward `/console` WebSocket
requests to port 5901. The request must have both:

- The exact browser `Origin` specified above.
- The path `/console?token=TOKEN` with the token file's value.

Preserve the Origin header at the gateway. Use TLS for remote browser access.
Do not log the query string. Rotate the token by restarting the bridge. The
WebSocket mode does not serve HTTP assets. The separate `assets` mode serves the
vendored browser files. Hosted identity, TLS and per-VM user authorisation are
gateway responsibilities.

The unmodified noVNC 1.6.0 browser application is vendored in `web/` at commit
`a8dfd6a3ea3c74244f5ebdaa5a7f1023007a7820`. Its source, authors and licence texts
are included. Browser code is loaded locally, without a CDN.

For an SSH-only session, run the asset and WebSocket processes on the VM host:

```sh
build/cargo_target/debug/firecracker-console tools/windows/console/web assets 127.0.0.1:6080
build/cargo_target/debug/firecracker-console /run/vm/console.sock websocket 127.0.0.1:5901 /run/vm/console.token http://127.0.0.1:6080
```

Forward both ports from the local computer:

```sh
ssh -L 6080:127.0.0.1:6080 -L 5901:127.0.0.1:5901 user@vm-host
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
  modifiers, Pause and Print Screen. Unicode text injection is not implemented.
- Standard three-button relative PS/2 mouse. No wheel or absolute tablet.
- Clipboard input is consumed and discarded, with a 1 MiB bound.
- Each connection has a 30-second read/write deadline. A silent client reconnects.
- Authenticated session disconnect and protocol errors send release-all to the
  VMM. Failed WebSocket authentication does not send guest input.
  Forced process termination cannot perform this exchange; the next successful
  session sends release-all before use.
- No video compression, audio, clipboard sharing or guest file transfer.

The host Unix protocol uses one request per connection. Each request is 12 bytes:
`FCC1`, operation byte, then seven payload bytes. Operations are frame (0),
key (1), relative mouse (2), release-all (3), and eject (4). This bridge does
not expose eject to RFB clients. Frame replies are `FCF1`, little-endian u32
width/height/byte count, then BGRA pixels. Input replies are `FCA1`, little-endian
u32 status and eight zero bytes. Zero means success.

Tests cover binary WebSocket transport and authentication, RFB negotiation,
frame clipping/resizing, pixel conversion, and keyboard/mouse requests against
a Unix-socket backend. These tests do not replace guest or noVNC browser tests.

## Unattended capture and input

Capture the current display as binary RGB PPM. Existing files are not replaced:

```sh
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
mise exec -- cargo run --locked --manifest-path tools/windows/console/Cargo.toml --example probe -- 127.0.0.1:5901 /run/vm/console.token http://127.0.0.1:6080
```

The diagnostic ISO test passed through the real Firecracker Unix socket and
WebSocket bridge: a 720x400 frame was received and `w` appeared in both the guest
serial log and the captured display. This establishes the transport/input path;
it is not a Windows installation result.

The vendored noVNC application was also tested in Chrome against Windows 10
Setup through an SSH-forwarded WebSocket bridge. Its screen refreshed after
guest activity, and physical browser key events executed `echo browser` in
the guest command prompt. Closing the test tab left the VM running.
