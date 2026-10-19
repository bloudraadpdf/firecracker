// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io::{self, Read, Write};

use crate::auth::Password;
use crate::{Frame, MAX_FRAME, RFB_FORMAT as FORMAT, invalid, read};

fn pixel_bytes(width: u16, height: u16) -> io::Result<usize> {
    let bytes = usize::try_from(u64::from(width) * u64::from(height) * 4)
        .map_err(|_| invalid("RFB framebuffer exceeds the address space"))?;
    if width == 0 || height == 0 || bytes > MAX_FRAME {
        return Err(invalid("RFB framebuffer exceeds the 32 MiB limit"));
    }
    Ok(bytes)
}

pub(crate) fn handshake(
    stream: &mut (impl Read + Write),
    password: Option<&Password>,
) -> io::Result<Frame> {
    if read::<12>(stream)? != *b"RFB 003.008\n" {
        return Err(invalid("the viewer requires an RFB 3.8 bridge"));
    }
    stream.write_all(b"RFB 003.008\n")?;
    stream.flush()?;
    let count = usize::from(read::<1>(stream)?[0]);
    if count == 0 || count > 16 {
        return Err(invalid("RFB bridge refused the connection"));
    }
    let mut methods = [0; 16];
    stream.read_exact(&mut methods[..count])?;
    let method = if password.is_some() { 2 } else { 1 };
    if !methods[..count].contains(&method) {
        return Err(invalid(if password.is_some() {
            "RFB bridge does not offer password authentication"
        } else {
            "RFB bridge requires --password-file"
        }));
    }
    stream.write_all(&[method])?;
    stream.flush()?;
    if let Some(password) = password {
        let challenge = read::<16>(stream)?;
        stream.write_all(&password.response(challenge))?;
        stream.flush()?;
    }
    if read::<4>(stream)? != [0; 4] {
        return Err(invalid("RFB authentication failed"));
    }
    stream.write_all(&[1])?;
    stream.flush()?;
    let header = read::<24>(stream)?;
    let width = u16::from_be_bytes([header[0], header[1]]);
    let height = u16::from_be_bytes([header[2], header[3]]);
    let length = pixel_bytes(width, height)?;
    let name_length = u32::from_be_bytes(header[20..24].try_into().unwrap());
    if name_length > 4096 {
        return Err(invalid("RFB desktop name exceeds limit"));
    }
    if io::copy(&mut stream.take(u64::from(name_length)), &mut io::sink())?
        != u64::from(name_length)
    {
        return Err(invalid("truncated RFB desktop name"));
    }
    stream.write_all(&[0; 4])?;
    stream.write_all(&FORMAT)?;
    // Raw pixels and DesktopSize are the existing bridge's complete display contract.
    stream.write_all(&[2, 0, 0, 2, 0, 0, 0, 0, 255, 255, 255, 33])?;
    stream.flush()?;
    Ok(Frame {
        width,
        height,
        pixels: vec![0; length],
    })
}

pub(crate) fn request(stream: &mut impl Write, size: (u16, u16)) -> io::Result<()> {
    stream.write_all(&[3, 1, 0, 0, 0, 0])?;
    stream.write_all(&size.0.to_be_bytes())?;
    stream.write_all(&size.1.to_be_bytes())?;
    stream.flush()
}

pub(crate) fn key(stream: &mut impl Write, keysym: u32, pressed: bool) -> io::Result<()> {
    stream.write_all(&[4, u8::from(pressed), 0, 0])?;
    stream.write_all(&keysym.to_be_bytes())?;
    stream.flush()
}

pub(crate) fn pointer(stream: &mut impl Write, buttons: u8, x: u16, y: u16) -> io::Result<()> {
    stream.write_all(&[5, buttons])?;
    stream.write_all(&x.to_be_bytes())?;
    stream.write_all(&y.to_be_bytes())?;
    stream.flush()
}

pub(crate) fn frame(stream: &mut impl Read, previous: &mut Frame) -> io::Result<Frame> {
    loop {
        match read::<1>(stream)?[0] {
            0 => break,
            2 => (),
            3 => {
                let header = read::<7>(stream)?;
                let length = u32::from_be_bytes(header[3..].try_into().unwrap());
                if length > 1024 * 1024 {
                    return Err(invalid("RFB clipboard exceeds limit"));
                }
                let copied = io::copy(&mut stream.take(u64::from(length)), &mut io::sink())?;
                if copied != u64::from(length) {
                    return Err(invalid("truncated RFB clipboard"));
                }
            }
            _ => return Err(invalid("unsupported RFB server message")),
        }
    }
    let header = read::<3>(stream)?;
    let count = u16::from_be_bytes([header[1], header[2]]);
    if count > 64 {
        return Err(invalid("too many RFB rectangles"));
    }
    let mut size = (previous.width, previous.height);
    let mut rectangles = Vec::new();
    let mut total = 0;
    for index in 0..count {
        let header = read::<12>(stream)?;
        let [x, y, width, height] =
            std::array::from_fn(|i| u16::from_be_bytes([header[i * 2], header[i * 2 + 1]]));
        let encoding = i32::from_be_bytes(header[8..12].try_into().unwrap());
        match encoding {
            0 => {
                let bytes = pixel_bytes(width, height)?;
                total += bytes;
                if total > MAX_FRAME {
                    return Err(invalid("RFB update exceeds the 32 MiB limit"));
                }
                let mut pixels = vec![0; bytes];
                stream.read_exact(&mut pixels)?;
                rectangles.push((x, y, width, height, pixels));
            }
            -223 if index + 1 == count && x == 0 && y == 0 => {
                pixel_bytes(width, height)?;
                size = (width, height);
            }
            _ => {
                return Err(invalid(
                    "unsupported RFB rectangle encoding or resize order",
                ));
            }
        }
    }
    if size != (previous.width, previous.height) {
        *previous = Frame {
            width: size.0,
            height: size.1,
            pixels: vec![0; pixel_bytes(size.0, size.1)?],
        };
    }
    for (x, y, width, height, pixels) in rectangles {
        if u32::from(x) + u32::from(width) > u32::from(size.0)
            || u32::from(y) + u32::from(height) > u32::from(size.1)
        {
            return Err(invalid("RFB rectangle lies outside the framebuffer"));
        }
        for row in 0..usize::from(height) {
            let source = row * usize::from(width) * 4;
            let target = ((usize::from(y) + row) * usize::from(size.0) + usize::from(x)) * 4;
            previous.pixels[target..target + usize::from(width) * 4]
                .copy_from_slice(&pixels[source..source + usize::from(width) * 4]);
        }
    }
    Ok(previous.clone())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn rectangle(x: u16, y: u16, w: u16, h: u16, encoding: i32, data: &[u8]) -> Vec<u8> {
        let mut result = Vec::new();
        for n in [x, y, w, h] {
            result.extend(n.to_be_bytes());
        }
        result.extend(encoding.to_be_bytes());
        result.extend(data);
        result
    }
    fn update(rectangles: &[Vec<u8>]) -> Cursor<Vec<u8>> {
        let mut result = vec![0, 0];
        result.extend(u16::try_from(rectangles.len()).unwrap().to_be_bytes());
        for r in rectangles {
            result.extend(r);
        }
        Cursor::new(result)
    }
    fn initial() -> Frame {
        Frame {
            width: 1,
            height: 1,
            pixels: vec![0; 4],
        }
    }

    #[test]
    fn resize_after_raw_preserves_pixels_and_rejects_bad_order() {
        let mut prior = initial();
        let mut wire = update(&[
            rectangle(0, 0, 2, 1, 0, &[1, 2, 3, 0, 4, 5, 6, 0]),
            rectangle(0, 0, 2, 1, -223, &[]),
        ]);
        let actual = frame(&mut wire, &mut prior).unwrap();
        assert_eq!(actual.width, 2);
        assert_eq!(actual.pixels, [1, 2, 3, 0, 4, 5, 6, 0]);
        frame(
            &mut update(&[
                rectangle(0, 0, 2, 1, -223, &[]),
                rectangle(0, 0, 1, 1, 0, &[0; 4]),
            ]),
            &mut prior,
        )
        .unwrap_err();
    }

    #[test]
    fn invalid_rectangles_and_truncated_payloads_fail_closed() {
        for rect in [
            rectangle(1, 0, 1, 1, 0, &[0; 4]),
            rectangle(0, 0, 65535, 65535, 0, &[]),
            rectangle(0, 0, 1, 1, 0, &[0; 3]),
            rectangle(0, 0, 0, 1, 0, &[]),
            rectangle(0, 0, 1, 1, 42, &[]),
        ] {
            frame(&mut update(&[rect]), &mut initial()).unwrap_err();
        }
        frame(&mut Cursor::new(vec![0, 0, 0, 65]), &mut initial()).unwrap_err();
    }

    #[test]
    fn partial_updates_preserve_other_pixels() {
        let mut prior = Frame {
            width: 2,
            height: 1,
            pixels: vec![9; 8],
        };
        let actual = frame(
            &mut update(&[rectangle(1, 0, 1, 1, 0, &[1, 2, 3, 4])]),
            &mut prior,
        )
        .unwrap();
        assert_eq!(actual.pixels, [9, 9, 9, 9, 1, 2, 3, 4]);
    }

    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }
    impl Read for Duplex {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            self.input.read(b)
        }
    }
    impl Write for Duplex {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.output.extend(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    fn greeting(method: u8) -> Duplex {
        let mut data = b"RFB 003.008\n".to_vec();
        data.extend([1, method]);
        if method == 2 {
            data.extend(0u8..16);
        }
        data.extend([0; 4]);
        data.extend(1u16.to_be_bytes());
        data.extend(1u16.to_be_bytes());
        data.extend(FORMAT);
        data.extend([0; 4]);
        Duplex {
            input: Cursor::new(data),
            output: vec![],
        }
    }

    #[test]
    fn client_authentication_never_downgrades_configured_password() {
        let password = Password::new("password").unwrap();
        handshake(&mut greeting(1), Some(&password)).unwrap_err();
        handshake(&mut greeting(2), None).unwrap_err();
        let mut wire = greeting(2);
        handshake(&mut wire, Some(&password)).unwrap();
        assert_eq!(
            &wire.output[13..29],
            &[
                0xb8, 0x66, 0x92, 0x41, 0x25, 0xc8, 0xee, 0xbb, 0x9d, 0xeb, 0xc1, 0xdb, 0x61, 0xc5,
                0x38, 0xe2
            ]
        );
        let mut wire = greeting(1);
        handshake(&mut wire, None).unwrap();
        assert_eq!(wire.output[12], 1);
    }

    #[test]
    fn header_bounds_are_checked_before_allocating_or_reading_pixels() {
        let mut wire = greeting(1);
        wire.input.get_mut()[18..20].copy_from_slice(&0u16.to_be_bytes());
        handshake(&mut wire, None).unwrap_err();
        let mut wire = greeting(1);
        wire.input.get_mut()[38..42].copy_from_slice(&1u32.to_be_bytes());
        handshake(&mut wire, None).unwrap_err();
        let mut wire = greeting(1);
        wire.input.get_mut()[38..42].copy_from_slice(&4097u32.to_be_bytes());
        handshake(&mut wire, None).unwrap_err();
    }

    #[test]
    fn key_pointer_and_refresh_messages_match_rfb_wire_format() {
        let mut out = vec![];
        key(&mut out, 0xffeb, true).unwrap();
        pointer(&mut out, 1, 799, 599).unwrap();
        request(&mut out, (800, 600)).unwrap();
        assert_eq!(
            out,
            [
                4, 1, 0, 0, 0, 0, 255, 235, 5, 1, 3, 31, 2, 87, 3, 1, 0, 0, 0, 0, 3, 32, 2, 88
            ]
        );
    }
}
