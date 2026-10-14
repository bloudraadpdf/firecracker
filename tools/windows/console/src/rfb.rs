// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use crate::{Backend, Frame, invalid, keys};
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

pub fn serve(stream: &mut (impl Read + Write), backend: &Backend) -> io::Result<()> {
    stream.write_all(b"RFB 003.008\n")?;
    stream.flush()?;
    if read::<12>(stream)? != *b"RFB 003.008\n" {
        return Err(invalid("RFB 3.8 required"));
    }
    stream.write_all(&[1, 1])?;
    stream.flush()?;
    if read::<1>(stream)? != [1] {
        return Err(invalid("invalid RFB security selection"));
    }
    stream.write_all(&[0; 4])?;
    stream.flush()?;
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
    let mut pointer = None;
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
                if let Some((code, extended)) =
                    keys::set2(u32::from_be_bytes(request[3..].try_into().unwrap()))
                {
                    backend.input(1, &[code, u8::from(extended), request[0]])?;
                }
            }
            5 => {
                let request = read::<5>(stream)?;
                let x = u16::from_be_bytes([request[1], request[2]]);
                let y = u16::from_be_bytes([request[3], request[4]]);
                let (old_x, old_y) = pointer.unwrap_or((x, y));
                pointer = Some((x, y));
                let dx = (i32::from(x) - i32::from(old_x)).clamp(-255, 255) as i16;
                let dy = (i32::from(old_y) - i32::from(y)).clamp(-255, 255) as i16;
                let buttons = request[0] & 1 | (request[0] & 2) << 1 | (request[0] & 4) >> 1;
                let mut payload = [0; 5];
                payload[0] = buttons;
                payload[1..3].copy_from_slice(&dx.to_le_bytes());
                payload[3..5].copy_from_slice(&dy.to_le_bytes());
                backend.input(2, &payload)?;
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
