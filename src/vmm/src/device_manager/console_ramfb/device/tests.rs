// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::*;

fn fixture() -> Ramfb {
    Ramfb::new(
        crate::test_utils::single_region_mem(2 * 1024 * 1024),
        RamfbState::default(),
    )
}

fn dma(device: &mut Ramfb, control: u32, length: u32, buffer: u64) -> u32 {
    let mut descriptor = control.to_be_bytes().to_vec();
    descriptor.extend(length.to_be_bytes());
    descriptor.extend(buffer.to_be_bytes());
    device
        .memory
        .write_slice(&descriptor, GuestAddress(0x100))
        .unwrap();
    device.write(MMIO_BASE, 16, &0u32.to_be_bytes());
    device.write(MMIO_BASE, 20, &0x100u32.to_be_bytes());
    let mut status = [0; 4];
    device
        .memory
        .read_slice(&mut status, GuestAddress(0x100))
        .unwrap();
    u32::from_be_bytes(status)
}

fn configure(device: &mut Ramfb) {
    let mut config = 0x1000u64.to_be_bytes().to_vec();
    for value in [0x3432_5258u32, 0, 16, 16, 80] {
        config.extend(value.to_be_bytes());
    }
    device
        .memory
        .write_slice(&config, GuestAddress(0x200))
        .unwrap();
    assert_eq!(dma(device, u32::from(RAMFB) << 16 | 8 | 16, 28, 0x200), 0);
}

#[test]
fn firmware_discovery_dma_bounds_and_register_state() {
    let mut device = fixture();
    device.write(MMIO_BASE, 8, &0u16.to_be_bytes());
    let mut signature = [0; 8];
    device.read(MMIO_BASE, 0, &mut signature);
    assert_eq!(&signature, b"QEMU\0\0\0\0");
    device.read(MMIO_BASE, 16, &mut signature);
    assert_eq!(&signature, b"QEMU CFG");
    assert_eq!(dma(&mut device, 0x19 << 16 | 8 | 2, 68, 0x200), 0);
    let mut directory = [0; 68];
    device
        .memory
        .read_slice(&mut directory, GuestAddress(0x200))
        .unwrap();
    assert_eq!(u32::from_be_bytes(directory[..4].try_into().unwrap()), 1);
    assert_eq!(u32::from_be_bytes(directory[4..8].try_into().unwrap()), 28);
    assert_eq!(
        u16::from_be_bytes(directory[8..10].try_into().unwrap()),
        RAMFB
    );
    assert_eq!(&directory[12..22], b"etc/ramfb\0");
    assert_eq!(dma(&mut device, 8 | 16, 4, 0x200), 1);
    assert_eq!(dma(&mut device, 8 | 2, 8, u64::MAX - 2), 1);
    assert_eq!(
        dma(
            &mut device,
            8 | 2,
            u32::try_from(MAX_TRANSFER + 1).unwrap(),
            0
        ),
        1
    );
    assert_eq!(dma(&mut device, 0xffff, 0, 0), 1);
    configure(&mut device);
    let original = device.state.config;
    assert_eq!(dma(&mut device, 16, 1, 0x200), 1);
    assert_eq!(original, device.state.config);
    device.write(MMIO_BASE, 8, &RAMFB.to_be_bytes());
    assert_eq!(dma(&mut device, 4, 24, 0), 0);
    device.write(MMIO_BASE, 16, &0x1234u32.to_be_bytes());
    let state = bitcode::serialize(&device.save()).unwrap();
    let mut restored = Ramfb::new(device.memory.clone(), bitcode::deserialize(&state).unwrap());
    let mut stride = [0; 4];
    restored.read(MMIO_BASE, 0, &mut stride);
    assert_eq!(u32::from_be_bytes(stride), 80);
    assert_eq!(restored.state.dma_high, 0x1234);
}

#[test]
fn framebuffer_uses_reserved_ram_and_survives_device_restore() {
    let mut device = fixture();
    configure(&mut device);
    for (offset, pixel) in [(0, 0xff12_3456u32), (80, 0xffab_cdef)] {
        device
            .memory
            .write_slice(&pixel.to_le_bytes(), GuestAddress(0x1000 + offset))
            .unwrap();
    }
    let frame = device.render();
    assert_eq!((frame.width, frame.height), (16, 16));
    assert_eq!(frame.pixels[0], 0x123456);
    assert_eq!(frame.pixels[16], 0xabcdef);
    let state = bitcode::serialize(&device.save()).unwrap();
    let restored = Ramfb::new(device.memory.clone(), bitcode::deserialize(&state).unwrap());
    assert_eq!(restored.render(), frame);
    device
        .memory
        .write_slice(&[1, 2, 3, 4], GuestAddress(0x1000))
        .unwrap();
    assert_eq!(restored.render().pixels[0], 0x030201);
}

#[test]
fn invalid_geometry_and_memory_ranges_never_reach_scanout() {
    let mut device = fixture();
    let blank = device.render();
    assert_eq!((blank.width, blank.height), (800, 600));
    assert_eq!(blank.pixels.len(), 800 * 600);
    assert!(blank.pixels.iter().all(|pixel| *pixel == 0));
    configure(&mut device);
    let valid = device.state.config;
    for (offset, value) in [
        (8, 0),
        (12, 1),
        (16, 0),
        (16, 2049),
        (20, u32::MAX),
        (24, 1),
        (24, u32::MAX),
    ] {
        device.state.config = valid;
        device.state.config[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        assert_eq!(device.render(), blank);
    }
    device.state.config = valid;
    device.state.config[..8].copy_from_slice(&(u64::MAX - 32).to_be_bytes());
    assert_eq!(device.render(), blank);
}

#[test]
fn configuration_page_cannot_overlap_restored_ram_or_devices() {
    for (base, length) in [
        (MMIO_BASE, 1),
        (MMIO_BASE - 1, 2),
        (MMIO_BASE + MMIO_SIZE - 1, 1),
        (u64::MAX, 2),
        (0, 0),
    ] {
        Ramfb::validate_mapping(base, length).unwrap_err();
    }
    Ramfb::validate_mapping(MMIO_BASE - 1, 1).unwrap();
    Ramfb::validate_mapping(MMIO_BASE + MMIO_SIZE, 1).unwrap();
}
