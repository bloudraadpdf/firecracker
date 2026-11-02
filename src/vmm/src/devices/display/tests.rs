// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::*;

fn vbe(vga: &mut Vga, register: u16, value: u16) {
    vga.write_port(0x1ce, &register.to_le_bytes());
    vga.write_port(0x1cf, &value.to_le_bytes());
}

fn indexed(vga: &mut Vga, port: u16, register: u8, value: u8) {
    vga.write_port(port, &[register, value]);
}

fn mode(vga: &mut Vga, width: u16, height: u16, bpp: u16) {
    vbe(vga, 4, 0);
    vbe(vga, 1, width);
    vbe(vga, 2, height);
    vbe(vga, 3, bpp);
    vbe(vga, 4, 0x41);
}

#[test]
fn legacy_registers_preserve_defined_bits_only() {
    let mut vga = Vga::default();
    mode(&mut vga, 640, 480, 32);
    for (port, masks) in [
        (0x3c4, &[0x03, 0x3d, 0x0f, 0x3f, 0x0e, 0, 0, 0][..]),
        (
            0x3ce,
            &[
                0x0f, 0x0f, 0x0f, 0x1f, 0x03, 0x7b, 0x0f, 0x0f, 0xff, 0, 0, 0, 0, 0, 0, 0,
            ][..],
        ),
    ] {
        for (index, mask) in masks.iter().enumerate() {
            vga.sequencer.fill(0xff);
            vga.graphics.fill(0xff);
            vga.write_port(port, &[u8::try_from(index).unwrap()]);
            let mut actual = [0];
            vga.read_port(port + 1, &mut actual);
            assert_eq!(actual[0], *mask);
            for value in 0..=u8::MAX {
                indexed(&mut vga, port, u8::try_from(index).unwrap(), value);
                let mut actual = [0];
                vga.read_port(port + 1, &mut actual);
                assert_eq!(actual[0], value & mask);
            }
        }
    }
    assert_eq!(vga.read_vbe(4), 0x41);
}

#[test]
fn dispi_pio_and_mmio_agree() {
    let mut vga = Vga::default();
    vga.write_port(0x1ce, &[0, 0]);
    let mut value = [0; 2];
    vga.read_port(0x1cf, &mut value);
    assert_eq!(u16::from_le_bytes(value), 0xb0c5);
    vga.write_registers(0x502, &640u16.to_le_bytes());
    vga.write_port(0x1ce, &[1, 0]);
    vga.read_port(0x1d0, &mut value);
    assert_eq!(u16::from_le_bytes(value), 640);
    vbe(&mut vga, 4, GET_CAPS);
    vga.read_registers(0x502, &mut value);
    assert_eq!(u16::from_le_bytes(value), MAX_DIMENSION);
    vga.read_registers(0x514, &mut value);
    assert_eq!(u16::from_le_bytes(value), 256);
}

#[test]
fn vbe_framebuffer_stride_and_pan() {
    let mut vga = Vga::default();
    mode(&mut vga, 2, 2, 32);
    vbe(&mut vga, 6, 4);
    vbe(&mut vga, 8, 1);
    vbe(&mut vga, 9, 1);
    vga.write_framebuffer(20, &[0x33, 0x22, 0x11, 0xff]);
    vga.write_framebuffer(40, &[0xcc, 0xbb, 0xaa, 0]);
    let frame = vga.render();
    assert_eq!((frame.width, frame.height), (2, 2));
    assert_eq!(frame.pixels, [0x112233, 0, 0, 0xaabbcc]);
    vbe(&mut vga, 8, 4);
    assert_eq!(vga.read_vbe(8), 1);
    vbe(&mut vga, 9, u16::MAX);
    assert_eq!(vga.read_vbe(9), 1);
}

#[test]
fn vbe_mode_clear_preserve_and_bounds() {
    let mut vga = Vga::default();
    mode(&mut vga, 640, 480, 32);
    vga.write_framebuffer(0, &[7]);
    vbe(&mut vga, 4, 0);
    vbe(&mut vga, 4, 0xc1);
    assert_eq!(vga.memory.byte(0), 7);
    vbe(&mut vga, 4, 0);
    vbe(&mut vga, 4, 0x41);
    assert_eq!(vga.memory.byte(0), 0);
    vbe(&mut vga, 1, 800);
    assert_eq!(vga.read_vbe(1), 640);
    vbe(&mut vga, 4, 0);
    vbe(&mut vga, 1, u16::MAX);
    assert_eq!(vga.read_vbe(1), 640);
    vbe(&mut vga, 1, 0);
    vbe(&mut vga, 4, 1);
    assert_eq!(vga.read_vbe(4), 0);
    for address in [FRAMEBUFFER_SIZE as u64, u64::MAX] {
        vga.write_framebuffer(address, &[1, 2, 3]);
        let mut value = [0; 3];
        vga.read_framebuffer(address, &mut value);
        assert_eq!(value, [255; 3]);
    }
}

#[test]
fn vbe_color_formats_and_bank_window() {
    let mut vga = Vga::default();
    for (bpp, bytes) in [
        (15, vec![0, 0x7c]),
        (16, vec![0, 0xf8]),
        (24, vec![0, 0, 255]),
        (32, vec![0, 0, 255, 0]),
    ] {
        mode(&mut vga, 1, 1, bpp);
        vga.write_framebuffer(0, &bytes);
        assert_eq!(vga.render().pixels, [0xff0000]);
    }
    vbe(&mut vga, 5, 255);
    vga.write_legacy(0xffff, &[0x91, 0x92]);
    assert_eq!(vga.memory.byte(FRAMEBUFFER_SIZE - 1), 0x91);
    vbe(&mut vga, 5, 256);
    assert_eq!(vga.read_vbe(5), 255);
    let mut data = [0; 2];
    vga.read_legacy(0xffff, &mut data);
    assert_eq!(data, [0x91, 0xff]);
}

#[test]
fn dac_increment_mask_and_precision() {
    let mut vga = Vga::default();
    mode(&mut vga, 1, 1, 8);
    vga.write_port(0x3c8, &[0xff]);
    for byte in [63, 0, 0, 0, 63, 0] {
        vga.write_port(0x3c9, &[byte]);
    }
    assert_eq!(vga.dac_write, 1);
    vga.write_framebuffer(0, &[0xff]);
    assert_eq!(vga.render().pixels, [0xff0000]);
    vga.write_port(0x3c6, &[0]);
    assert_eq!(vga.render().pixels, [0x00ff00]);
    vga.write_port(0x3c7, &[0xff]);
    let mut value = [0];
    vga.read_port(0x3c9, &mut value);
    assert_eq!(value, [63]);
    vbe(&mut vga, 4, 0x61);
    assert_eq!(vga.render().pixels, [0x003f00]);
}

fn planar(vga: &mut Vga) {
    indexed(vga, 0x3c4, 2, 15);
    indexed(vga, 0x3c4, 4, 4);
    indexed(vga, 0x3ce, 6, 4);
    indexed(vga, 0x3ce, 8, 255);
}

#[test]
fn vga_latches_write_modes_and_compare() {
    let mut vga = Vga::default();
    planar(&mut vga);
    vga.write_legacy(0, &[0x96]);
    assert_eq!(&memory_bytes(&vga, 0, 4), &[0x96; 4]);
    let mut data = [0];
    vga.read_legacy(0, &mut data);
    indexed(&mut vga, 0x3ce, 5, 1);
    vga.write_legacy(1, &[0]);
    assert_eq!(&memory_bytes(&vga, 4, 4), &[0x96; 4]);
    indexed(&mut vga, 0x3ce, 5, 2);
    indexed(&mut vga, 0x3ce, 8, 0xf0);
    vga.write_legacy(2, &[5]);
    assert_eq!(&memory_bytes(&vga, 8, 4), &[0xf6, 6, 0xf6, 6]);
    indexed(&mut vga, 0x3ce, 5, 3);
    indexed(&mut vga, 0x3ce, 0, 10);
    indexed(&mut vga, 0x3ce, 8, 255);
    vga.write_legacy(3, &[0x0f]);
    assert_eq!(&memory_bytes(&vga, 12, 4), &[0x90, 0x9f, 0x90, 0x9f]);
    indexed(&mut vga, 0x3ce, 5, 8);
    indexed(&mut vga, 0x3ce, 7, 15);
    indexed(&mut vga, 0x3ce, 2, 5);
    vga.read_legacy(2, &mut data);
    assert_eq!(data, [0xf0]);
}

#[test]
fn chain_four_and_text_odd_even_addressing() {
    let mut vga = Vga::default();
    planar(&mut vga);
    indexed(&mut vga, 0x3c4, 4, 8);
    vga.write_legacy(0, &[1, 2, 3, 4, 5]);
    assert_eq!(&memory_bytes(&vga, 0, 5), &[1, 2, 3, 4, 5]);
    indexed(&mut vga, 0x3c4, 4, 0);
    indexed(&mut vga, 0x3c4, 2, 3);
    indexed(&mut vga, 0x3ce, 6, 12);
    indexed(&mut vga, 0x3ce, 5, 0x10);
    vga.write_legacy(0x18000, &[b'A', 0x1f, b'B', 0x2f]);
    assert_eq!(
        &memory_bytes(&vga, 0, 8),
        &[b'A', 0x1f, 3, 4, b'B', 0x2f, 0, 0]
    );
    let mut text = [0; 4];
    vga.read_legacy(0x18000, &mut text);
    assert_eq!(&text, b"A\x1fB\x2f");
    vga.read_legacy(0, &mut text);
    assert_eq!(text, [0xff; 4]);
}

#[test]
fn firmware_uploaded_font_text_and_cursor() {
    let mut vga = Vga::default();
    planar(&mut vga);
    indexed(&mut vga, 0x3c4, 2, 4);
    vga.write_legacy(u64::from(b'A') * 32, &[0x81]);
    indexed(&mut vga, 0x3c4, 4, 0);
    indexed(&mut vga, 0x3c4, 2, 3);
    indexed(&mut vga, 0x3ce, 6, 12);
    vga.write_legacy(0x18000, &[b'A', 0x0f]);
    indexed(&mut vga, 0x3c4, 1, 1);
    indexed(&mut vga, 0x3d4, 1, 0);
    indexed(&mut vga, 0x3d4, 0x12, 0);
    indexed(&mut vga, 0x3d4, 0x18, 0xff);
    indexed(&mut vga, 0x3d4, 0xa, 0x20);
    vga.write_port(0x3c0, &[15]);
    vga.write_port(0x3c0, &[15]);
    vga.write_port(0x3c0, &[0x20]);
    vga.write_port(0x3c8, &[15]);
    for _ in 0..3 {
        vga.write_port(0x3c9, &[63]);
    }
    assert_eq!(vga.render().pixels, [0xffffff, 0, 0, 0, 0, 0, 0, 0xffffff]);
    indexed(&mut vga, 0x3d4, 0xa, 0);
    assert_eq!(vga.render().pixels, [0xffffff; 8]);
}

#[test]
fn attribute_flip_flop_and_crtc_protection() {
    let mut vga = Vga::default();
    vga.write_port(0x3c0, &[5]);
    let mut value = [0];
    vga.read_port(0x3da, &mut value);
    vga.write_port(0x3c0, &[6]);
    vga.write_port(0x3c0, &[42]);
    assert_eq!(vga.attribute[6], 42);
    assert_eq!(vga.attribute[5], 0);
    indexed(&mut vga, 0x3d4, 0x11, 0x80);
    indexed(&mut vga, 0x3d4, 1, 80);
    assert_eq!(vga.crtc[1], 0);
    indexed(&mut vga, 0x3d4, 7, 0xff);
    assert_eq!(vga.crtc[7], 0x10);
}

#[test]
fn snapshot_roundtrip_and_invalid_state() {
    let mut vga = Vga::default();
    mode(&mut vga, 2, 1, 24);
    vga.write_framebuffer(0, &[1, 2, 3, 4, 5, 6]);
    let bytes = bitcode::serialize(&vga).unwrap();
    let restored: Vga = bitcode::deserialize(&bytes).unwrap();
    assert!(restored.save().validate());
    assert_eq!(restored.render(), vga.render());
    let mut invalid = vga.save();
    invalid.memory.pop();
    let invalid = bitcode::serialize(&invalid).unwrap();
    bitcode::deserialize::<Vga>(&invalid).unwrap_err();
    vga.dac_component = 3;
    assert!(!vga.save().validate());
}

#[test]
fn planar_pixels_and_scanline_stride() {
    let mut vga = Vga::default();
    planar(&mut vga);
    indexed(&mut vga, 0x3c4, 1, 1);
    indexed(&mut vga, 0x3d4, 1, 0);
    indexed(&mut vga, 0x3d4, 0x12, 1);
    indexed(&mut vga, 0x3d4, 0x13, 1);
    indexed(&mut vga, 0x3d4, 0x18, 0xff);
    indexed(&mut vga, 0x3ce, 6, 5);
    for (register, value) in [(1, 1), (2, 2), (0x12, 15)] {
        vga.write_port(0x3c0, &[register]);
        vga.write_port(0x3c0, &[value]);
    }
    vga.write_port(0x3c0, &[0x20]);
    vga.write_port(0x3c8, &[1]);
    for byte in [63, 0, 0, 0, 63, 0] {
        vga.write_port(0x3c9, &[byte]);
    }
    indexed(&mut vga, 0x3c4, 2, 1);
    vga.write_legacy(0, &[0x80]);
    indexed(&mut vga, 0x3c4, 2, 2);
    vga.write_legacy(2, &[0x40]);
    let frame = vga.render();
    assert_eq!((frame.width, frame.height), (8, 2));
    assert_eq!(frame.pixels[0], 0xff0000);
    assert_eq!(frame.pixels[9], 0x00ff00);
    assert_eq!(frame.pixels.iter().filter(|pixel| **pixel != 0).count(), 2);
}

#[test]
fn untrusted_register_values_keep_accesses_bounded() {
    let mut vga = Vga::default();
    let mut seed = 0x12345678u32;
    let ports = [
        0x3c0, 0x3c2, 0x3c4, 0x3c5, 0x3c6, 0x3c7, 0x3c8, 0x3c9, 0x3ce, 0x3cf, 0x3b4, 0x3b5, 0x3d4,
        0x3d5,
    ];
    for index in 0..10000 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let port = ports[index % ports.len()];
        vga.write_port(port, &seed.to_le_bytes()[..2]);
        let mut data = [0; 8];
        vga.read_legacy(u64::from(seed), &mut data);
        vga.write_legacy(u64::from(seed), &data);
        vga.write_registers(u64::from(seed & 0xfff), &seed.to_le_bytes()[..2]);
        vga.read_registers(u64::MAX, &mut data);
        assert_eq!(data, [255; 8]);
    }
    assert!(vga.save().validate());
    let frame = vga.render();
    assert!(frame.width <= MAX_DIMENSION && frame.height <= MAX_DIMENSION);
    assert_eq!(
        frame.pixels.len(),
        usize::from(frame.width) * usize::from(frame.height)
    );
}

fn memory_bytes(vga: &Vga, offset: usize, len: usize) -> Vec<u8> {
    let mut bytes = vec![0; len];
    vga.memory.read(offset, &mut bytes);
    bytes
}

#[test]
fn mapped_vram_is_shared_but_snapshots_are_independent() {
    use vm_memory::{Bytes, VolatileMemory};
    let vga = Vga::default();
    let mapping = vga.framebuffer_mapping();
    assert_eq!(mapping.as_ptr() as usize % 4096, 0);
    mapping
        .get_slice(0, 4)
        .unwrap()
        .write_slice(&[1, 2, 3, 4], 0)
        .unwrap();
    assert_eq!(memory_bytes(&vga, 0, 4), [1, 2, 3, 4]);
    let snapshot = vga.clone();
    mapping
        .get_slice(0, 4)
        .unwrap()
        .write_slice(&[5, 6, 7, 8], 0)
        .unwrap();
    assert_eq!(memory_bytes(&snapshot, 0, 4), [1, 2, 3, 4]);
    let encoded = bitcode::serialize(&vga).unwrap();
    let restored: Vga = bitcode::deserialize(&encoded).unwrap();
    assert_eq!(memory_bytes(&restored, 0, 4), [5, 6, 7, 8]);
    assert!(!std::sync::Arc::ptr_eq(
        &mapping,
        &restored.framebuffer_mapping()
    ));
}

fn firmware_mode(vga: &mut Vga, width: u16, height: u16) {
    for (offset, value) in [(0x402, 1), (0x400, 0x20)] {
        vga.write_registers(offset, &[value]);
    }
    for (register, value) in [
        (4, 0),
        (5, 0),
        (8, 0),
        (9, 0),
        (3, 32),
        (1, width),
        (6, width),
        (2, height),
        (7, height),
        (4, 0x41),
    ] {
        vga.write_registers(0x500 + register * 2, &value.to_le_bytes());
    }
}

#[test]
fn firmware_mmio_mode_and_guest_vram_survive_restore() {
    use vm_memory::{Bytes, VolatileMemory};

    let mut vga = Vga::default();
    let mut register = [0; 2];
    for (offset, expected) in [(0x500, 0xb0c5u16), (0x514, 256)] {
        vga.read_registers(offset, &mut register);
        assert_eq!(u16::from_le_bytes(register), expected);
    }
    firmware_mode(&mut vga, 640, 480);
    let mapping = vga.framebuffer_mapping();
    mapping
        .get_slice(0, 4)
        .unwrap()
        .write_slice(&[0x33, 0x22, 0x11, 0], 0)
        .unwrap();
    let frame = vga.render();
    assert_eq!((frame.width, frame.height), (640, 480));
    assert_eq!(frame.pixels[0], 0x112233);
    let encoded = bitcode::serialize(&vga).unwrap();
    let restored: Vga = bitcode::deserialize(&encoded).unwrap();
    assert!(restored.save().validate());
    assert_eq!(restored.render(), frame);
    assert!(!std::sync::Arc::ptr_eq(
        &mapping,
        &restored.framebuffer_mapping()
    ));
    firmware_mode(&mut vga, 800, 600);
    assert_eq!(vga.render().pixels[0], 0);
    assert_eq!(restored.render(), frame);
}

#[test]
fn mmio_access_widths_and_framebuffer_edges_are_bounded() {
    let mut vga = Vga::default();
    firmware_mode(&mut vga, 640, 480);
    for (offset, data) in [
        (0x502, vec![0xff]),
        (0x502, vec![0xff; 4]),
        (0x503, vec![0xff; 2]),
        (0x516, vec![0xff; 2]),
        (u64::MAX, vec![0xff; 8]),
    ] {
        vga.write_registers(offset, &data);
        let mut result = vec![0; data.len()];
        vga.read_registers(offset, &mut result);
        assert_eq!(result, data);
    }
    assert_eq!((vga.render().width, vga.render().height), (640, 480));
    let last = FRAMEBUFFER_SIZE as u64 - 1;
    vga.write_framebuffer(last, &[0x5a]);
    for offset in [last, FRAMEBUFFER_SIZE as u64, u64::MAX] {
        vga.write_framebuffer(offset, &[1, 2]);
        let mut result = [0; 2];
        vga.read_framebuffer(offset, &mut result);
        assert_eq!(result, [0xff; 2]);
    }
    let mut result = [0];
    vga.read_framebuffer(last, &mut result);
    assert_eq!(result, [0x5a]);
    assert!(vga.save().validate());
}
