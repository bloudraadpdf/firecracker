// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

mod assets;
mod keys;
mod rfb;
mod websocket;

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const MAX_FRAME: usize = 32 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Clone)]
struct Backend(PathBuf);

struct Frame {
    width: u16,
    height: u16,
    pixels: Vec<u8>,
}

impl Backend {
    fn request(&self, op: u8, payload: &[u8]) -> io::Result<(UnixStream, [u8; 16])> {
        if payload.len() > 7 {
            return Err(invalid("console payload too large"));
        }
        let mut stream = UnixStream::connect(&self.0)?;
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
            width: width as u16,
            height: height as u16,
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

fn listen(
    address: SocketAddr,
    backend: Backend,
    auth: Option<websocket::Auth>,
    busy: Arc<AtomicBool>,
) -> io::Result<()> {
    if !address.ip().is_loopback() {
        return Err(invalid(
            "console listeners must bind to loopback; use an authenticated remote gateway or SSH tunnel",
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
        std::thread::spawn(move || {
            let result = (|| {
                configure(&stream)?;
                if let Some(auth) = auth {
                    let mut socket = websocket::accept(stream, &auth)?;
                    guard.input_active = true;
                    guard.backend.input(3, &[])?;
                    rfb::serve(&mut socket, &guard.backend)
                } else {
                    guard.input_active = true;
                    guard.backend.input(3, &[])?;
                    rfb::serve(&mut { stream }, &guard.backend)
                }
            })();
            if let Err(error) = result {
                eprintln!("Console session ended: {error}");
            }
        });
    }
    Ok(())
}

fn run() -> io::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let socket = args.next().ok_or_else(|| invalid("usage: firecracker-console SOCKET rfb ADDRESS | SOCKET websocket ADDRESS TOKEN_FILE ORIGIN | WEB_DIR assets ADDRESS"))?;
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
    let auth = if mode == "rfb" {
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
            "mode must be rfb, websocket, assets, screenshot, key, release or eject",
        ));
    };
    if args.next().is_some() {
        return Err(invalid("unexpected argument"));
    }
    listen(
        address,
        Backend(PathBuf::from(socket)),
        auth,
        Arc::new(AtomicBool::new(false)),
    )
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn rfb_drives_frame_keyboard_and_mouse_over_unix_protocol() {
        let path = PathBuf::from(format!("console-test-{}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let backend = Backend(path.clone());
        let host = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 12];
                stream.read_exact(&mut request).unwrap();
                assert_eq!(&request[..4], b"FCC1");
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
                    stream.write_all(&response).unwrap();
                }
            }
            requests
        });
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(tcp.local_addr().unwrap()).unwrap();
        configure(&client).unwrap();
        let (mut server, _) = tcp.accept().unwrap();
        let session = std::thread::spawn(move || rfb::serve(&mut server, &backend));
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
        let requests = host.join().unwrap();
        assert_eq!(&requests[2][4..8], &[1, 0x1c, 0, 1]);
        assert_eq!(&requests[3][4..10], &[2, 1, 0, 0, 0, 0]);
        assert_eq!(&requests[4][4..10], &[2, 2, 5, 0, 2, 0]);
        drop(client);
        assert!(session.join().unwrap().is_err());
        std::fs::remove_file(path).unwrap();
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
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 12];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(&request[..5], b"FCC1\x03");
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
