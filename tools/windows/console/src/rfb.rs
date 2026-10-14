// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use crate::{Backend, Frame, invalid, keys};
use des::cipher::{BlockEncrypt, KeyInit};
use std::io::{self, Read, Write};

const DEFAULT_FORMAT: [u8; 16] = [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0];

fn read<const N: usize>(stream: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[derive(Clone, Copy)]
struct PixelFormat {
    big_endian: bool,
    red: u8,
    green: u8,
    blue: u8,
}
impl PixelFormat {
    fn parse(bytes: [u8; 16]) -> io::Result<Self> {
        let shifts = [bytes[10], bytes[11], bytes[12]];
        if bytes[0] != 32
            || ![24, 32].contains(&bytes[1])
            || bytes[2] > 1
            || bytes[3] != 1
            || bytes[4..10] != [0, 255, 0, 255, 0, 255]
            || shifts.iter().any(|shift| ![0, 8, 16, 24].contains(shift))
            || shifts[0] == shifts[1]
            || shifts[0] == shifts[2]
            || shifts[1] == shifts[2]
        {
            return Err(invalid(
                "only 32-bit true-colour RGB888 pixel formats are supported",
            ));
        }
        Ok(Self {
            big_endian: bytes[2] != 0,
            red: bytes[10],
            green: bytes[11],
            blue: bytes[12],
        })
    }
    fn convert(self, pixels: &mut [u8]) {
        for pixel in pixels.as_chunks_mut::<4>().0 {
            let value = u32::from(pixel[2]) << self.red
                | u32::from(pixel[1]) << self.green
                | u32::from(pixel[0]) << self.blue;
            pixel.copy_from_slice(&if self.big_endian {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            });
        }
    }
}

fn rectangle(
    stream: &mut impl Write,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    encoding: i32,
) -> io::Result<()> {
    for value in [x, y, width, height] {
        stream.write_all(&value.to_be_bytes())?;
    }
    stream.write_all(&encoding.to_be_bytes())
}

fn update(
    stream: &mut impl Write,
    frame: &mut Frame,
    format: PixelFormat,
    size: &mut (u16, u16),
    resize: bool,
    region: [u16; 4],
) -> io::Result<()> {
    let changed = *size != (frame.width, frame.height);
    if changed && !resize {
        return Err(invalid(
            "client does not support display resizing; reconnect",
        ));
    }
    let [x, y, width, height] = region;
    let width = width.min(frame.width.saturating_sub(x));
    let height = height.min(frame.height.saturating_sub(y));
    let pixels = width != 0 && height != 0;
    let count = u16::from(changed) + u16::from(pixels);
    stream.write_all(&[0, 0])?;
    stream.write_all(&count.to_be_bytes())?;
    if pixels {
        format.convert(&mut frame.pixels);
        rectangle(stream, x, y, width, height, 0)?;
        for row in y..y + height {
            let start = (usize::from(row) * usize::from(frame.width) + usize::from(x)) * 4;
            stream.write_all(&frame.pixels[start..start + usize::from(width) * 4])?;
        }
    }
    // DesktopSize must be the last rectangle in an update (RFC 6143, 7.8.2).
    if changed {
        rectangle(stream, 0, 0, frame.width, frame.height, -223)?;
        *size = (frame.width, frame.height);
    }
    stream.flush()
}

#[derive(Clone)]
pub struct Password([u8; 8]);

impl Password {
    pub fn new(value: &str) -> io::Result<Self> {
        if value.is_empty() || value.len() > 8 || !value.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(invalid(
                "VNC password must contain 1..8 printable ASCII characters",
            ));
        }
        let mut key = [0; 8];
        for (out, byte) in key.iter_mut().zip(value.bytes()) {
            *out = byte.reverse_bits();
        }
        Ok(Self(key))
    }

    fn response(&self, mut challenge: [u8; 16]) -> [u8; 16] {
        let cipher = des::Des::new(&self.0.into());
        for block in challenge.as_chunks_mut::<8>().0 {
            cipher.encrypt_block(block.into());
        }
        challenge
    }
}

pub(crate) fn handshake(
    stream: &mut (impl Read + Write),
    password: Option<&Password>,
) -> io::Result<()> {
    stream.write_all(b"RFB 003.008\n")?;
    stream.flush()?;
    let version = read::<12>(stream)?;
    let security = if password.is_some() { 2u8 } else { 1 };
    match &version {
        b"RFB 003.003\n" => stream.write_all(&u32::from(security).to_be_bytes())?,
        b"RFB 003.007\n" | b"RFB 003.008\n" => {
            stream.write_all(&[1, security])?;
            stream.flush()?;
            if read::<1>(stream)? != [security] {
                return Err(invalid("invalid RFB security selection"));
            }
        }
        _ => return Err(invalid("unsupported RFB version")),
    }
    if let Some(password) = password {
        let mut challenge = [0; 16];
        getrandom::fill(&mut challenge).map_err(io::Error::other)?;
        stream.write_all(&challenge)?;
        stream.flush()?;
        let response = read::<16>(stream)?;
        let expected = password.response(challenge);
        if response
            .iter()
            .zip(expected)
            .fold(0, |diff, (a, b)| diff | a ^ b)
            != 0
        {
            stream.write_all(&1u32.to_be_bytes())?;
            if version == *b"RFB 003.008\n" {
                let reason = b"Authentication failed";
                stream.write_all(&(reason.len() as u32).to_be_bytes())?;
                stream.write_all(reason)?;
            }
            stream.flush()?;
            return Err(invalid("VNC authentication failed"));
        }
    }
    if password.is_some() || version == *b"RFB 003.008\n" {
        stream.write_all(&[0; 4])?;
    }
    stream.flush()
}

fn absolute_axis(position: u16, extent: u16) -> u16 {
    let maximum = extent.saturating_sub(1);
    if maximum == 0 {
        return 0;
    }
    (u32::from(position.min(maximum)) * 32767 / u32::from(maximum)) as u16
}

#[derive(Clone, Copy, Default)]
pub enum KeyboardMapping {
    #[default]
    Standard,
    MacOs,
}

impl KeyboardMapping {
    fn keysym(self, value: u32) -> u32 {
        match (self, value) {
            (Self::MacOs, 0xffe9) => 0xffeb,
            (Self::MacOs, 0xffea) => 0xffec,
            _ => value,
        }
    }
}

pub fn serve(
    stream: &mut (impl Read + Write),
    backend: &Backend,
    mapping: KeyboardMapping,
) -> io::Result<()> {
    read::<1>(stream)?;
    let frame = backend.frame()?;
    stream.write_all(&frame.width.to_be_bytes())?;
    stream.write_all(&frame.height.to_be_bytes())?;
    stream.write_all(&DEFAULT_FORMAT)?;
    let name = b"Firecracker console";
    stream.write_all(&(name.len() as u32).to_be_bytes())?;
    stream.write_all(name)?;
    stream.flush()?;
    let mut size = (frame.width, frame.height);
    let mut format = PixelFormat::parse(DEFAULT_FORMAT)?;
    let mut resize = false;
    let mut keyboard = keys::Keyboard::default();
    loop {
        match read::<1>(stream)?[0] {
            0 => {
                read::<3>(stream)?;
                format = PixelFormat::parse(read(stream)?)?;
            }
            2 => {
                let header = read::<3>(stream)?;
                let count = u16::from_be_bytes([header[1], header[2]]);
                if count > 1024 {
                    return Err(invalid("too many RFB encodings"));
                }
                resize = false;
                for _ in 0..count {
                    if i32::from_be_bytes(read(stream)?) == -223 {
                        resize = true;
                    }
                }
            }
            3 => {
                let request = read::<9>(stream)?;
                let mut region = [0; 4];
                for (out, bytes) in region
                    .iter_mut()
                    .zip(request[1..].as_chunks::<2>().0.iter())
                {
                    *out = u16::from_be_bytes(*bytes);
                }
                update(
                    stream,
                    &mut backend.frame()?,
                    format,
                    &mut size,
                    resize,
                    region,
                )?;
                // Bound refresh rate even when the client continuously requests unchanged pixels.
                std::thread::sleep(std::time::Duration::from_millis(33));
            }
            4 => {
                let request = read::<7>(stream)?;
                if request[0] > 1 {
                    return Err(invalid("invalid key state"));
                }
                let keysym = mapping.keysym(u32::from_be_bytes(request[3..].try_into().unwrap()));
                let mut pending_leds = 0;
                if request[0] != 0 {
                    let state = backend.input_state()?;
                    keyboard.set_leds(state.leds);
                    pending_leds = state.pending_leds;
                }
                if request[0] != 0 && !matches!(keysym, 0xffe5 | 0xff7f) {
                    let deadline =
                        std::time::Instant::now() + std::time::Duration::from_millis(100);
                    while pending_leds != 0 {
                        if std::time::Instant::now() >= deadline {
                            return Err(invalid("guest keyboard LED update timed out"));
                        }
                        std::thread::sleep(std::time::Duration::from_millis(1));
                        let state = backend.input_state()?;
                        keyboard.set_leds(state.leds);
                        pending_leds = state.pending_leds;
                    }
                }
                keyboard.event(keysym, request[0] != 0, |code, extended, pressed| {
                    backend.input(1, &[code, u8::from(extended), u8::from(pressed)])
                })?;
            }
            5 => {
                let request = read::<5>(stream)?;
                let x = u16::from_be_bytes([request[1], request[2]]);
                let y = u16::from_be_bytes([request[3], request[4]]);
                let buttons = request[0] & 1 | (request[0] & 2) << 1 | (request[0] & 4) >> 1;
                let mut payload = [0; 6];
                payload[0] = buttons;
                if backend.input_state()?.absolute_pointer {
                    payload[1..3].copy_from_slice(&absolute_axis(x, size.0).to_le_bytes());
                    payload[3..5].copy_from_slice(&absolute_axis(y, size.1).to_le_bytes());
                    payload[5] =
                        (i8::from(request[0] & 8 != 0) - i8::from(request[0] & 16 != 0)) as u8;
                    backend.input(6, &payload)?;
                }
            }
            6 => {
                let header = read::<7>(stream)?;
                let count = u32::from_be_bytes(header[3..].try_into().unwrap());
                if count > 1024 * 1024 {
                    return Err(invalid("clipboard exceeds limit"));
                }
                io::copy(&mut stream.take(u64::from(count)), &mut io::sink())?;
            }
            _ => return Err(invalid("unsupported RFB message")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absolute_coordinates_cover_edges_and_clamp_outside_display() {
        assert_eq!(absolute_axis(0, 800), 0);
        assert_eq!(absolute_axis(799, 800), 32767);
        assert_eq!(absolute_axis(600, 600), 32767);
        assert_eq!(absolute_axis(32768, 0), 0);
        assert_eq!(absolute_axis(1, 1), 0);
        assert_eq!(absolute_axis(512, 1025), 16383);
    }

    #[test]
    fn macos_command_mapping_is_explicit_and_preserves_option() {
        for (keysym, standard, macos) in [
            (0xffe9, (0x11, false), (0x1f, true)),
            (0xffea, (0x11, true), (0x27, true)),
            (0xffe7, (0x11, false), (0x11, false)),
            (0xffe8, (0x11, true), (0x11, true)),
            (0xffeb, (0x1f, true), (0x1f, true)),
            (0xffec, (0x27, true), (0x27, true)),
        ] {
            assert_eq!(
                keys::set2(KeyboardMapping::Standard.keysym(keysym)),
                Some(standard)
            );
            assert_eq!(
                keys::set2(KeyboardMapping::MacOs.keysym(keysym)),
                Some(macos)
            );
        }
    }
    #[test]
    fn security_handshake_versions() {
        struct Stream {
            input: io::Cursor<Vec<u8>>,
            output: Vec<u8>,
        }
        impl Read for Stream {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                self.input.read(bytes)
            }
        }
        impl Write for Stream {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.output.write(bytes)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        for (version, selection, expected) in [
            (b"RFB 003.003\n", &[][..], &[0, 0, 0, 1][..]),
            (b"RFB 003.007\n", &[1][..], &[1, 1][..]),
            (b"RFB 003.008\n", &[1][..], &[1, 1, 0, 0, 0, 0][..]),
        ] {
            let mut input = version.to_vec();
            input.extend(selection);
            input.push(1); // ClientInit must remain unread by the handshake.
            let mut stream = Stream {
                input: io::Cursor::new(input),
                output: Vec::new(),
            };
            handshake(&mut stream, None).unwrap();
            assert_eq!(&stream.output[..12], b"RFB 003.008\n");
            assert_eq!(&stream.output[12..], expected);
            assert_eq!(read::<1>(&mut stream).unwrap(), [1]);
        }
        for input in [b"RFB 003.009\n".to_vec(), b"RFB 003.008\n\x02".to_vec()] {
            let mut stream = Stream {
                input: io::Cursor::new(input),
                output: Vec::new(),
            };
            assert!(handshake(&mut stream, None).is_err());
        }
    }
    #[test]
    fn pixel_layouts_and_validation() {
        let mut pixels = [3, 2, 1, 0];
        PixelFormat::parse(DEFAULT_FORMAT)
            .unwrap()
            .convert(&mut pixels);
        assert_eq!(pixels, [3, 2, 1, 0]);
        let mut format = DEFAULT_FORMAT;
        format[2] = 1;
        PixelFormat::parse(format).unwrap().convert(&mut pixels);
        assert_eq!(pixels, [0, 1, 2, 3]);
        format[10] = 8;
        assert!(PixelFormat::parse(format).is_err());
        format = DEFAULT_FORMAT;
        format[0] = 16;
        assert!(PixelFormat::parse(format).is_err());
    }
    #[test]
    fn cropped_update_and_resize() {
        let mut frame = Frame {
            width: 2,
            height: 2,
            pixels: (0..16).collect(),
        };
        let mut out = Vec::new();
        let mut size = (1, 1);
        update(
            &mut out,
            &mut frame,
            PixelFormat::parse(DEFAULT_FORMAT).unwrap(),
            &mut size,
            true,
            [1, 1, 5, 5],
        )
        .unwrap();
        assert_eq!(&out[..4], &[0, 0, 0, 2]);
        assert_eq!(&out[16..20], &[12, 13, 14, 0]);
        assert_eq!(&out[out.len() - 4..], &(-223i32).to_be_bytes());
        assert_eq!(size, (2, 2));
    }
}

#[cfg(test)]
#[path = "rfb_auth_tests.rs"]
mod auth_tests;
