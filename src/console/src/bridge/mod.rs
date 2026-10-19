// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

mod assets;
mod keys;
#[cfg(feature = "gui")]
mod local;
mod rfb;
mod websocket;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[cfg(feature = "gui")]
pub use local::LocalBridge;

use crate::{Frame, MAX_FRAME, invalid};

const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct Backend(PathBuf);

struct InputState {
    absolute_pointer: bool,
    leds: u8,
    pending_leds: u8,
}

impl Backend {
    fn request(&self, op: u8, payload: &[u8]) -> io::Result<(UnixStream, [u8; 16])> {
        if payload.len() > 7 {
            return Err(invalid("console payload too large"));
        }
        let mut stream = utils::unix_socket::connect(&self.0)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        let mut request = [0; 12];
        request[..4].copy_from_slice(b"FCC1");
        request[4] = op;
        request[5..5 + payload.len()].copy_from_slice(payload);
        stream.write_all(&request)?;
        let mut response = [0; 16];
        stream.read_exact(&mut response)?;
        Ok((stream, response))
    }

    fn input(&self, op: u8, payload: &[u8]) -> io::Result<()> {
        let (_, response) = self.request(op, payload)?;
        if &response[..4] != b"FCA1" || response[4..] != [0; 12] {
            return Err(invalid("console rejected input"));
        }
        Ok(())
    }

    fn input_state(&self) -> io::Result<InputState> {
        let (_, response) = self.request(5, &[])?;
        if &response[..4] != b"FCA1"
            || (response[9] & !7 != 0 || response[10] & !7 != 0 || response[11..] != [0; 5])
        {
            return Err(invalid("invalid pointer capability response"));
        }
        let status = u32::from_le_bytes(response[4..8].try_into().unwrap());
        match (status, response[8]) {
            (0, ready @ 0..=1) => Ok(InputState {
                absolute_pointer: ready != 0,
                leds: response[9],
                pending_leds: response[10],
            }),
            _ => Err(invalid("invalid pointer capability response")),
        }
    }

    fn frame(&self) -> io::Result<Frame> {
        let (mut stream, response) = self.request(0, &[])?;
        if &response[..4] != b"FCF1" {
            return Err(invalid("console frame unavailable"));
        }
        let width = u32::from_le_bytes(response[4..8].try_into().unwrap());
        let height = u32::from_le_bytes(response[8..12].try_into().unwrap());
        let length = u32::from_le_bytes(response[12..16].try_into().unwrap()) as usize;
        if width == 0
            || height == 0
            || width > u32::from(u16::MAX)
            || height > u32::from(u16::MAX)
            || length > MAX_FRAME
            || u64::from(width) * u64::from(height) * 4 != length as u64
        {
            return Err(invalid("invalid console frame dimensions"));
        }
        let mut pixels = vec![0; length];
        stream.read_exact(&mut pixels)?;
        Ok(Frame {
            width: u16::try_from(width).unwrap(),
            height: u16::try_from(height).unwrap(),
            pixels,
        })
    }
}

impl Frame {
    fn write_ppm(&self, mut output: impl Write) -> io::Result<()> {
        write!(output, "P6\n{} {}\n255\n", self.width, self.height)?;
        for pixel in self.pixels.as_chunks::<4>().0 {
            output.write_all(&[pixel[2], pixel[1], pixel[0]])?;
        }
        output.flush()
    }
}

struct SessionGuard {
    input_active: bool,
    backend: Backend,
    busy: Arc<AtomicBool>,
}
impl Drop for SessionGuard {
    fn drop(&mut self) {
        if self.input_active
            && let Err(error) = self.backend.input(3, &[])
        {
            eprintln!("Could not release console input: {error}");
        }
        self.busy.store(false, Ordering::Release);
    }
}

fn configure(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))
}

fn authenticate_console(
    stream: &mut (impl Read + Write),
    guard: &mut SessionGuard,
    password: Option<&rfb::Password>,
) -> io::Result<()> {
    rfb::handshake(stream, password)?;
    guard.input_active = true;
    guard.backend.input(3, &[])
}

fn serve_connection(
    mut stream: TcpStream,
    guard: &mut SessionGuard,
    auth: Option<websocket::Auth>,
    password: Option<rfb::Password>,
    mapping: rfb::KeyboardMapping,
) -> io::Result<()> {
    configure(&stream)?;
    let transport = stream.try_clone()?;
    if let Some(auth) = auth {
        let mut socket = websocket::accept(stream, &auth)?;
        authenticate_console(&mut socket, guard, None)?;
        transport.set_read_timeout(None)?;
        rfb::serve(&mut socket, &guard.backend, mapping)
    } else {
        authenticate_console(&mut stream, guard, password.as_ref())?;
        transport.set_read_timeout(None)?;
        rfb::serve(&mut stream, &guard.backend, mapping)
    }
}

fn listen(
    address: SocketAddr,
    backend: Backend,
    auth: Option<websocket::Auth>,
    password: Option<rfb::Password>,
    mapping: rfb::KeyboardMapping,
    busy: Arc<AtomicBool>,
) -> io::Result<()> {
    if !address.ip().is_loopback() {
        return Err(invalid(
            "console listeners must bind to loopback; use an authenticated remote gateway or SSH \
             tunnel",
        ));
    }
    let listener = TcpListener::bind(address)?;
    eprintln!(
        "{} console on {}",
        if auth.is_some() { "WebSocket" } else { "RFB" },
        listener.local_addr()?
    );
    for connection in listener.incoming() {
        let stream = connection?;
        if busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            continue;
        }
        let mut guard = SessionGuard {
            input_active: false,
            backend: backend.clone(),
            busy: busy.clone(),
        };
        let auth = auth.clone();
        let password = password.clone();
        std::thread::spawn(move || {
            let result = serve_connection(stream, &mut guard, auth, password, mapping);
            if let Err(error) = result {
                eprintln!("Console session ended: {error}");
            }
        });
    }
    Ok(())
}

pub fn run(mut args: impl Iterator<Item = std::ffi::OsString>) -> io::Result<()> {
    let socket = args.next().ok_or_else(|| {
        invalid(
            "usage: firecracker-console SOCKET rfb|rfb-macos ADDRESS [PASSWORD_FILE] | SOCKET \
             websocket ADDRESS TOKEN_FILE ORIGIN | WEB_DIR assets ADDRESS",
        )
    })?;
    let mode = args.next().ok_or_else(|| invalid("missing mode"))?;
    let backend = Backend(PathBuf::from(&socket));
    if mode == "screenshot" {
        let output = args
            .next()
            .ok_or_else(|| invalid("missing screenshot output path"))?;
        if args.next().is_some() {
            return Err(invalid("unexpected argument"));
        }
        let frame = backend.frame()?;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?;
        return frame.write_ppm(io::BufWriter::new(file));
    }
    if mode == "release" || mode == "eject" {
        if args.next().is_some() {
            return Err(invalid("unexpected argument"));
        }
        return backend.input(if mode == "release" { 3 } else { 4 }, &[]);
    }
    if mode == "key" {
        let code = args
            .next()
            .ok_or_else(|| invalid("missing hexadecimal set-2 key code"))?
            .into_string()
            .map_err(|_| invalid("invalid key code"))?;
        let code = u8::from_str_radix(code.trim_start_matches("0x"), 16)
            .map_err(|_| invalid("invalid hexadecimal key code"))?;
        let extended = match args.next().as_deref().and_then(|value| value.to_str()) {
            Some("normal") => 0,
            Some("extended") => 1,
            _ => return Err(invalid("key requires normal or extended")),
        };
        let pressed = match args.next().as_deref().and_then(|value| value.to_str()) {
            Some("press") => 1,
            Some("release") => 0,
            _ => return Err(invalid("key requires press or release")),
        };
        if args.next().is_some() {
            return Err(invalid("unexpected argument"));
        }
        return backend.input(1, &[code, extended, pressed]);
    }
    let address: SocketAddr = args
        .next()
        .ok_or_else(|| invalid("missing bind address"))?
        .to_str()
        .ok_or_else(|| invalid("invalid bind address"))?
        .parse()
        .map_err(|_| invalid("invalid bind address"))?;
    if mode == "assets" {
        if args.next().is_some() {
            return Err(invalid("unexpected argument"));
        }
        return assets::serve(address, PathBuf::from(socket));
    }
    let mut password = None;
    let mapping = if mode == "rfb-macos" {
        rfb::KeyboardMapping::MacOs
    } else {
        rfb::KeyboardMapping::Standard
    };
    let auth = if mode == "rfb" || mode == "rfb-macos" {
        if let Some(path) = args.next() {
            password = Some(rfb::Password::from_file(Path::new(&path))?);
        }
        None
    } else if mode == "websocket" {
        let token_file = args.next().ok_or_else(|| invalid("missing token file"))?;
        let token = std::fs::read_to_string(Path::new(&token_file))?
            .trim()
            .to_owned();
        let origin = args
            .next()
            .ok_or_else(|| invalid("missing exact browser origin"))?
            .into_string()
            .map_err(|_| invalid("invalid origin"))?;
        Some(websocket::Auth::new(token, origin)?)
    } else {
        return Err(invalid(
            "mode must be rfb, rfb-macos, websocket, assets, screenshot, key, release or eject",
        ));
    };
    if args.next().is_some() {
        return Err(invalid("unexpected argument"));
    }
    listen(
        address,
        Backend(PathBuf::from(socket)),
        auth,
        password,
        mapping,
        Arc::new(AtomicBool::new(false)),
    )
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;

    fn read_backend_request(listener: &UnixListener) -> (UnixStream, [u8; 12]) {
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for a console request"
            );
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
                    stream.set_write_timeout(Some(TIMEOUT)).unwrap();
                    let mut request = [0; 12];
                    stream.read_exact(&mut request).unwrap();
                    assert_eq!(&request[..4], b"FCC1");
                    return (stream, request);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("console request failed: {error}"),
            }
        }
    }

    #[test]
    fn full_backend_backlog_does_not_hold_session() {
        const CHILD: &str = "FIRECRACKER_CONSOLE_BACKLOG_TEST_SOCKET";
        if let Some(path) = std::env::var_os(CHILD) {
            let backend = Backend(PathBuf::from(path));
            assert_eq!(
                backend.request(5, &[]).err().unwrap().kind(),
                io::ErrorKind::WouldBlock
            );
            let busy = Arc::new(AtomicBool::new(true));
            drop(SessionGuard {
                input_active: true,
                backend,
                busy: busy.clone(),
            });
            assert!(!busy.load(Ordering::Acquire));
            return;
        }
        let path = PathBuf::from(format!("console-backlog-{}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let result = (|| {
            use std::os::fd::AsRawFd;
            // SAFETY: listener owns the valid socket; backlog zero permits one queued peer.
            if unsafe { libc::listen(listener.as_raw_fd(), 0) } != 0 {
                return Err(io::Error::last_os_error());
            }
            let _queued = utils::unix_socket::connect(&path)?;
            let mut child = std::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "bridge::tests::full_backend_backlog_does_not_hold_session",
                    "--test-threads=1",
                ])
                .env(CHILD, &path)
                .spawn()?;
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Ok(None) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Ok(None) => {
                        break Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "console backend blocked on a full listener backlog",
                        ));
                    }
                    Err(error) => break Err(error),
                }
            };
            let _ = child.kill();
            child.wait()?;
            status
        })();
        drop(listener);
        std::fs::remove_file(path).unwrap();
        assert!(result.unwrap().success());
    }

    #[test]
    fn rfb_drives_frame_keyboard_and_mouse_over_unix_protocol() {
        for (ready, pending) in [(false, 0), (true, 0), (false, 1), (true, 1), (true, 7)] {
            let acknowledgement = usize::from(pending & 6 != 0);
            let path = PathBuf::from(format!("console-test-{}.sock", std::process::id()));
            let listener = UnixListener::bind(&path).unwrap();
            let backend = Backend(path.clone());
            let host = std::thread::spawn(move || {
                let mut requests = Vec::new();
                for _ in 0..(if ready { 8 } else { 6 }) + acknowledgement {
                    let (mut stream, request) = read_backend_request(&listener);
                    requests.push(request);
                    if request[4] == 0 {
                        let mut response = b"FCF1".to_vec();
                        for value in [2u32, 1, 8] {
                            response.extend(value.to_le_bytes());
                        }
                        response.extend([3, 2, 1, 0, 6, 5, 4, 0]);
                        stream.write_all(&response).unwrap();
                    } else {
                        let mut response = [0; 16];
                        response[..4].copy_from_slice(b"FCA1");
                        if request[4] == 5 {
                            response[8] = u8::from(ready);
                            response[10] = if requests.len() == 3 {
                                pending
                            } else {
                                pending & 1
                            };
                        }
                        stream.write_all(&response).unwrap();
                    }
                }
                requests
            });
            let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
            let mut client = TcpStream::connect(tcp.local_addr().unwrap()).unwrap();
            configure(&client).unwrap();
            let (mut server, _) = tcp.accept().unwrap();
            let session = std::thread::spawn(move || {
                rfb::handshake(&mut server, None)?;
                rfb::serve(&mut server, &backend, rfb::KeyboardMapping::Standard)
            });
            let mut version = [0; 12];
            client.read_exact(&mut version).unwrap();
            assert_eq!(&version, b"RFB 003.008\n");
            client.write_all(&version).unwrap();
            let mut security = [0; 2];
            client.read_exact(&mut security).unwrap();
            assert_eq!(security, [1, 1]);
            client.write_all(&[1]).unwrap();
            let mut result = [0; 4];
            client.read_exact(&mut result).unwrap();
            assert_eq!(result, [0; 4]);
            client.write_all(&[1]).unwrap();
            let mut init = [0; 24];
            client.read_exact(&mut init).unwrap();
            assert_eq!(&init[..4], &[0, 2, 0, 1]);
            let length = u32::from_be_bytes(init[20..24].try_into().unwrap());
            let mut name = vec![0; length as usize];
            client.read_exact(&mut name).unwrap();
            client.write_all(&[3, 0, 0, 0, 0, 0, 0, 2, 0, 1]).unwrap();
            let mut update = [0; 24];
            client.read_exact(&mut update).unwrap();
            assert_eq!(&update[16..], &[3, 2, 1, 0, 6, 5, 4, 0]);
            client.write_all(&[4, 1, 0, 0, 0, 0, 0, 0x61]).unwrap();
            client.write_all(&[5, 1, 0, 10, 0, 20]).unwrap();
            client.write_all(&[5, 4, 0, 15, 0, 18]).unwrap();
            let requests = host.join();
            std::fs::remove_file(&path).unwrap();
            let requests = requests.unwrap();
            assert_eq!(&requests[3 + acknowledgement][4..8], &[1, 0x1c, 0, 1]);
            if ready {
                assert_eq!(
                    &requests[5 + acknowledgement][4..11],
                    &[6, 1, 255, 127, 0, 0, 0]
                );
                assert_eq!(
                    &requests[7 + acknowledgement][4..11],
                    &[6, 2, 255, 127, 0, 0, 0]
                );
            } else {
                assert!(
                    requests
                        .iter()
                        .all(|request| request[4] != 2 && request[4] != 6)
                );
            }
            drop(client);
            session.join().unwrap().unwrap_err();
        }
    }

    #[test]
    fn input_state_rejects_undefined_pending_led_bits() {
        let path = PathBuf::from(format!("console-input-flags-{}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let backend = Backend(path.clone());
        let host = std::thread::spawn(move || {
            let (mut stream, request) = read_backend_request(&listener);
            assert_eq!(request[4], 5);
            let mut response = [0; 16];
            response[..4].copy_from_slice(b"FCA1");
            response[10] = 8;
            stream.write_all(&response).unwrap();
        });
        let state = backend.input_state();
        let host = host.join();
        std::fs::remove_file(path).unwrap();
        host.unwrap();
        assert_eq!(state.err().unwrap().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn disconnect_releases_keys_before_another_client_can_connect() {
        let path = PathBuf::from(format!("console-release-{}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let busy = Arc::new(AtomicBool::new(true));
        let guard = SessionGuard {
            input_active: true,
            backend: Backend(path.clone()),
            busy: busy.clone(),
        };
        let host_busy = busy.clone();
        let host = std::thread::spawn(move || {
            let (mut stream, request) = read_backend_request(&listener);
            assert_eq!(request[4], 3);
            assert!(host_busy.load(Ordering::Acquire));
            let mut response = [0; 16];
            response[..4].copy_from_slice(b"FCA1");
            stream.write_all(&response).unwrap();
        });
        drop(guard);
        host.join().unwrap();
        assert!(!busy.load(Ordering::Acquire));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn malformed_frame_headers_are_rejected_before_reading_pixels() {
        let path = PathBuf::from(format!("console-bounds-{}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let headers = [
            [0, 1, 0],
            [1, 0, 0],
            [u32::MAX, u32::MAX, 4],
            [65536, 1, 262144],
            [1, 1, 8],
            [4096, 4096, 67108864],
        ];
        let host = std::thread::spawn(move || {
            for header in headers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 12];
                stream.read_exact(&mut request).unwrap();
                let mut response = b"FCF1".to_vec();
                for value in header {
                    response.extend(value.to_le_bytes());
                }
                stream.write_all(&response).unwrap();
            }
        });
        let backend = Backend(path.clone());
        for _ in headers {
            assert_eq!(
                backend.frame().err().unwrap().kind(),
                io::ErrorKind::InvalidData
            );
        }
        host.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn screenshot_converts_bgra_to_ppm_rgb() {
        let frame = Frame {
            width: 2,
            height: 1,
            pixels: vec![3, 2, 1, 0, 6, 5, 4, 0],
        };
        let mut output = Vec::new();
        frame.write_ppm(&mut output).unwrap();
        assert_eq!(output, b"P6\n2 1\n255\n\x01\x02\x03\x04\x05\x06");
    }
    #[test]
    fn unauthenticated_session_cleanup_cannot_inject_input() {
        let path = PathBuf::from(format!(
            "console-unauthenticated-{}.sock",
            std::process::id()
        ));
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let busy = Arc::new(AtomicBool::new(true));
        drop(SessionGuard {
            input_active: false,
            backend: Backend(path.clone()),
            busy: busy.clone(),
        });
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(!busy.load(Ordering::Acquire));
        std::fs::remove_file(path).unwrap();
    }
}
